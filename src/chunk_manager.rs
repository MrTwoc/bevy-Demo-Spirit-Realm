//! Chunk manager: loads chunks around the player and unloads distant ones.
//!
//! 使用分帧加载队列避免一帧内同步加载大量区块导致卡顿。
//! 区块按与玩家的距离排序，每帧只加载固定数量（`CHUNKS_PER_FRAME`）。
//! 使用LRU（最近最少使用）缓存淘汰机制，优先卸载最久未访问且距离较远的区块。
//!
//! # 异步网格生成（两阶段流水线）
//!
//! 地形生成 + 树木生成（CPU 密集型）和网格生成均已迁移到后台工作线程：
//!
//! ```text
//! ┌──────────────────────────────────────────────────────────────┐
//! │ 主线程（轻量）                         工作线程（CPU 密集）    │
//! │                                                              │
//! │ ① 提交 Prepare 任务 ─────────────────→ ② 地形生成 + 树木生成 │
//! │                                                              │
//! │ ③ 收集 Prepare 结果 ─────────────────→                       │
//! │ ④ 创建 ECS 实体                                              │
//! │ ⑤ 提交 Generate 任务 ───────────────→ ⑥ 网格生成（面剔除）    │
//! │                                                              │
//! │ ⑦ 收集 Mesh 结果 ──────────────────→                        │
//! │ ⑧ GPU 上传                                                  │
//! └──────────────────────────────────────────────────────────────┘
//! ```
//!
//! # 优势
//!
//! - **消除加载尖峰**：地形生成（~50ms）和网格生成均不在主线程，帧时间无长尾。
//! - **两阶段流水线**：Prepare 和 Generate 可并行执行，最大化工作线程利用率。
//! - **分帧控制**：每帧限制 Prepare 提交和 Result 收集数量，避免一帧内过度开销。
//! - **LRU 缓存**：超过 `MAX_CACHED_CHUNKS` 时逐步淘汰远处区块。

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::NoCpuCulling;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::prelude::*;
use bevy::render::render_resource::Extent3d;
use bevy::render::render_resource::TextureDimension;
use bevy::render::render_resource::TextureFormat;
use std::collections::HashMap;
use std::sync::Arc;

use crate::async_mesh::{AsyncMeshManager, MESH_UPLOADS_PER_FRAME, MeshTask};
use crate::chunk::{Chunk, ChunkComponent, ChunkCoord, ChunkNeighbors, WorldTypeResource};
use crate::chunk_changes::{LodChangedFlag, NeighborChangedFlag};
use crate::chunk_dirty::{
    ChunkAtlasHandle, ChunkCoordComponent, ChunkMeshHandle, DirtyChunk, is_air_chunk,
};
use crate::hud::CachedTriangleCount;
use crate::lod::{LodLevel, LodManager};
use crate::resource_pack::{ResourcePackManager, VoxelMaterial};
use crate::tree_gen::{TreeConfig, TreeNoise};

/// 渲染距离（区块数）。增大此值可以看到更远的世界，但需要更多区块加载。
pub const RENDER_DISTANCE: i32 = 16;
/// 游戏启动时的初始加载半径（Voxy 式渐进加载）。
/// 不一次性加载全视距，避免启动时的大量任务积压 最小值(16)。
pub const INITIAL_LOAD_RADIUS: i32 = 16;
/// 探测半径：玩家移动时，只在此半径内扫描新出现的区块并加入加载队列。
/// 设为视距的一半（最多 8 区块），配合 UNLOAD_DISTANCE 实现渐进式加载：
/// 探测范围之外的区块不会被主动发现，但已加载的区块只要在 UNLOAD_DISTANCE 内就持续保留。
pub const DETECTION_RADIUS: i32 = if RENDER_DISTANCE <= 16 {
    16
} else {
    RENDER_DISTANCE / 2
};
/// 卸载距离：超过此距离的区块会被卸载。比渲染距离大 1 避免边界闪烁。
pub const UNLOAD_DISTANCE: i32 = RENDER_DISTANCE + (RENDER_DISTANCE / 4);
/// 每帧最多提交到异步队列的区块数。控制任务提交速率，避免工作线程积压。
pub const CHUNKS_PER_FRAME: usize = 32;
/// 最大缓存区块数。当超过此数量时，使用LRU策略淘汰最久未访问的区块。
pub const MAX_CACHED_CHUNKS: usize = 20000;
/// LRU淘汰时每帧最多卸载的区块数。避免一帧内卸载太多导致卡顿。
pub const LRU_UNLOADS_PER_FRAME: usize = 32;
/// 每帧最多标记邻居为脏的数量。
/// 设为较大值以确保所有新加载区块的邻居都能被正确标记重建。
/// 移除旧版 16 的限制，因为此限制导致超出部分的邻居永久性缺少重建（Bug #1）。
pub const NEIGHBOR_DIRTY_PER_FRAME: usize = 512;
/// 每帧最多处理的删除数量。控制分帧删除速率，避免大量删除操作阻塞主线程。
pub const DELETIONS_PER_FRAME: usize = 16;
/// 每帧分帧加载队列构建最多处理的区块扫描步数。
pub const QUEUE_BUILD_STEPS_PER_FRAME: usize = 500;

/// 已加载区块的条目
pub struct ChunkEntry {
    pub entity: Entity,
    /// `Arc<Chunk>` 避免深拷贝：实体组件和 Entry 共享同一份 ChunkData，
    /// 提交异步任务时仅执行 Arc::clone（引用计数 +1，O(1) 开销）。
    pub data: Arc<Chunk>,
    pub last_accessed: u64,
    /// 固体方块的 Mesh Handle
    pub solid_mesh_handle: Handle<Mesh>,
    pub solid_material_handle: Handle<VoxelMaterial>,
    /// 水方块的 Mesh Handle（仅当区块包含水时存在）
    pub water_mesh_handle: Option<Handle<Mesh>>,
    /// 水 Mesh 的子实体（作为主区块实体的子节点，随父实体生命周期管理）
    pub water_entity: Option<Entity>,
    pub water_triangle_count: u32,
    pub lod_level: LodLevel,
    pub triangle_count: u32,
}

/// 待删除区块的信息
struct PendingDeletion {
    entity: Entity,
    mesh_handle: Handle<Mesh>,
    water_mesh_handle: Option<Handle<Mesh>>,
}

