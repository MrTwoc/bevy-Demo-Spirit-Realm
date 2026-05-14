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
use bevy::camera::visibility::NoCpuCulling;
use bevy::image::ImageSampler;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use std::collections::HashMap;

use crate::async_mesh::{AsyncMeshManager, MESH_UPLOADS_PER_FRAME, MeshTask};
use crate::chunk::{Chunk, ChunkCoord, ChunkNeighbors, fill_terrain};
use crate::chunk_dirty::{
    ChunkAtlasHandle, ChunkCoordComponent, ChunkMeshHandle, DirtyChunk, is_air_chunk,
};
use crate::lod::{LodLevel, LodManager};
use crate::resource_pack::{ResourcePackManager, VoxelMaterial};

/// 渲染距离（区块数）。增大此值可以看到更远的世界，但需要更多区块加载。
pub const RENDER_DISTANCE: i32 = 32;
/// 卸载距离：超过此距离的区块会被卸载。比渲染距离大 1 避免边界闪烁。
pub const UNLOAD_DISTANCE: i32 = RENDER_DISTANCE + 1;
/// 每帧最多提交到异步队列的区块数。控制任务提交速率，避免工作线程积压。
pub const CHUNKS_PER_FRAME: usize = 32;
/// 最大缓存区块数。当超过此数量时，使用LRU策略淘汰最久未访问的区块。
/// 默认值：渲染距离内约 8*8*π*9 ≈ 1800 个区块，设置为 2000 留有余量。
pub const MAX_CACHED_CHUNKS: usize = 2000;
/// LRU淘汰时每帧最多卸载的区块数。避免一帧内卸载太多导致卡顿。
pub const LRU_UNLOADS_PER_FRAME: usize = 32;
/// 每帧最多标记邻居为脏的数量。限制脏标记速率，避免级联重建风暴。
/// 移动时每个新区块会标记最多6个邻居，限制数量可以避免每帧大量重建。
pub const NEIGHBOR_DIRTY_PER_FRAME: usize = 16;
/// 每帧最多处理的删除数量。控制分帧删除速率，避免大量删除操作阻塞主线程。
/// 当需要卸载大量区块时，删除操作会分散到多帧执行。
pub const DELETIONS_PER_FRAME: usize = 16;

/// 每帧分帧加载队列构建最多处理的区块扫描步数。
/// 控制 `rebuild_load_queue` 分帧构建的速率，避免一次遍历太多区块导致卡顿。
/// 每个扫描步处理一个 (dx, dz, cy) 组合，约 ~8000 步覆盖整个渲染范围。
pub const QUEUE_BUILD_STEPS_PER_FRAME: usize = 500;

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
    /// 当前 LOD 级别
    pub lod_level: LodLevel,
}

/// 待删除区块的信息，用于分帧删除策略。
///
/// 当区块需要卸载时，先将信息放入此队列，每帧处理固定数量，
/// 避免大量删除操作集中在单帧造成卡顿。
struct PendingDeletion {
    entity: Entity,
    mesh_handle: Handle<Mesh>,
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
    /// 分帧删除队列：待删除的区块信息
    pending_deletions: Vec<PendingDeletion>,
    /// 分帧加载队列构建状态
    load_queue_build_state: Option<LoadQueueBuildState>,
}

