//! Chunk manager: loads chunks around the player and unloads distant ones.
//!
//! 使用分帧加载队列避免一帧内同步加载大量区块导致卡顿。
//! 区块按与玩家的距离排序，每帧只加载固定数量（`CHUNKS_PER_FRAME`）。
//! 使用LRU（最近最少使用）缓存淘汰机制，优先卸载最久未访问且距离较远的区块。
//!
//! # 异步网格生成
//!
//! 网格生成已迁移到后台工作线程（Phase 0 优化），消除加载尖峰：
//! - 主线程：地形生成 + 任务提交（轻量）
//! - 工作线程：网格计算（CPU 密集）
//! - 主线程：结果收集 + GPU 上传（分帧控制）

use bevy::asset::RenderAssetUsages;
use bevy::image::ImageSampler;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use std::collections::HashMap;

use crate::async_mesh::{AsyncMeshManager, MESH_UPLOADS_PER_FRAME, MeshTask, UvLookupTable};
use crate::chunk::{Chunk, ChunkCoord, ChunkNeighbors, fill_terrain};
use crate::chunk_dirty::{
    ChunkAtlasHandle, ChunkCoordComponent, ChunkMeshHandle, DirtyChunk, is_air_chunk,
};
use crate::resource_pack::{ResourcePackManager, VoxelMaterial};

/// 渲染距离（区块数）。增大此值可以看到更远的世界，但需要更多区块加载。
pub const RENDER_DISTANCE: i32 = 8;
/// 卸载距离：超过此距离的区块会被卸载。比渲染距离大 1 避免边界闪烁。
pub const UNLOAD_DISTANCE: i32 = RENDER_DISTANCE + 1;
/// 每帧最多提交到异步队列的区块数。控制任务提交速率，避免工作线程积压。
pub const CHUNKS_PER_FRAME: usize = 4;
/// 最大缓存区块数。当超过此数量时，使用LRU策略淘汰最久未访问的区块。
/// 默认值：渲染距离内约 8*8*π*9 ≈ 1800 个区块，设置为 2000 留有余量。
pub const MAX_CACHED_CHUNKS: usize = 2000;
/// LRU淘汰时每帧最多卸载的区块数。避免一帧内卸载太多导致卡顿。
pub const LRU_UNLOADS_PER_FRAME: usize = 8;

/// 已加载区块的条目，包含实体、区块数据、GPU 资源句柄和 LRU 访问信息。
///
/// `mesh_handle` 和 `material_handle` 用于在卸载/淘汰时正确释放 GPU 资源，
/// 避免 `Assets<Mesh>` 和 `Assets<StandardMaterial>` 中的资源泄漏（P0 #1 修复）。
pub struct ChunkEntry {
    pub entity: Entity,
    pub data: Chunk,
    /// 最后访问时间戳（帧号），用于LRU淘汰
    pub last_accessed: u64,
    /// 区块网格的 Mesh 句柄，卸载时需要从 Assets 中移除
    pub mesh_handle: Handle<Mesh>,
    /// 区块材质的 VoxelMaterial 句柄，卸载时需要从 Assets 中移除
    pub material_handle: Handle<VoxelMaterial>,
}

#[derive(Resource)]
pub struct LoadedChunks {
    pub entries: HashMap<ChunkCoord, ChunkEntry>,
    /// 分帧加载队列：按距离排序的待加载区块坐标
    pub load_queue: Vec<ChunkCoord>,
    /// 上一次玩家所在的区块坐标，用于检测玩家是否移动到了新区块
    pub last_player_chunk: Option<ChunkCoord>,
    /// 当前帧号，用于LRU时间戳
    pub frame_counter: u64,
}

impl Default for LoadedChunks {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            load_queue: Vec::new(),
            last_player_chunk: None,
            frame_counter: 0,
        }
    }
}

/// Y 轴加载半径：玩家上下各加载多少层 Y 区块。
/// 每层 32 格，±4 层 = ±128 米，覆盖玩家周围主要交互高度。
pub const Y_LOAD_RADIUS: i32 = 4;
/// Y 轴卸载半径：超过此距离的 Y 区块会被卸载。比加载半径大 1 避免边界闪烁。
pub const Y_UNLOAD_RADIUS: i32 = Y_LOAD_RADIUS + 1;