#[derive(Resource)]
pub struct LoadedChunks {
    pub entries: HashMap<ChunkCoord, ChunkEntry>,
    pub entries_ordered: Vec<ChunkCoord>,
    pub load_queue: Vec<ChunkCoord>,
    pub last_player_chunk: Option<ChunkCoord>,
    pub frame_counter: u64,
    pub needs_unload_check: bool,
    pending_deletions: Vec<PendingDeletion>,
    load_queue_build_state: Option<LoadQueueBuildState>,
}

impl Default for LoadedChunks {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            entries_ordered: Vec::new(),
            load_queue: Vec::new(),
            last_player_chunk: None,
            frame_counter: 0,
            needs_unload_check: false,
            pending_deletions: Vec::new(),
            load_queue_build_state: None,
        }
    }
}

/// Y 轴加载半径：玩家上下各加载多少层 Y 区块。
pub const Y_LOAD_RADIUS: i32 = 2;
/// Y 轴卸载半径：超过此距离的 Y 区块会被卸载。比加载半径大 1 避免边界闪烁。
pub const Y_UNLOAD_RADIUS: i32 = Y_LOAD_RADIUS + 1;

/// 存储 Atlas 纹理句柄的资源
#[derive(Resource)]
pub struct AtlasTextureHandle {
    pub handle: Handle<Image>,
}

/// 全局共享的 VoxelMaterial 实例。
#[derive(Resource, Clone)]
pub struct SharedVoxelMaterial {
    pub handle: Handle<VoxelMaterial>,
}

/// 透明VoxelMaterial 实例（水方块使用）。
#[derive(Resource, Clone)]
pub struct TransparentVoxelMaterial {
    pub handle: Handle<VoxelMaterial>,
}

/// 全局共享的空 Mesh Handle，所有零三角形/全空气区块共用同一个 GPU Buffer。
/// 避免为每个空气区块创建一个独立的空 Mesh（浪费 GPU 对象 + Asset 系统扫描开销）。
#[derive(Resource, Clone)]
pub struct SharedEmptyMesh {
    pub handle: Handle<Mesh>,
}

/// 6 个方向的偏移量
const NEIGHBOR_OFFSETS: [(i32, i32, i32); 6] = [
    (1, 0, 0),
    (-1, 0, 0),
    (0, 1, 0),
    (0, -1, 0),
    (0, 0, 1),
    (0, 0, -1),
];

/// 分帧加载队列构建状态
struct LoadQueueBuildState {
    center: ChunkCoord,
    radius: i32,
    dx: i32,
    dz: i32,
    cy: i32,
    cy_min: i32,
    cy_max: i32,
    missing: Vec<ChunkCoord>,
}

impl LoadQueueBuildState {
    fn new(center: ChunkCoord, radius: i32, cy_min: i32, cy_max: i32) -> Self {
        Self {
            center,
            radius,
            dx: -radius,
            dz: -radius,
            cy: cy_min,
            cy_min,
            cy_max,
            missing: Vec::new(),
        }
    }
}

fn collect_neighbors(coord: ChunkCoord, loaded: &LoadedChunks) -> ChunkNeighbors {
    let mut neighbors = ChunkNeighbors::empty();

    for (i, (dx, dy, dz)) in NEIGHBOR_OFFSETS.iter().enumerate() {
        let neighbor_coord = ChunkCoord {
            cx: coord.cx + dx,
            cy: coord.cy + dy,
            cz: coord.cz + dz,
        };

        if let Some(entry) = loaded.entries.get(&neighbor_coord) {
            neighbors.neighbor_data[i] = Some(Arc::clone(&entry.data));
        }
    }

    neighbors
}

/// Startup system: spawns the camera and HUD, then queues initial chunks for loading.
pub fn setup_world(
    mut commands: Commands,
    mut loaded: ResMut<LoadedChunks>,
    resource_pack: Res<ResourcePackManager>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<VoxelMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    tree_config: Res<TreeConfig>,
    tree_noise: Res<TreeNoise>,
) {
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
            TextureFormat::Rgba8Unorm,
            RenderAssetUsages::default(),
        );
        // 使用重复模式采样器，支持纹理平铺
        // U/V 方向重复，W 方向（纹理数组层）夹紧
        bevy_image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
            label: Some("array_texture_sampler".to_string()),
            address_mode_u: ImageAddressMode::Repeat,
            address_mode_v: ImageAddressMode::Repeat,
            address_mode_w: ImageAddressMode::ClampToEdge,
            mag_filter: ImageFilterMode::Nearest,
            min_filter: ImageFilterMode::Nearest,
            mipmap_filter: ImageFilterMode::Nearest,
            ..Default::default()
        });
        images.add(bevy_image)
    } else {
        images.add(Image::default())
    };

    commands.insert_resource(AtlasTextureHandle {
        handle: atlas_handle.clone(),
    });

    let shared_material = materials.add(VoxelMaterial {
        array_texture: atlas_handle.clone(),
        alpha_mode: AlphaMode::Opaque,
    });
    let transparent_material = materials.add(VoxelMaterial {
        array_texture: atlas_handle.clone(),
        alpha_mode: AlphaMode::Blend,
    });
    commands.insert_resource(SharedVoxelMaterial {
        handle: shared_material,
    });
    commands.insert_resource(TransparentVoxelMaterial {
        handle: transparent_material,
    });

    // 创建共享空 Mesh（所有全空气/零三角形区块共用，避免每个区块独立创建一个空 Mesh）
    let empty_mesh = meshes.add(Mesh::new(
        bevy::render::render_resource::PrimitiveTopology::TriangleList,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    ));
    commands.insert_resource(SharedEmptyMesh { handle: empty_mesh });

    let worker_count = crate::async_mesh::default_worker_count();
    let uv_table = crate::async_mesh::UvLookupTable::from_resource_pack(&resource_pack);
    commands.insert_resource(AsyncMeshManager::new(
        worker_count,
        uv_table,
        tree_config.as_ref().clone(),
        tree_noise.as_ref().clone(),
        WorldTypeResource::default().0,
    ));

    // 插入世界类型资源，默认是 Noise（覆盖已存在的旧值也没问题）
    commands.insert_resource(WorldTypeResource::default());

    use crate::camera::CameraController;
    let camera_transform = Transform::from_xyz(16.0, 64.0, 16.0);
    let camera_entity = commands
        .spawn((
            Camera3d::default(),
            camera_transform,
            CameraController::default(),
            // NoCpuCulling,
        ))
        .id();

    crate::hud::setup_hud(&mut commands, camera_entity);
    crate::hud::setup_hardware_info_hud(&mut commands, camera_entity);

    let center = ChunkCoord {
        cx: 0,
        cy: 0,
        cz: 0,
    };
    loaded.last_player_chunk = Some(center);
    if let Some(queue) = rebuild_load_queue(
        center,
        &mut *loaded,
        QUEUE_BUILD_STEPS_PER_FRAME,
        INITIAL_LOAD_RADIUS,
    ) {
        loaded.load_queue = queue;
    }
}