impl Default for LoadedChunks {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            load_queue: Vec::new(),
            last_player_chunk: None,
            frame_counter: 0,
            pending_deletions: Vec::new(),
            load_queue_build_state: None,
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

/// 全局共享的 VoxelMaterial 实例。
///
/// 所有区块共享同一个材质实例，Bevy 可以自动合批（MultiDrawIndirect），
/// 大幅减少 Draw Call 数量（从 ~2000 降低到数十个）。
///
/// 注意：由于共享材质，卸载区块时不应调用 `materials.remove()` 移除共享材质。
#[derive(Resource, Clone)]
pub struct SharedVoxelMaterial {
    pub handle: Handle<VoxelMaterial>,
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

/// 分帧加载队列构建状态。
///
/// 当玩家移动到新区块时，`rebuild_load_queue` 需要遍历整个渲染范围内的所有区块。
/// 为了避免一帧内同步遍历太多区块导致卡顿，将遍历过程分帧执行。
///
/// # 字段说明
/// - `center`: 玩家当前位置（区块坐标）
/// - `dx`, `dz`, `cy`: 当前扫描位置
/// - `missing`: 已收集到的待加载区块坐标
struct LoadQueueBuildState {
    /// 玩家当前位置（区块坐标）
    center: ChunkCoord,
    /// 当前扫描的 X 偏移
    dx: i32,
    /// 当前扫描的 Z 偏移
    dz: i32,
    /// 当前扫描的 Y 层
    cy: i32,
    /// 已收集到的待加载区块坐标
    missing: Vec<ChunkCoord>,
}

impl LoadQueueBuildState {
    /// 创建新的分帧构建状态
    fn new(center: ChunkCoord, cy_min: i32, cy_max: i32) -> Self {
        Self {
            center,
            dx: -RENDER_DISTANCE,
            dz: -RENDER_DISTANCE,
            cy: cy_min,
            missing: Vec::new(),
        }
    }
}
///
/// 使用 `to_shared_vec()` 返回 `Arc<Vec<BlockId>>`，避免每个邻居
/// 独立分配 32KB 堆内存。多个区块引用同一邻居时共享同一份 `Arc` 数据。
fn collect_neighbors(coord: ChunkCoord, loaded: &LoadedChunks) -> ChunkNeighbors {
    let mut neighbors = ChunkNeighbors::empty();

    for (i, (dx, dy, dz)) in NEIGHBOR_OFFSETS.iter().enumerate() {
        let neighbor_coord = ChunkCoord {
            cx: coord.cx + dx,
            cy: coord.cy + dy,
            cz: coord.cz + dz,
        };

        if let Some(entry) = loaded.entries.get(&neighbor_coord) {
            neighbors.neighbor_data[i] = Some(entry.data.to_shared_vec());
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
    mut materials: ResMut<Assets<VoxelMaterial>>,
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

    // 创建全局共享材质实例（所有区块共享同一个材质，Bevy 可自动合批）
    let shared_material = materials.add(VoxelMaterial {
        array_texture: atlas_handle.clone(),
    });
    commands.insert_resource(SharedVoxelMaterial {
        handle: shared_material,
    });

    // 初始化异步网格管理器（UV 查找表内置于管理器中，通过 Arc 共享给所有工作线程）
    let worker_count = crate::async_mesh::default_worker_count();
    let uv_table = crate::async_mesh::UvLookupTable::from_resource_pack(&resource_pack);
    commands.insert_resource(AsyncMeshManager::new(worker_count, uv_table));

    use crate::camera::CameraController;
    // 摄像机初始位置在地形上方（地形基准高度 16 + 振幅 32 = 最高约 48，留余量）
    let camera_transform = Transform::from_xyz(16.0, 64.0, 16.0);
    let camera_entity = commands
        .spawn((
            Camera3d::default(),
            camera_transform,
            CameraController::default(),
            NoCpuCulling, // 禁用 CPU 视锥体剔除（让 GPU 做）
        ))
        .id();

    crate::hud::setup_hud(&mut commands, camera_entity);
    crate::hud::setup_hardware_info_hud(&mut commands, camera_entity);

    // 将初始区块加入加载队列（不立即加载，由 chunk_loader_system 分帧处理）
    let center = ChunkCoord {
        cx: 0,
        cy: 0,
        cz: 0,
    };
    loaded.last_player_chunk = Some(center);
    rebuild_load_queue(center, &mut *loaded, QUEUE_BUILD_STEPS_PER_FRAME);
}

/// 每帧系统：异步网格结果收集 + 分帧任务提交 + 卸载远处区块 + LOD 更新。
///
/// 工作流程（Phase 1 LOD 系统）：
/// 1. 收集异步网格生成结果，上传到 GPU（每帧最多 `MESH_UPLOADS_PER_FRAME` 个）
/// 2. 处理分帧删除队列（每帧最多 `DELETIONS_PER_FRAME` 个），避免大量删除阻塞主线程
/// 3. 检测玩家是否移动到新区块，若是则：
///    - 重建加载队列
///    - 卸载超出范围的区块（放入 pending_deletions 队列）
///    - 更新 LOD 管理器的区块 LOD 级别
/// 4. 从队列头部取出最多 `CHUNKS_PER_FRAME` 个区块，生成地形数据并提交异步任务（携带 LOD 级别）
/// 5. LRU 淘汰超出缓存上限的区块（放入 pending_deletions 队列）
pub fn chunk_loader_system(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut loaded: ResMut<LoadedChunks>,
    async_mesh: Res<AsyncMeshManager>,
    camera_query: Query<&Transform, With<Camera3d>>,
    atlas_handle: Res<AtlasTextureHandle>,
    shared_material: Res<SharedVoxelMaterial>,
    dirty_query: Query<&DirtyChunk>,
    mut lod_manager: ResMut<LodManager>,
) {
    let Ok(cam_transform) = camera_query.single() else {
        return;
    };

    let player_chunk = ChunkCoord::from_world(cam_transform.translation);

    // 递增帧计数器
    loaded.frame_counter += 1;
    let current_frame = loaded.frame_counter;

    // // ── 调试：每 300 帧输出一次 LOD 分布统计（已验证，注释掉）─────
    // if current_frame % 300 == 0 {
    //     let mut lod_counts = [0u32; 4]; // LOD0, LOD1, LOD2, LOD3
    //     for (_, entry) in &loaded.entries {
    //         lod_counts[entry.lod_level as usize] += 1;
    //     }
    //     let total = loaded.entries.len();
    //     eprintln!(
    //         "[LOD Stats] frame={} total_chunks={} LOD0={} LOD1={} LOD2={} LOD3={}",
    //         current_frame, total, lod_counts[0], lod_counts[1], lod_counts[2], lod_counts[3]
    //     );
    // }

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

            // 移除旧的 mesh 资源（材质使用全局共享实例，不单独移除）
            meshes.remove(&entry.mesh_handle);

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

            // 使用全局共享材质实例（Bevy 可自动合批，减少 Draw Call）
            let mat_handle = shared_material.handle.clone();

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

    // ── 步骤 1.5：分帧删除处理 ────────────────────────────────
    // 每帧只处理固定数量的删除操作，避免大量删除集中在单帧造成卡顿
    // 删除操作包括：meshes.remove() 和 commands.entity().despawn()
    let delete_count = DELETIONS_PER_FRAME.min(loaded.pending_deletions.len());
    let deletions_this_frame = loaded.pending_deletions.drain(..delete_count);
    for deletion in deletions_this_frame {
        // 清理 mesh 资源（材质使用全局共享实例，不单独移除）
        meshes.remove(&deletion.mesh_handle);
        // 延迟删除实体（Bevy Commands 延迟执行）
        commands.entity(deletion.entity).despawn();
    }

    // ── 步骤 2：检测玩家移动，启动/继续分帧加载队列构建 ──────────────────────
    // 如果 load_queue_build_state.is_some()，说明正在构建中，继续分帧处理
    // 如果 last_player_chunk != player_chunk，说明需要启动新的构建
    let needs_rebuild =
        loaded.load_queue_build_state.is_some() || loaded.last_player_chunk != Some(player_chunk);

    if needs_rebuild {
        // 更新 last_player_chunk（只在启动新构建时需要）
        if loaded.load_queue_build_state.is_none() {
            loaded.last_player_chunk = Some(player_chunk);
        }

        // 分帧构建加载队列
        if let Some(built_queue) =
            rebuild_load_queue(player_chunk, &mut *loaded, QUEUE_BUILD_STEPS_PER_FRAME)
        {
            // 构建完成，替换 load_queue
            loaded.load_queue = built_queue;
            // 启动异步卸载
            unload_distant_chunks(player_chunk, &mut *loaded, &*async_mesh, &mut *lod_manager);
        }
    }

    // ── 步骤 2.5：更新 LOD 管理器 ─────────────────────────────
    // 检测需要切换 LOD 的区块，标记为脏以便重建
    let to_rebuild = lod_manager.update(player_chunk, &*loaded);
    for (coord, new_lod) in to_rebuild {
        if let Some(entry) = loaded.entries.get(&coord) {
            commands.entity(entry.entity).insert(DirtyChunk);
            // 更新 ChunkEntry 中的 lod_level
            if let Some(entry) = loaded.entries.get_mut(&coord) {
                entry.lod_level = new_lod;
            }
        }
    }

    // LRU 淘汰：当缓存区块数超过上限时，淘汰最久未访问且距离较远的区块
    lru_evict(player_chunk, &mut *loaded, &*async_mesh, &mut *lod_manager);

    // ── 步骤 3：分帧提交异步任务 ────────────────────────────────
    let drain_count = CHUNKS_PER_FRAME.min(loaded.load_queue.len());
    let chunks_to_load: Vec<ChunkCoord> = loaded.load_queue.drain(..drain_count).collect();

    // 每帧允许标记邻居脏的最大数量（限制级联重建风暴）
    let mut neighbor_dirty_remaining = NEIGHBOR_DIRTY_PER_FRAME;

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

        // 计算该区块的 LOD 级别（基于与玩家的距离）
        let dist = (((coord.cx - player_chunk.cx).pow(2)
            + (coord.cy - player_chunk.cy).pow(2)
            + (coord.cz - player_chunk.cz).pow(2)) as f32)
            .sqrt();
        let lod_level = LodLevel::from_chunk_distance(dist);

        // 创建占位 Mesh（材质使用全局共享实例）
        let placeholder_mesh = meshes.add(Mesh::new(
            bevy::render::render_resource::PrimitiveTopology::TriangleList,
            RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
        ));
        let placeholder_mat = shared_material.handle.clone();

        // 先将区块数据注册到 LoadedChunks（chunk 在此处被 move 进去，避免后续克隆）
        let entity = commands.spawn_empty().id();
        loaded.entries.insert(
            coord,
            ChunkEntry {
                entity,
                data: chunk,
                last_accessed: current_frame,
                mesh_handle: placeholder_mesh.clone(),
                material_handle: placeholder_mat.clone(),
                lod_level,
            },
        );

        // 从 entries 中取出引用，补充实体组件（避免 clone ChunkData）
        let entry = loaded.entries.get(&coord).unwrap();
        let position = coord.to_world_origin();
        commands.entity(entity).insert((
            entry.data.clone(), // ECS 组件需要一份副本，但只克隆这一次
            Transform::from_translation(position),
            Visibility::default(),
            ChunkAtlasHandle(atlas_handle.handle.clone()),
            ChunkCoordComponent(coord),
            Mesh3d(placeholder_mesh.clone()),
            MeshMaterial3d(placeholder_mat.clone()),
            ChunkMeshHandle {
                mesh: placeholder_mesh.clone(),
                material: placeholder_mat.clone(),
            },
        ));

        // 同时注册到 LodManager
        lod_manager.set_lod(coord, lod_level);

        // 提交异步网格生成任务（携带 LOD 级别）
        // 从 entries 中取出 Arc 共享的邻居数据引用
        let entry = loaded.entries.get(&coord).unwrap();
        async_mesh.submit_task(MeshTask::Generate {
            coord,
            data: entry.data.clone(), // 只克隆这一次
            neighbors,
            lod_level: Some(lod_level),
        });

        // 标记邻居区块为脏，使其重新生成网格以正确剔除与新区块的接触面。
        // 当区块A先加载时，其边界面上的方块面被保留（因为邻居还未加载）。
        // 后来区块B加载后，区块A需要重新生成网格才能剔除接触面。
        // 限制每帧脏标记数量，避免级联重建风暴。
        for (dx, dy, dz) in NEIGHBOR_OFFSETS.iter() {
            if neighbor_dirty_remaining == 0 {
                break;
            }
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
                neighbor_dirty_remaining -= 1;
            }
        }
    }
}

/// 重建加载队列（分帧版本）：收集渲染距离内所有未加载的区块坐标，按与玩家的距离排序。
///
/// 为了避免一帧内同步遍历太多区块导致卡顿，将遍历过程分帧执行。
/// 每次调用处理最多 `QUEUE_BUILD_STEPS_PER_FRAME` 步扫描。
///
/// # 返回
/// - `Some`: 队列构建完成，`Vec<ChunkCoord>` 是排序后的待加载区块列表
/// - `None`: 队列构建未完成，需要继续调用
///
/// # 状态管理
/// 通过 `loaded.load_queue_build_state` 跟踪分帧构建进度。
fn rebuild_load_queue(
    center: ChunkCoord,
    loaded: &mut LoadedChunks,
    steps_limit: usize,
) -> Option<Vec<ChunkCoord>> {
    // 检查是否有正在进行的构建
    if let Some(ref state) = loaded.load_queue_build_state {
        // 如果中心点变了，重置构建状态
        if state.center != center {
            loaded.load_queue_build_state = None;
        }
    }

    // 如果没有正在进行的构建，创建新的构建状态
    if loaded.load_queue_build_state.is_none() {
        let cy_min = center.cy - Y_LOAD_RADIUS;
        let cy_max = center.cy + Y_LOAD_RADIUS;
        loaded.load_queue_build_state = Some(LoadQueueBuildState::new(center, cy_min, cy_max));
    }

    let state = loaded
        .load_queue_build_state
        .as_mut()
        .expect("guaranteed by logic above");

    let cy_min = center.cy - Y_LOAD_RADIUS;
    let cy_max = center.cy + Y_LOAD_RADIUS;

    let mut steps_done = 0;

    // 继续上次的扫描
    while steps_done < steps_limit {
        // 圆形裁剪：只加载圆形范围内的区块（而非方形），减少角落浪费
        if state.dx * state.dx + state.dz * state.dz > RENDER_DISTANCE * RENDER_DISTANCE {
            // 这个位置在圆形外，跳过，处理下一个位置
            state.dz += 1;
            if state.dz > RENDER_DISTANCE {
                state.dz = -RENDER_DISTANCE;
                state.dx += 1;
            }
            if state.dx > RENDER_DISTANCE {
                // 扫描完成
                break;
            }
            continue;
        }

        // 处理当前 (dx, dz, cy) 位置
        let coord = ChunkCoord {
            cx: center.cx + state.dx,
            cy: state.cy,
            cz: center.cz + state.dz,
        };

        if !loaded.entries.contains_key(&coord) {
            state.missing.push(coord);
        }

        // 推进到下一个位置
        state.cy += 1;
        if state.cy > cy_max {
            state.cy = cy_min;
            state.dz += 1;
            if state.dz > RENDER_DISTANCE {
                state.dz = -RENDER_DISTANCE;
                state.dx += 1;
            }
        }

        steps_done += 1;

        // 检查是否完成
        if state.dx > RENDER_DISTANCE {
            break;
        }
    }

    // 检查是否完成
    if state.dx > RENDER_DISTANCE {
        // 构建完成，排序并返回
        let cy_min = center.cy - Y_LOAD_RADIUS;
        let cy_max = center.cy + Y_LOAD_RADIUS;

        state.missing.sort_by_key(|coord| {
            let dx = (coord.cx - center.cx).abs();
            let dy = (coord.cy - center.cy).abs();
            let dz = (coord.cz - center.cz).abs();
            dx * dx + dy * dy + dz * dz
        });

        let result = Some(std::mem::take(&mut state.missing));
        loaded.load_queue_build_state = None;
        result
    } else {
        // 构建未完成
        None
    }
}

/// 卸载超出加载范围的区块实体。
///
/// XZ 方向：超出 `UNLOAD_DISTANCE` 的区块卸载。
/// Y 方向：超出 `Y_UNLOAD_RADIUS` 的区块卸载。
///
/// 卸载操作会被分帧执行：区块信息先加入 `pending_deletions` 队列，
/// 每帧通过 `DELETIONS_PER_FRAME` 限制删除数量，避免大量操作阻塞主线程。
/// 同时取消该区块的异步网格生成任务，并从 LodManager 中移除记录。
fn unload_distant_chunks(
    center: ChunkCoord,
    loaded: &mut LoadedChunks,
    async_mesh: &AsyncMeshManager,
    lod_manager: &mut LodManager,
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

        // 从 LodManager 中移除记录
        lod_manager.remove(&coord);

        // 将删除信息加入 pending_deletions 队列，等待分帧删除
        if let Some(entry) = loaded.entries.remove(&coord) {
            loaded.pending_deletions.push(PendingDeletion {
                entity: entry.entity,
                mesh_handle: entry.mesh_handle,
            });
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
/// 淘汰操作会被分帧执行：区块信息先加入 `pending_deletions` 队列，
/// 每帧通过 `DELETIONS_PER_FRAME` 限制删除数量，避免大量删除操作阻塞主线程。
/// 同时取消该区块的异步网格生成任务，并从 LodManager 中移除记录。
fn lru_evict(
    center: ChunkCoord,
    loaded: &mut LoadedChunks,
    async_mesh: &AsyncMeshManager,
    lod_manager: &mut LodManager,
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

        // 从 LodManager 中移除记录
        lod_manager.remove(&coord);

        // 将删除信息加入 pending_deletions 队列，等待分帧删除
        if let Some(entry) = loaded.entries.remove(&coord) {
            loaded.pending_deletions.push(PendingDeletion {
                entity: entry.entity,
                mesh_handle: entry.mesh_handle,
            });
        }
    }
}