/// 存储 Atlas 纹理句柄的资源
#[derive(Resource)]
pub struct AtlasTextureHandle {
    pub handle: Handle<Image>,
}

/// 6 个方向的偏移量，与 chunk.rs 中 FACES 顺序一致：[+X, -X, +Y, -Y, +Z, -Z]
const NEIGHBOR_OFFSETS: [(i32, i32, i32); 6] = [
    (1, 0, 0),  // +X (Right)
    (-1, 0, 0), // -X (Left)
    (0, 1, 0),  // +Y (Top)
    (0, -1, 0), // -Y (Bottom)
    (0, 0, 1),  // +Z (Front)
    (0, 0, -1), // -Z (Back)
];

/// 从已加载区块中收集指定坐标的 6 个邻居数据
fn collect_neighbors(coord: ChunkCoord, loaded: &LoadedChunks) -> ChunkNeighbors {
    let mut neighbors = ChunkNeighbors::empty();

    for (i, (dx, dy, dz)) in NEIGHBOR_OFFSETS.iter().enumerate() {
        let neighbor_coord = ChunkCoord {
            cx: coord.cx + dx,
            cy: coord.cy + dy,
            cz: coord.cz + dz,
        };

        if let Some(entry) = loaded.entries.get(&neighbor_coord) {
            neighbors.neighbor_data[i] = Some(entry.data.to_vec());
        }
    }

    neighbors
}

/// Startup system: spawns the camera and HUD, then queues initial chunks for loading.
///
/// 同时初始化异步网格管理器和 UV 查找表资源。
pub fn setup_world(
    mut commands: Commands,
    mut loaded: ResMut<LoadedChunks>,
    resource_pack: Res<ResourcePackManager>,
    mut images: ResMut<Assets<Image>>,
) {
    // 从资源包创建 Texture Array 纹理
    let atlas_handle = if let Some(atlas) = &resource_pack.atlas {
        let size = Extent3d {
            width: atlas.tex_size,
            height: atlas.tex_size,
            depth_or_array_layers: atlas.array_layers.max(1),
        };
        let pixel_data = if atlas.array_layers > 0 {
            atlas.array_pixels.clone()
        } else {
            atlas.image.clone()
        };
        let mut bevy_image = Image::new(
            size,
            TextureDimension::D2,
            pixel_data,
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::default(),
        );
        // 使用最近邻采样（Nearest），保持 16x16 像素纹理锐利，避免模糊
        bevy_image.sampler = ImageSampler::nearest();
        images.add(bevy_image)
    } else {
        images.add(Image::default())
    };

    commands.insert_resource(AtlasTextureHandle {
        handle: atlas_handle.clone(),
    });

    // 初始化异步网格管理器
    let worker_count = crate::async_mesh::default_worker_count();
    commands.insert_resource(AsyncMeshManager::new(worker_count));

    // 预构建 UV 查找表（一次性构建，所有任务共享）
    let uv_table = UvLookupTable::from_resource_pack(&resource_pack);
    commands.insert_resource(uv_table);

    use crate::camera::CameraController;
    // 摄像机初始位置在地形上方（地形基准高度 16 + 振幅 32 = 最高约 48，留余量）
    let camera_transform = Transform::from_xyz(16.0, 64.0, 16.0);
    let camera_entity = commands
        .spawn((
            Camera3d::default(),
            camera_transform,
            CameraController::default(),
        ))
        .id();

    crate::hud::setup_hud(&mut commands, camera_entity);

    // 将初始区块加入加载队列（不立即加载，由 chunk_loader_system 分帧处理）
    let center = ChunkCoord {
        cx: 0,
        cy: 0,
        cz: 0,
    };
    loaded.last_player_chunk = Some(center);
    rebuild_load_queue(center, &mut *loaded);
}