/// 每帧系统：异步网格结果收集 + 分帧任务提交 + 卸载远处区块 + LOD 更新。
pub fn chunk_loader_system(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut loaded: ResMut<LoadedChunks>,
    mut cached: ResMut<CachedTriangleCount>,
    async_mesh: Res<AsyncMeshManager>,
    camera_query: Query<&Transform, With<Camera3d>>,
    atlas_handle: Res<AtlasTextureHandle>,
    shared_material: Res<SharedVoxelMaterial>,
    transparent_material: Res<TransparentVoxelMaterial>,
    shared_empty_mesh: Res<SharedEmptyMesh>,
    mut lod_manager: ResMut<LodManager>,
) {
    let Ok(cam_transform) = camera_query.single() else {
        return;
    };

    let player_chunk = ChunkCoord::from_world(cam_transform.translation);

    loaded.frame_counter += 1;
    let current_frame = loaded.frame_counter;

    // ── 步骤 1：收集异步结果并上传 GPU ──────────────────────────
    // ⚠️ 无论 DirtyChunk 是否存在都应用结果，避免占位空网格持续显示。
    // 如果 DirtyChunk 因 LOD 变更或邻居标记而存在，dirty 系统后续会
    // 提交重建任务覆盖为正确版本。移除脏标记检查防止以下死锁场景：
    //   - 邻居标记添加 DirtyChunk → 结果被跳过 → dirty 提交新任务但
    //     pending_tasks 有该区块返回 false → DirtyChunk 不清理 →
    //     下帧继续跳过 → 区块永久显示空网格
    let results = async_mesh.collect_results(MESH_UPLOADS_PER_FRAME);
    for result in results {
        if !loaded.entries.contains_key(&result.coord) {
            continue;
        }

        // 先提取 entry 中的值，避免跨越 get_mut 借用的生命周期
        let (entity, water_entity, old_handle, old_water_handle, old_tri_count) = {
            let entry = loaded.entries.get(&result.coord).unwrap();
            (
                entry.entity,
                entry.water_entity,
                entry.solid_mesh_handle.clone(),
                entry.water_mesh_handle.clone(),
                entry.triangle_count,
            )
        };

        // ── 将网格数据包装为 Arc，避免后续克隆产生完整 memcpy ──
        // 固体网格数据仅走 Bevy Mesh 管线（Mesh3d + MeshMaterial3d）
        let solid_positions = Arc::new(result.solid.positions);
        let solid_normals = Arc::new(result.solid.normals);
        let solid_uvs = Arc::new(result.solid.uvs);
        let solid_indices = Arc::new(result.solid.indices);

        // 1. 处理固体 Mesh
        let solid_triangle_count = result.solid.triangle_count;
        let new_handle: Handle<Mesh>;

        if solid_triangle_count > 0 {
            // 有可见面 → 原地更新 Mesh 数据（复用现有 Handle，避免 remove + add 的 Asset 系统开销）
            if old_handle != shared_empty_mesh.handle {
                // 已有独立 Handle → 原地更新顶点/索引数据
                if let Some(mesh) = meshes.get_mut(&old_handle) {
                    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, (*solid_positions).clone());
                    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, (*solid_uvs).clone());
                    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, (*solid_normals).clone());
                    mesh.insert_indices(bevy::mesh::Indices::U32((*solid_indices).clone()));
                }
                new_handle = old_handle.clone();
            } else {
                // 当前是共享空 Mesh（占位或零几何体状态）→ 创建独立 Handle
                new_handle = meshes.add(
                    Mesh::new(
                        bevy::render::render_resource::PrimitiveTopology::TriangleList,
                        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
                    )
                    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, (*solid_positions).clone())
                    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, (*solid_uvs).clone())
                    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, (*solid_normals).clone())
                    .with_inserted_indices(bevy::mesh::Indices::U32((*solid_indices).clone())),
                );
            }

            // 更新固体实体
            commands.entity(entity).insert((
                Mesh3d(new_handle.clone()),
                MeshMaterial3d(shared_material.handle.clone()),
                ChunkMeshHandle {
                    mesh: new_handle.clone(),
                    material: shared_material.handle.clone(),
                },
            ));

            // 更新 entry（固体 handle 仅在非共享时更新）
            if let Some(entry) = loaded.entries.get_mut(&result.coord) {
                if old_handle == shared_empty_mesh.handle {
                    entry.solid_mesh_handle = new_handle;
                }
                entry.solid_material_handle = shared_material.handle.clone();
            }
        } else {
            // 三角形数为 0（所有面被遮挡），切换到共享空 Mesh
            if old_handle != shared_empty_mesh.handle {
                // 之前有几何体 → 清理旧 Handle
                meshes.remove(&old_handle);
            }
            let empty_mesh = shared_empty_mesh.handle.clone();

            commands.entity(entity).insert((
                Mesh3d(empty_mesh.clone()),
                MeshMaterial3d(shared_material.handle.clone()),
                ChunkMeshHandle {
                    mesh: empty_mesh.clone(),
                    material: shared_material.handle.clone(),
                },
            ));

            if let Some(entry) = loaded.entries.get_mut(&result.coord) {
                entry.solid_mesh_handle = empty_mesh;
                entry.solid_material_handle = shared_material.handle.clone();
            }
        }

        // 2. 处理水 Mesh（作为主实体的子节点）
        if let Some(water_data) = result.water {
            let water_triangle_count = water_data.triangle_count;

            // 尝试原地更新现有水 Mesh，不存在则创建新 Handle
            let water_mesh_handle = if let Some(handle) = old_water_handle {
                // 复用现有 Handle，原地更新顶点/索引数据
                if let Some(mesh) = meshes.get_mut(&handle) {
                    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, water_data.positions);
                    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, water_data.uvs);
                    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, water_data.normals);
                    mesh.insert_indices(bevy::mesh::Indices::U32(water_data.indices));
                }
                handle
            } else {
                meshes.add(
                    Mesh::new(
                        bevy::render::render_resource::PrimitiveTopology::TriangleList,
                        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
                    )
                    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, water_data.positions)
                    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, water_data.uvs)
                    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, water_data.normals)
                    .with_inserted_indices(bevy::mesh::Indices::U32(water_data.indices)),
                )
            };

            if let Some(we) = water_entity {
                // 更新已有水子实体
                commands.entity(we).insert((
                    Mesh3d(water_mesh_handle.clone()),
                    MeshMaterial3d(transparent_material.handle.clone()),
                    Transform::IDENTITY,
                ));
            } else {
                // 创建新的水子实体（挂载到父区块实体下）
                let water_entity = commands
                    .spawn((
                        Mesh3d(water_mesh_handle.clone()),
                        MeshMaterial3d(transparent_material.handle.clone()),
                        Transform::IDENTITY,
                        Visibility::default(),
                    ))
                    .id();
                commands.entity(entity).add_child(water_entity);

                if let Some(entry) = loaded.entries.get_mut(&result.coord) {
                    entry.water_entity = Some(water_entity);
                }
            }

            if let Some(entry) = loaded.entries.get_mut(&result.coord) {
                entry.water_mesh_handle = Some(water_mesh_handle);
                entry.water_triangle_count = water_triangle_count;
            }
        } else {
            // 区块不再包含水，移除水子实体
            if let Some(we) = water_entity {
                if let Some(water_handle) = old_water_handle {
                    meshes.remove(&water_handle);
                }
                commands.entity(we).despawn();
            }
            if let Some(entry) = loaded.entries.get_mut(&result.coord) {
                entry.water_entity = None;
                entry.water_mesh_handle = None;
                entry.water_triangle_count = 0;
            }
        }

        if let Some(entry) = loaded.entries.get_mut(&result.coord) {
            // ⭐ 增量更新三角形数
            cached.0 = cached
                .0
                .wrapping_add(solid_triangle_count)
                .wrapping_sub(old_tri_count);
            entry.triangle_count = solid_triangle_count;
        }
    }

    // ── 步骤 1.5：分帧删除处理 ────────────────────────────────
    let delete_count = DELETIONS_PER_FRAME.min(loaded.pending_deletions.len());
    let deletions_this_frame = loaded.pending_deletions.drain(..delete_count);
    for deletion in deletions_this_frame {
        // 只移除独立 Mesh Handle（共享空 Handle 由 SharedEmptyMesh Resource 管理，不重复清理）
        if deletion.mesh_handle != shared_empty_mesh.handle {
            meshes.remove(&deletion.mesh_handle);
        }
        // 清理水 Mesh Handle
        if let Some(water_handle) = deletion.water_mesh_handle {
            meshes.remove(&water_handle);
        }
        // despawn 自动清理所有子节点（水实体作为子实体挂载在父实体下）
        commands.entity(deletion.entity).despawn();
    }

    // ── 步骤 2：检测玩家移动，启动/继续分帧加载队列构建 ──────────────────────
    let needs_rebuild =
        loaded.load_queue_build_state.is_some() || loaded.last_player_chunk != Some(player_chunk);

    if needs_rebuild {
        if loaded.load_queue_build_state.is_none() {
            loaded.last_player_chunk = Some(player_chunk);
            loaded.needs_unload_check = true;
        }

        if let Some(built_queue) = rebuild_load_queue(
            player_chunk,
            &mut *loaded,
            QUEUE_BUILD_STEPS_PER_FRAME,
            DETECTION_RADIUS,
        ) {
            loaded.load_queue = built_queue;
            unload_distant_chunks(
                player_chunk,
                &mut *loaded,
                &*async_mesh,
                &mut *lod_manager,
                &mut *cached,
            );
        }
    }

    // ── 步骤 2.5：更新 LOD 管理器 ─────────────────────────────
    let to_rebuild = lod_manager.update_incremental(player_chunk, &*loaded);
    for (coord, new_lod) in to_rebuild {
        if let Some(entry) = loaded.entries.get(&coord) {
            commands
                .entity(entry.entity)
                .insert((DirtyChunk, LodChangedFlag));
            if let Some(entry) = loaded.entries.get_mut(&coord) {
                entry.lod_level = new_lod;
            }
        }
    }

    lru_evict(
        player_chunk,
        &mut *loaded,
        &*async_mesh,
        &mut *lod_manager,
        &mut *cached,
    );

    // ── 步骤 3A：收集准备完成的区块数据，创建实体并提交网格生成任务 ──
    // 将地形+树木生成（Prepare 阶段）的结果转化为 ECS 实体和网格生成任务。
    // 使用 CHUNKS_PER_FRAME * 2 的收集上限以匹配两阶段流水线产出率。
    let prepare_results = async_mesh.collect_prepare_results(CHUNKS_PER_FRAME * 2);
    let mut dirty_neighbors: Vec<Entity> = Vec::new();
    let mut neighbor_dirty_remaining = NEIGHBOR_DIRTY_PER_FRAME;

    for prepare_result in prepare_results {
        let coord = prepare_result.coord;
        let chunk = prepare_result.data;

        // 区块已在之前被加载（例如通过相邻区块的脏重建流程），跳过。
        if loaded.entries.contains_key(&coord) {
            continue;
        }

        // 纯空气区块（理论很少见）：不创建实体，不占用 ECS 资源。
        if is_air_chunk(&chunk) {
            continue;
        }

        let neighbors = collect_neighbors(coord, &*loaded);

        let dist_sq = (coord.cx - player_chunk.cx).pow(2)
            + (coord.cy - player_chunk.cy).pow(2)
            + (coord.cz - player_chunk.cz).pow(2);
        let lod_level = LodLevel::from_chunk_distance_sq(dist_sq);

        // 使用共享空 Mesh 作为占位符（避免每创建一个新区块就增加一个 GPU Buffer 对象）
        // 当异步结果返回后，有几何体的区块会被升级为独立 Handle（meshes.add），
        // 零几何体区块继续共享此 Handle，无需额外 GPU 内存。
        let placeholder_mesh = shared_empty_mesh.handle.clone();
        let placeholder_mat = shared_material.handle.clone();

        // 使用 Arc 包装 ChunkData，实体组件和 Entry 共享同一份数据：
        // - 实体组件：ChunkComponent(Arc::clone(&shared)) → 引用计数 +1，O(1)
        // - Entry.data：shared → 所有权转移，零拷贝
        // - 异步任务：Arc::clone(&entry.data) → 引用计数 +1，O(1)
        // 对比旧代码：chunk.clone()（~32KB）+ entry.data.clone()（~32KB）= 64KB 深拷贝
        let shared = Arc::new(chunk);
        let position = coord.to_world_origin();
        let entity = commands
            .spawn((
                ChunkComponent(Arc::clone(&shared)),
                Transform::from_translation(position),
                Visibility::default(),
                // NoCpuCulling,
                ChunkAtlasHandle(atlas_handle.handle.clone()),
                ChunkCoordComponent(coord),
                Mesh3d(placeholder_mesh.clone()),
                MeshMaterial3d(placeholder_mat.clone()),
                ChunkMeshHandle {
                    mesh: placeholder_mesh.clone(),
                    material: placeholder_mat.clone(),
                },
            ))
            .id();

        loaded.entries.insert(
            coord,
            ChunkEntry {
                entity,
                data: shared,
                last_accessed: current_frame,
                solid_mesh_handle: placeholder_mesh.clone(),
                solid_material_handle: placeholder_mat.clone(),
                water_mesh_handle: None,
                water_entity: None,
                water_triangle_count: 0,
                lod_level,
                triangle_count: 0,
            },
        );

        loaded.entries_ordered.push(coord);

        lod_manager.set_lod(coord, lod_level);

        // 提交网格生成任务（在工作线程中将准备好的区块数据转为 GPU 网格）
        let entry = loaded.entries.get(&coord).unwrap();
        async_mesh.submit_task(MeshTask::Generate {
            coord,
            data: Arc::clone(&entry.data),
            neighbors,
            lod_level: Some(lod_level),
        });

        // 标记相邻区块为脏块，使其网格在邻居变化后被重建
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
                // neighbor_entry.data 是 Arc<Chunk>，通过 as_ref() 获取 &ChunkData
                if is_air_chunk(neighbor_entry.data.as_ref()) {
                    continue;
                }
                dirty_neighbors.push(neighbor_entry.entity);
                neighbor_dirty_remaining -= 1;
            }
        }
    }

    // 批量应用脏标记（附带邻居变更标记）
    for entity in dirty_neighbors {
        commands
            .entity(entity)
            .insert((DirtyChunk, NeighborChangedFlag));
    }

    // ── 步骤 3B：提交区块数据准备任务（地形生成 + 树木生成，在工作线程执行） ──
    // 从加载队列中取出区块坐标，提交 Prepare 任务。
    // 双重 pending 检查（prepare + generate）避免重复提交和竞态。
    let drain_count = CHUNKS_PER_FRAME.min(loaded.load_queue.len());
    let chunks_to_submit: Vec<ChunkCoord> = loaded.load_queue.drain(..drain_count).collect();

    for coord in chunks_to_submit {
        if loaded.entries.contains_key(&coord) {
            continue;
        }
        if async_mesh.is_prepare_pending(&coord) || async_mesh.is_pending(&coord) {
            continue;
        }
        async_mesh.submit_prepare_task(coord);
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// 优化后的分帧系统（替代原 chunk_loader_system 单一入口点）
//
// 设计目标：
// 1. GPU 上传（collect_and_upload_meshes + process_pending_deletions）在 First
//    调度中执行，优先于渲染阶段，减少帧尾延迟。
// 2. CPU 密集型操作（manage_chunk_load_state + spawn_entities_from_prepare
//    + submit_prepare_tasks）在 Update 调度中链式执行，共享加载状态。
// 3. 更少的 ResMut 参数 → 降低各系统与相机/输入等系统的锁竞争。
// 4. frame_counter 仅由 collect_and_upload_meshes 递增一次，后续系统通过
//    loaded.frame_counter 读取，无需重复递增。
// ═══════════════════════════════════════════════════════════════════════════════

// ── run_if 条件函数 ────────────────────────────────────────────
//
// 这些函数作为 run_if 条件使用，使用 Res<T>（只读）访问，
// 当返回 false 时系统完全不运行 — 不获取任何 ResMut 锁，不消耗 CPU。
// 在空闲帧（玩家静止、无新区块加载）中 3 个系统都会被跳过。

/// `run_if` 条件：是否有待删除的区块实体。
///
/// 仅在 `pending_deletions` 非空时运行 `process_pending_deletions`。
pub fn has_pending_deletions(loaded: Res<LoadedChunks>) -> bool {
    !loaded.pending_deletions.is_empty()
}

/// `run_if` 条件：加载队列是否非空。
///
/// 仅在加载队列中有待处理的区块坐标时运行 `submit_prepare_tasks`。
pub fn has_load_queue_items(loaded: Res<LoadedChunks>) -> bool {
    !loaded.load_queue.is_empty()
}

/// `run_if` 条件：工作线程是否有待处理的异步任务。
///
/// 使用 `pending_count()`（Prepare + Generate 合计）作为粗粒度检查：
/// 若两者均为 0，`collect_prepare_results` 必然返回空，系统可安全跳过。
pub fn has_pending_prepare_results(async_mesh: Res<AsyncMeshManager>) -> bool {
    async_mesh.pending_count() > 0
}

// ── 分帧系统入口 ───────────────────────────────────────────────

/// 系统 1：收集异步网格结果并上传 GPU（First 调度，优先于渲染）。
///
/// 原 `chunk_loader_system` 步骤 1：收集工作线程完成的网格数据，
/// 原地更新或创建 Mesh Handle，更新实体组件和 CachedTriangleCount。
pub fn collect_and_upload_meshes(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut loaded: ResMut<LoadedChunks>,
    mut cached: ResMut<CachedTriangleCount>,
    async_mesh: Res<AsyncMeshManager>,
    shared_material: Res<SharedVoxelMaterial>,
    transparent_material: Res<TransparentVoxelMaterial>,
    shared_empty_mesh: Res<SharedEmptyMesh>,
) {
    loaded.frame_counter += 1;

    let results = async_mesh.collect_results(MESH_UPLOADS_PER_FRAME);
    for result in results {
        if !loaded.entries.contains_key(&result.coord) {
            continue;
        }

        let (entity, water_entity, old_handle, old_water_handle, old_tri_count) = {
            let entry = loaded.entries.get(&result.coord).unwrap();
            (
                entry.entity,
                entry.water_entity,
                entry.solid_mesh_handle.clone(),
                entry.water_mesh_handle.clone(),
                entry.triangle_count,
            )
        };

        // Tier 1 零拷贝优化：直接从 SubMeshData 移动数据，消除 Arc 包装和 clone 拷贝
        let solid_positions = result.solid.positions;
        let solid_normals = result.solid.normals;
        let solid_uvs = result.solid.uvs;
        let solid_indices = result.solid.indices;

        let solid_triangle_count = result.solid.triangle_count;
        let new_handle: Handle<Mesh>;

        if solid_triangle_count > 0 {
            if old_handle != shared_empty_mesh.handle {
                if let Some(mesh) = meshes.get_mut(&old_handle) {
                    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, solid_positions);
                    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, solid_uvs);
                    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, solid_normals);
                    mesh.insert_indices(bevy::mesh::Indices::U32(solid_indices));
                    new_handle = old_handle.clone();
                } else {
                    // 安全网：old_handle 无效（如 Handle::default()），创建新 Mesh
                    // 触发场景：place_block 在纯空气区块按需创建实体时使用了无效的占位句柄
                    new_handle = meshes.add(
                        Mesh::new(
                            bevy::render::render_resource::PrimitiveTopology::TriangleList,
                            RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
                        )
                        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, solid_positions)
                        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, solid_uvs)
                        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, solid_normals)
                        .with_inserted_indices(bevy::mesh::Indices::U32(solid_indices)),
                    );
                }
            } else {
                new_handle = meshes.add(
                    Mesh::new(
                        bevy::render::render_resource::PrimitiveTopology::TriangleList,
                        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
                    )
                    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, solid_positions)
                    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, solid_uvs)
                    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, solid_normals)
                    .with_inserted_indices(bevy::mesh::Indices::U32(solid_indices)),
                );
            }

            commands.entity(entity).insert((
                Mesh3d(new_handle.clone()),
                MeshMaterial3d(shared_material.handle.clone()),
                ChunkMeshHandle {
                    mesh: new_handle.clone(),
                    material: shared_material.handle.clone(),
                },
            ));

            if let Some(entry) = loaded.entries.get_mut(&result.coord) {
                if old_handle == shared_empty_mesh.handle {
                    entry.solid_mesh_handle = new_handle;
                }
                entry.solid_material_handle = shared_material.handle.clone();
            }
        } else {
            if old_handle != shared_empty_mesh.handle {
                meshes.remove(&old_handle);
            }
            let empty_mesh = shared_empty_mesh.handle.clone();

            commands.entity(entity).insert((
                Mesh3d(empty_mesh.clone()),
                MeshMaterial3d(shared_material.handle.clone()),
                ChunkMeshHandle {
                    mesh: empty_mesh.clone(),
                    material: shared_material.handle.clone(),
                },
            ));

            if let Some(entry) = loaded.entries.get_mut(&result.coord) {
                entry.solid_mesh_handle = empty_mesh;
                entry.solid_material_handle = shared_material.handle.clone();
            }
        }

        // 处理水 Mesh
        if let Some(water_data) = result.water {
            let water_triangle_count = water_data.triangle_count;

            let water_mesh_handle = if let Some(handle) = old_water_handle {
                if let Some(mesh) = meshes.get_mut(&handle) {
                    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, water_data.positions);
                    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, water_data.uvs);
                    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, water_data.normals);
                    mesh.insert_indices(bevy::mesh::Indices::U32(water_data.indices));
                }
                handle
            } else {
                meshes.add(
                    Mesh::new(
                        bevy::render::render_resource::PrimitiveTopology::TriangleList,
                        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
                    )
                    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, water_data.positions)
                    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, water_data.uvs)
                    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, water_data.normals)
                    .with_inserted_indices(bevy::mesh::Indices::U32(water_data.indices)),
                )
            };

            if let Some(we) = water_entity {
                commands.entity(we).insert((
                    Mesh3d(water_mesh_handle.clone()),
                    MeshMaterial3d(transparent_material.handle.clone()),
                    Transform::IDENTITY,
                ));
            } else {
                let water_entity = commands
                    .spawn((
                        Mesh3d(water_mesh_handle.clone()),
                        MeshMaterial3d(transparent_material.handle.clone()),
                        Transform::IDENTITY,
                        Visibility::default(),
                    ))
                    .id();
                commands.entity(entity).add_child(water_entity);

                if let Some(entry) = loaded.entries.get_mut(&result.coord) {
                    entry.water_entity = Some(water_entity);
                }
            }

            if let Some(entry) = loaded.entries.get_mut(&result.coord) {
                entry.water_mesh_handle = Some(water_mesh_handle);
                entry.water_triangle_count = water_triangle_count;
            }
        } else {
            if let Some(we) = water_entity {
                if let Some(water_handle) = old_water_handle {
                    meshes.remove(&water_handle);
                }
                commands.entity(we).despawn();
            }
            if let Some(entry) = loaded.entries.get_mut(&result.coord) {
                entry.water_entity = None;
                entry.water_mesh_handle = None;
                entry.water_triangle_count = 0;
            }
        }

        if let Some(entry) = loaded.entries.get_mut(&result.coord) {
            cached.0 = cached
                .0
                .wrapping_add(solid_triangle_count)
                .wrapping_sub(old_tri_count);
            entry.triangle_count = solid_triangle_count;
        }
    }
}