/// 每帧系统：异步网格结果收集 + 分帧任务提交 + 卸载远处区块。
///
/// 工作流程（Phase 0 异步网格生成）：
/// 1. 收集异步网格生成结果，上传到 GPU（每帧最多 `MESH_UPLOADS_PER_FRAME` 个）
/// 2. 检测玩家是否移动到新区块，若是则重建加载队列
/// 3. 从队列头部取出最多 `CHUNKS_PER_FRAME` 个区块，生成地形数据并提交异步任务
/// 4. 卸载超出 `UNLOAD_DISTANCE` 的区块（同时取消其异步任务）
pub fn chunk_loader_system(
    mut commands: Commands,
    mut materials: ResMut<Assets<VoxelMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut loaded: ResMut<LoadedChunks>,
    async_mesh: Res<AsyncMeshManager>,
    camera_query: Query<&Transform, With<Camera3d>>,
    atlas_handle: Res<AtlasTextureHandle>,
    uv_table: Res<UvLookupTable>,
    dirty_query: Query<&DirtyChunk>,
) {
    let Ok(cam_transform) = camera_query.single() else {
        return;
    };

    let player_chunk = ChunkCoord::from_world(cam_transform.translation);

    // 递增帧计数器
    loaded.frame_counter += 1;
    let current_frame = loaded.frame_counter;

    // ── 步骤 1：收集异步结果并上传 GPU ──────────────────────────
    let results = async_mesh.collect_results(MESH_UPLOADS_PER_FRAME);
    for result in results {
        // 检查区块是否已被卸载（在异步生成期间被卸载）
        if !loaded.entries.contains_key(&result.coord) {
            // 区块已卸载，丢弃结果
            continue;
        }

        // 获取已存在的实体和旧资源句柄
        if let Some(entry) = loaded.entries.get(&result.coord) {
            // 如果区块已被标记为脏（在异步生成期间发生了修改），
            // 丢弃这个过时的结果，避免上传旧网格导致"幽灵方块"。
            if dirty_query.get(entry.entity).is_ok() {
                continue;
            }
            let entity = entry.entity;

            // 移除旧的 mesh 和 material 资源
            meshes.remove(&entry.mesh_handle);
            materials.remove(&entry.material_handle);

            // 创建新 Mesh 并上传到 GPU
            // 即使网格为空（全空气区块），也需要替换旧 Mesh 以清除"幽灵方块"
            let mesh_handle = meshes.add(
                Mesh::new(
                    bevy::render::render_resource::PrimitiveTopology::TriangleList,
                    RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
                )
                .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, result.positions)
                .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, result.uvs)
                .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, result.normals)
                .with_inserted_indices(bevy::mesh::Indices::U32(result.indices)),
            );

            let mat_handle = materials.add(VoxelMaterial {
                array_texture: atlas_handle.handle.clone(),
            });

            // 更新实体组件
            commands.entity(entity).insert((
                Mesh3d(mesh_handle.clone()),
                MeshMaterial3d(mat_handle.clone()),
                ChunkMeshHandle {
                    mesh: mesh_handle.clone(),
                    material: mat_handle.clone(),
                },
            ));

            // 更新 LoadedChunks 中的句柄
            if let Some(entry) = loaded.entries.get_mut(&result.coord) {
                entry.mesh_handle = mesh_handle;
                entry.material_handle = mat_handle;
            }
        }
    }

    // ── 步骤 2：检测玩家移动，重建加载队列 ──────────────────────
    if loaded.last_player_chunk != Some(player_chunk) {
        loaded.last_player_chunk = Some(player_chunk);
        rebuild_load_queue(player_chunk, &mut *loaded);
        unload_distant_chunks(
            player_chunk,
            &mut commands,
            &mut meshes,
            &mut materials,
            &mut *loaded,
            &*async_mesh,
        );
    }

    // LRU 淘汰：当缓存区块数超过上限时，淘汰最久未访问且距离较远的区块
    lru_evict(
        player_chunk,
        &mut commands,
        &mut meshes,
        &mut materials,
        &mut *loaded,
        &*async_mesh,
    );

    // ── 步骤 3：分帧提交异步任务 ────────────────────────────────
    let drain_count = CHUNKS_PER_FRAME.min(loaded.load_queue.len());
    let chunks_to_load: Vec<ChunkCoord> = loaded.load_queue.drain(..drain_count).collect();

    for coord in chunks_to_load {
        // 跳过已加载的（可能在队列重建前已被加载）
        if loaded.entries.contains_key(&coord) {
            continue;
        }

        // 跳过已在异步队列中的
        if async_mesh.is_pending(&coord) {
            continue;
        }

        // 生成地形数据（轻量操作，保留在主线程）
        let mut chunk = Chunk::filled(0);
        fill_terrain(&mut chunk, &coord);

        // 跳过全空气区块（高于地形或低于地形的区块），不创建实体和提交任务
        if is_air_chunk(&chunk) {
            continue;
        }

        // 收集邻居数据用于跨区块面剔除
        let neighbors = collect_neighbors(coord, &*loaded);

        // 创建实体（无 Mesh，等待异步结果）
        let position = coord.to_world_origin();
        let entity = commands
            .spawn((
                chunk.clone(),
                Transform::from_translation(position),
                Visibility::default(),
                ChunkAtlasHandle(atlas_handle.handle.clone()),
                ChunkCoordComponent(coord),
            ))
            .id();

        // 创建占位 Mesh 和 Material（避免后续更新时找不到句柄）
        let placeholder_mesh = meshes.add(Mesh::new(
            bevy::render::render_resource::PrimitiveTopology::TriangleList,
            RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
        ));
        let placeholder_mat = materials.add(VoxelMaterial {
            array_texture: atlas_handle.handle.clone(),
        });

        commands.entity(entity).insert((
            Mesh3d(placeholder_mesh.clone()),
            MeshMaterial3d(placeholder_mat.clone()),
            ChunkMeshHandle {
                mesh: placeholder_mesh.clone(),
                material: placeholder_mat.clone(),
            },
        ));

        // 注册到 LoadedChunks
        loaded.entries.insert(
            coord,
            ChunkEntry {
                entity,
                data: chunk.clone(),
                last_accessed: current_frame,
                mesh_handle: placeholder_mesh,
                material_handle: placeholder_mat,
            },
        );

        // 提交异步网格生成任务
        async_mesh.submit_task(MeshTask::Generate {
            coord,
            data: chunk,
            neighbors,
            uv_table: uv_table.clone(),
        });

        // 标记邻居区块为脏，使其重新生成网格以正确剔除与新区块的接触面。
        // 当区块A先加载时，其边界面上的方块面被保留（因为邻居还未加载）。
        // 后来区块B加载后，区块A需要重新生成网格才能剔除接触面。
        for (dx, dy, dz) in NEIGHBOR_OFFSETS.iter() {
            let neighbor_coord = ChunkCoord {
                cx: coord.cx + dx,
                cy: coord.cy + dy,
                cz: coord.cz + dz,
            };
            if let Some(neighbor_entry) = loaded.entries.get(&neighbor_coord) {
                // 跳过全空气区块（无需重建网格）
                if is_air_chunk(&neighbor_entry.data) {
                    continue;
                }
                commands.entity(neighbor_entry.entity).insert(DirtyChunk);
            }
        }
    }
}