/// 系统 2：分帧处理待删除的区块实体（First 调度）。
///
/// 原 `chunk_loader_system` 步骤 1.5：清理要被卸载的区块实体的 Mesh 资源。
pub fn process_pending_deletions(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut loaded: ResMut<LoadedChunks>,
    shared_empty_mesh: Res<SharedEmptyMesh>,
) {
    let delete_count = DELETIONS_PER_FRAME.min(loaded.pending_deletions.len());
    let deletions_this_frame = loaded.pending_deletions.drain(..delete_count);
    for deletion in deletions_this_frame {
        if deletion.mesh_handle != shared_empty_mesh.handle {
            meshes.remove(&deletion.mesh_handle);
        }
        if let Some(water_handle) = deletion.water_mesh_handle {
            meshes.remove(&water_handle);
        }
        commands.entity(deletion.entity).despawn();
    }
}

/// 系统 3：区块生命周期管理（Update 调度）。
///
/// 原 `chunk_loader_system` 步骤 2 + 2.5 + 2.6：
/// 检测玩家移动 → 重建加载队列 → 卸载远处区块 → 更新 LOD → LRU 淘汰。
pub fn manage_chunk_load_state(
    mut commands: Commands,
    mut loaded: ResMut<LoadedChunks>,
    async_mesh: Res<AsyncMeshManager>,
    mut lod_manager: ResMut<LodManager>,
    mut cached: ResMut<CachedTriangleCount>,
    camera_query: Query<&Transform, With<Camera3d>>,
) {
    let Ok(cam_transform) = camera_query.single() else {
        return;
    };
    let player_chunk = ChunkCoord::from_world(cam_transform.translation);

    // ── 步骤 2：检测玩家移动，启动/继续分帧加载队列构建 ──────
    let needs_rebuild = loaded.load_queue_build_state.is_some()
        || loaded.last_player_chunk != Some(player_chunk);

    if needs_rebuild {
        if loaded.load_queue_build_state.is_none() {
            loaded.last_player_chunk = Some(player_chunk);
            loaded.needs_unload_check = true;
        }

        if let Some(built_queue) = rebuild_load_queue(
            player_chunk,
            &mut *loaded,
            QUEUE_BUILD_STEPS_PER_FRAME,
            DETECTION_RADIUS,
        ) {
            loaded.load_queue = built_queue;
            unload_distant_chunks(
                player_chunk,
                &mut *loaded,
                &*async_mesh,
                &mut *lod_manager,
                &mut *cached,
            );
        }
    }

    // ── 步骤 2.5：更新 LOD 管理器 ─────────────────────────
    let to_rebuild = lod_manager.update_incremental(player_chunk, &*loaded);
    for (coord, new_lod) in to_rebuild {
        if let Some(entry) = loaded.entries.get(&coord) {
            commands
                .entity(entry.entity)
                .insert((DirtyChunk, LodChangedFlag));
            if let Some(entry) = loaded.entries.get_mut(&coord) {
                entry.lod_level = new_lod;
            }
        }
    }

    // ── 步骤 2.6：LRU 缓存淘汰 ───────────────────────────
    lru_evict(
        player_chunk,
        &mut *loaded,
        &*async_mesh,
        &mut *lod_manager,
        &mut *cached,
    );
}