/// 重建加载队列：收集渲染距离内所有未加载的区块坐标，按与玩家的距离排序。
///
/// Y 轴使用流式加载：基于玩家当前 Y 坐标动态计算加载范围，
/// 只加载玩家上下 `Y_LOAD_RADIUS` 层的区块，支持 ±10240 格地形探索。
///
/// 近处的区块排在前面，优先加载。
fn rebuild_load_queue(center: ChunkCoord, loaded: &mut LoadedChunks) {
    let cy_min = center.cy - Y_LOAD_RADIUS;
    let cy_max = center.cy + Y_LOAD_RADIUS;

    let mut missing: Vec<ChunkCoord> = Vec::new();

    for dx in -RENDER_DISTANCE..=RENDER_DISTANCE {
        for dz in -RENDER_DISTANCE..=RENDER_DISTANCE {
            // 圆形裁剪：只加载圆形范围内的区块（而非方形），减少角落浪费
            if dx * dx + dz * dz > RENDER_DISTANCE * RENDER_DISTANCE {
                continue;
            }
            for cy in cy_min..=cy_max {
                let coord = ChunkCoord {
                    cx: center.cx + dx,
                    cy,
                    cz: center.cz + dz,
                };
                if !loaded.entries.contains_key(&coord) {
                    missing.push(coord);
                }
            }
        }
    }

    // 按与玩家的三维距离排序（近处优先）
    missing.sort_by_key(|coord| {
        let dx = (coord.cx - center.cx).abs();
        let dy = (coord.cy - center.cy).abs();
        let dz = (coord.cz - center.cz).abs();
        dx * dx + dy * dy + dz * dz
    });

    loaded.load_queue = missing;
}