/// 系统 4：处理异步准备好的区块数据并创建实体（Update 调度）。
///
/// 原 `chunk_loader_system` 步骤 3A：
/// 收集工作线程完成的地形数据 → 创建 ECS 实体 → 提交网格生成任务 → 标记邻居脏块。
pub fn spawn_entities_from_prepare(
    mut commands: Commands,
    mut loaded: ResMut<LoadedChunks>,
    async_mesh: Res<AsyncMeshManager>,
    atlas_handle: Res<AtlasTextureHandle>,
    shared_material: Res<SharedVoxelMaterial>,
    shared_empty_mesh: Res<SharedEmptyMesh>,
    mut lod_manager: ResMut<LodManager>,
) {
    // 使用上一系统（manage_chunk_load_state）设置的 last_player_chunk
    // 和 collect_and_upload_meshes 递增的 frame_counter
    let player_chunk = loaded
        .last_player_chunk
        .unwrap_or(ChunkCoord { cx: 0, cy: 0, cz: 0 });
    let current_frame = loaded.frame_counter;

    let prepare_results = async_mesh.collect_prepare_results(CHUNKS_PER_FRAME * 2);
    let mut dirty_neighbors: Vec<Entity> = Vec::new();
    let mut neighbor_dirty_remaining = NEIGHBOR_DIRTY_PER_FRAME;

    for prepare_result in prepare_results {
        let coord = prepare_result.coord;
        let chunk = prepare_result.data;

        if loaded.entries.contains_key(&coord) {
            continue;
        }

        if is_air_chunk(&chunk) {
            continue;
        }

        let neighbors = collect_neighbors(coord, &*loaded);

        let dist_sq = (coord.cx - player_chunk.cx).pow(2)
            + (coord.cy - player_chunk.cy).pow(2)
            + (coord.cz - player_chunk.cz).pow(2);
        let lod_level = LodLevel::from_chunk_distance_sq(dist_sq);

        let placeholder_mesh = shared_empty_mesh.handle.clone();
        let placeholder_mat = shared_material.handle.clone();

        let shared = Arc::new(chunk);
        let position = coord.to_world_origin();
        let entity = commands
            .spawn((
                ChunkComponent(Arc::clone(&shared)),
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
            ))
            .id();

        loaded.entries.insert(
            coord,
            ChunkEntry {
                entity,
                data: shared,
                last_accessed: current_frame,
                solid_mesh_handle: placeholder_mesh.clone(),
                solid_material_handle: placeholder_mat.clone(),
                water_mesh_handle: None,
                water_entity: None,
                water_triangle_count: 0,
                lod_level,
                triangle_count: 0,
            },
        );

        loaded.entries_ordered.push(coord);

        lod_manager.set_lod(coord, lod_level);

        let entry = loaded.entries.get(&coord).unwrap();
        async_mesh.submit_task(MeshTask::Generate {
            coord,
            data: Arc::clone(&entry.data),
            neighbors,
            lod_level: Some(lod_level),
        });

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
                if is_air_chunk(neighbor_entry.data.as_ref()) {
                    continue;
                }
                dirty_neighbors.push(neighbor_entry.entity);
                neighbor_dirty_remaining -= 1;
            }
        }
    }

    for entity in dirty_neighbors {
        commands
            .entity(entity)
            .insert((DirtyChunk, NeighborChangedFlag));
    }
}