/// 卸载超出加载范围的区块实体。
///
/// XZ 方向：超出 `UNLOAD_DISTANCE` 的区块卸载。
/// Y 方向：超出 `Y_UNLOAD_RADIUS` 的区块卸载。
///
/// 卸载前会清理关联的 GPU 资源（mesh + material），避免内存泄漏。
/// 同时取消该区块的异步网格生成任务。
fn unload_distant_chunks(
    center: ChunkCoord,
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<VoxelMaterial>,
    loaded: &mut LoadedChunks,
    async_mesh: &AsyncMeshManager,
) {
    let to_remove: Vec<ChunkCoord> = loaded
        .entries
        .keys()
        .filter(|coord| {
            let dx = (coord.cx - center.cx).abs();
            let dz = (coord.cz - center.cz).abs();
            let dy = (coord.cy - center.cy).abs();
            dx > UNLOAD_DISTANCE || dz > UNLOAD_DISTANCE || dy > Y_UNLOAD_RADIUS
        })
        .copied()
        .collect();

    for coord in to_remove {
        // 取消异步任务（如果还在处理中）
        async_mesh.cancel_task(coord);

        if let Some(entry) = loaded.entries.remove(&coord) {
            // 清理 GPU 资源后再销毁实体
            meshes.remove(&entry.mesh_handle);
            materials.remove(&entry.material_handle);
            commands.entity(entry.entity).despawn();
        }
    }
}

/// LRU 缓存淘汰：当缓存区块数超过 `MAX_CACHED_CHUNKS` 时，淘汰最久未访问且距离较远的区块。
///
/// 淘汰策略：
/// 1. 只淘汰渲染距离外的区块（近景区块不淘汰）
/// 2. 按 (last_accessed, distance) 排序，最久未访问 + 最远的优先淘汰
/// 3. 每帧最多淘汰 `LRU_UNLOADS_PER_FRAME` 个区块，避免卡顿
///
/// 淘汰前会清理关联的 GPU 资源（mesh + material），避免内存泄漏。
/// 同时取消该区块的异步网格生成任务。
fn lru_evict(
    center: ChunkCoord,
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<VoxelMaterial>,
    loaded: &mut LoadedChunks,
    async_mesh: &AsyncMeshManager,
) {
    if loaded.entries.len() <= MAX_CACHED_CHUNKS {
        return;
    }

    // 收集渲染距离外的区块，按 (last_accessed, distance) 排序
    let mut candidates: Vec<(ChunkCoord, u64, i32)> = loaded
        .entries
        .iter()
        .filter(|(coord, _)| {
            let dx = (coord.cx - center.cx).abs();
            let dz = (coord.cz - center.cz).abs();
            // 只淘汰渲染距离外的区块
            dx > RENDER_DISTANCE || dz > RENDER_DISTANCE
        })
        .map(|(coord, entry)| {
            let dx = (coord.cx - center.cx).abs();
            let dy = (coord.cy - center.cy).abs();
            let dz = (coord.cz - center.cz).abs();
            let dist_sq = dx * dx + dy * dy + dz * dz;
            (*coord, entry.last_accessed, dist_sq)
        })
        .collect();

    // 按 last_accessed 升序（最久未访问优先），距离降序（最远优先）
    candidates.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| b.2.cmp(&a.2)));

    // 淘汰最多 LRU_UNLOADS_PER_FRAME 个区块
    let evict_count = (loaded.entries.len() - MAX_CACHED_CHUNKS)
        .min(LRU_UNLOADS_PER_FRAME)
        .min(candidates.len());

    for i in 0..evict_count {
        let coord = candidates[i].0;

        // 取消异步任务（如果还在处理中）
        async_mesh.cancel_task(coord);

        if let Some(entry) = loaded.entries.remove(&coord) {
            // 清理 GPU 资源后再销毁实体
            meshes.remove(&entry.mesh_handle);
            materials.remove(&entry.material_handle);
            commands.entity(entry.entity).despawn();
        }
    }
}