/// 系统 5：从加载队列中提交新的 Prepare 任务（Update 调度）。
///
/// 原 `chunk_loader_system` 步骤 3B：
/// 将加载队列中的区块坐标提交到工作线程进行地形+树木生成。
pub fn submit_prepare_tasks(
    mut loaded: ResMut<LoadedChunks>,
    async_mesh: Res<AsyncMeshManager>,
) {
    let drain_count = CHUNKS_PER_FRAME.min(loaded.load_queue.len());
    let chunks_to_submit: Vec<ChunkCoord> = loaded.load_queue.drain(..drain_count).collect();

    for coord in chunks_to_submit {
        if loaded.entries.contains_key(&coord) {
            continue;
        }
        if async_mesh.is_prepare_pending(&coord) || async_mesh.is_pending(&coord) {
            continue;
        }
        async_mesh.submit_prepare_task(coord);
    }
}

/// 重建加载队列（分帧版本）
fn rebuild_load_queue(
    center: ChunkCoord,
    loaded: &mut LoadedChunks,
    steps_limit: usize,
    radius: i32,
) -> Option<Vec<ChunkCoord>> {
    if let Some(ref state) = loaded.load_queue_build_state {
        if state.center != center || state.radius != radius {
            loaded.load_queue_build_state = None;
        }
    }

    if loaded.load_queue_build_state.is_none() {
        let cy_min = center.cy - Y_LOAD_RADIUS;
        let cy_max = center.cy + Y_LOAD_RADIUS;
        loaded.load_queue_build_state =
            Some(LoadQueueBuildState::new(center, radius, cy_min, cy_max));
    }

    let state = loaded
        .load_queue_build_state
        .as_mut()
        .expect("guaranteed by logic above");

    let cy_min = center.cy - Y_LOAD_RADIUS;
    let cy_max = center.cy + Y_LOAD_RADIUS;

    let mut steps_done = 0;

    while steps_done < steps_limit {
        if state.dx * state.dx + state.dz * state.dz > state.radius * state.radius {
            state.dz += 1;
            if state.dz > state.radius {
                state.dz = -state.radius;
                state.dx += 1;
            }
            if state.dx > state.radius {
                break;
            }
            continue;
        }

        let coord = ChunkCoord {
            cx: center.cx + state.dx,
            cy: state.cy,
            cz: center.cz + state.dz,
        };

        if !loaded.entries.contains_key(&coord) {
            state.missing.push(coord);
        }

        state.cy += 1;
        if state.cy > cy_max {
            state.cy = cy_min;
            state.dz += 1;
            if state.dz > state.radius {
                state.dz = -state.radius;
                state.dx += 1;
            }
        }

        steps_done += 1;

        if state.dx > state.radius {
            break;
        }
    }

    if state.dx > state.radius {
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
        None
    }
}

/// 卸载超出加载范围的区块实体
fn unload_distant_chunks(
    center: ChunkCoord,
    loaded: &mut LoadedChunks,
    async_mesh: &AsyncMeshManager,
    lod_manager: &mut LodManager,
    cached: &mut CachedTriangleCount,
) {
    // 仅在玩家移动后才执行全量扫描，静止时跳过
    if !loaded.needs_unload_check {
        return;
    }
    loaded.needs_unload_check = false;

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
        async_mesh.cancel_task(coord);
        lod_manager.remove(&coord);

        if let Some(entry) = loaded.entries.remove(&coord) {
            cached.0 = cached.0.wrapping_sub(entry.triangle_count);
            cached.0 = cached.0.wrapping_sub(entry.water_triangle_count);
            loaded.pending_deletions.push(PendingDeletion {
                entity: entry.entity,
                mesh_handle: entry.solid_mesh_handle,
                water_mesh_handle: entry.water_mesh_handle,
            });
        }
    }

    // 批量同步 entries_ordered：O(N) retain，替代 O(N²) 的逐条 position() + remove
    loaded
        .entries_ordered
        .retain(|c| loaded.entries.contains_key(c));
}

/// LRU 缓存淘汰
fn lru_evict(
    center: ChunkCoord,
    loaded: &mut LoadedChunks,
    async_mesh: &AsyncMeshManager,
    lod_manager: &mut LodManager,
    cached: &mut CachedTriangleCount,
) {
    if loaded.entries.len() <= MAX_CACHED_CHUNKS {
        return;
    }

    let mut candidates: Vec<(ChunkCoord, u64, i32)> = loaded
        .entries
        .iter()
        .filter(|(coord, _)| {
            let dx = (coord.cx - center.cx).abs();
            let dz = (coord.cz - center.cz).abs();
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

    let evict_count = (loaded.entries.len() - MAX_CACHED_CHUNKS)
        .min(LRU_UNLOADS_PER_FRAME)
        .min(candidates.len());

    if evict_count == 0 {
        return;
    }

    // 改用 select_nth_unstable_by 选出前 evict_count 个项（O(N)），再局部排序
    candidates.select_nth_unstable_by(evict_count - 1, |a, b| {
        a.1.cmp(&b.1).then_with(|| b.2.cmp(&a.2))
    });
    candidates[..evict_count].sort_by(|a, b| a.1.cmp(&b.1).then_with(|| b.2.cmp(&a.2)));

    for i in 0..evict_count {
        let coord = candidates[i].0;

        async_mesh.cancel_task(coord);
        lod_manager.remove(&coord);

        if let Some(entry) = loaded.entries.remove(&coord) {
            cached.0 = cached.0.wrapping_sub(entry.triangle_count);
            cached.0 = cached.0.wrapping_sub(entry.water_triangle_count);
            loaded.pending_deletions.push(PendingDeletion {
                entity: entry.entity,
                mesh_handle: entry.solid_mesh_handle,
                water_mesh_handle: entry.water_mesh_handle,
            });
        }
        // 同步从 entries_ordered 中删除
        if let Some(pos) = loaded.entries_ordered.iter().position(|c| *c == coord) {
            loaded.entries_ordered.swap_remove(pos);
        }
    }
}
