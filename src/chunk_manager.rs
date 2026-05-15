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
use crate::tree_generator::{TreeConfig, TreeGenerator, TreeWriteRequest, generate_trees_in_chunk};

/// 渲染距离（区块数）。增大此值可以看到更远的世界，但需要更多区块加载。
pub const RENDER_DISTANCE: i32 = 16;
/// 卸载距离：超过此距离的区块会被卸载。比渲染距离大 1 避免边界闪烁。
pub const UNLOAD_DISTANCE: i32 = RENDER_DISTANCE + 1;
/// 每帧最多提交到异步队列的区块数。控制任务提交速率，避免工作线程积压。
pub const CHUNKS_PER_FRAME: usize = 16;
/// 最大缓存区块数。当超过此数量时，使用LRU策略淘汰最久未访问的区块。
pub const MAX_CACHED_CHUNKS: usize = 2000;
/// LRU淘汰时每帧最多卸载的区块数。避免一帧内卸载太多导致卡顿。
pub const LRU_UNLOADS_PER_FRAME: usize = 16;
/// 每帧最多标记邻居为脏的数量。限制脏标记速率，避免级联重建风暴。
pub const NEIGHBOR_DIRTY_PER_FRAME: usize = 16;
/// 每帧最多处理的删除数量。控制分帧删除速率，避免大量删除操作阻塞主线程。
pub const DELETIONS_PER_FRAME: usize = 16;
/// 每帧分帧加载队列构建最多处理的区块扫描步数。
pub const QUEUE_BUILD_STEPS_PER_FRAME: usize = 500;

/// 已加载区块的条目
pub struct ChunkEntry {
    pub entity: Entity,
    pub data: Chunk,
    pub last_accessed: u64,
    pub mesh_handle: Handle<Mesh>,
    pub material_handle: Handle<VoxelMaterial>,
    pub lod_level: LodLevel,
}

/// 待删除区块的信息
struct PendingDeletion {
    entity: Entity,
    mesh_handle: Handle<Mesh>,
}

#[derive(Resource)]
pub struct LoadedChunks {
    pub entries: HashMap<ChunkCoord, ChunkEntry>,
    pub load_queue: Vec<ChunkCoord>,
    pub last_player_chunk: Option<ChunkCoord>,
    pub frame_counter: u64,
    pending_deletions: Vec<PendingDeletion>,
    load_queue_build_state: Option<LoadQueueBuildState>,
    /// 待处理的跨区块树木写入请求
    pub pending_tree_writes: Vec<TreeWriteRequest>,
    /// 树木生成器（全局共享）
    tree_generator: TreeGenerator,
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
            pending_tree_writes: Vec::new(),
            tree_generator: TreeGenerator::new(54321, TreeConfig::default()),
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
    dx: i32,
    dz: i32,
    cy: i32,
    missing: Vec<ChunkCoord>,
}

impl LoadQueueBuildState {
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
pub fn setup_world(
    mut commands: Commands,
    mut loaded: ResMut<LoadedChunks>,
    resource_pack: Res<ResourcePackManager>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<VoxelMaterial>>,
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
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::default(),
        );
        bevy_image.sampler = ImageSampler::nearest();
        images.add(bevy_image)
    } else {
        images.add(Image::default())
    };

    commands.insert_resource(AtlasTextureHandle {
        handle: atlas_handle.clone(),
    });

    let shared_material = materials.add(VoxelMaterial {
        array_texture: atlas_handle.clone(),
    });
    commands.insert_resource(SharedVoxelMaterial {
        handle: shared_material,
    });

    let worker_count = crate::async_mesh::default_worker_count();
    let uv_table = crate::async_mesh::UvLookupTable::from_resource_pack(&resource_pack);
    commands.insert_resource(AsyncMeshManager::new(worker_count, uv_table));

    use crate::camera::CameraController;
    let camera_transform = Transform::from_xyz(16.0, 64.0, 16.0);
    let camera_entity = commands
        .spawn((
            Camera3d::default(),
            camera_transform,
            CameraController::default(),
            NoCpuCulling,
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
    rebuild_load_queue(center, &mut *loaded, QUEUE_BUILD_STEPS_PER_FRAME);
}

/// 每帧系统：异步网格结果收集 + 分帧任务提交 + 卸载远处区块 + LOD 更新。
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

    loaded.frame_counter += 1;
    let current_frame = loaded.frame_counter;

    // ── 步骤 1：收集异步结果并上传 GPU ──────────────────────────
    let results = async_mesh.collect_results(MESH_UPLOADS_PER_FRAME);
    for result in results {
        if !loaded.entries.contains_key(&result.coord) {
            continue;
        }

        if let Some(entry) = loaded.entries.get(&result.coord) {
            if dirty_query.get(entry.entity).is_ok() {
                continue;
            }
            let entity = entry.entity;

            meshes.remove(&entry.mesh_handle);

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

            let mat_handle = shared_material.handle.clone();

            commands.entity(entity).insert((
                Mesh3d(mesh_handle.clone()),
                MeshMaterial3d(mat_handle.clone()),
                ChunkMeshHandle {
                    mesh: mesh_handle.clone(),
                    material: mat_handle.clone(),
                },
            ));

            if let Some(entry) = loaded.entries.get_mut(&result.coord) {
                entry.mesh_handle = mesh_handle;
                entry.material_handle = mat_handle;
            }
        }
    }

    // ── 步骤 1.5：分帧删除处理 ────────────────────────────────
    let delete_count = DELETIONS_PER_FRAME.min(loaded.pending_deletions.len());
    let deletions_this_frame = loaded.pending_deletions.drain(..delete_count);
    for deletion in deletions_this_frame {
        meshes.remove(&deletion.mesh_handle);
        commands.entity(deletion.entity).despawn();
    }

    // ── 步骤 2：检测玩家移动，启动/继续分帧加载队列构建 ──────────────────────
    let needs_rebuild =
        loaded.load_queue_build_state.is_some() || loaded.last_player_chunk != Some(player_chunk);

    if needs_rebuild {
        if loaded.load_queue_build_state.is_none() {
            loaded.last_player_chunk = Some(player_chunk);
        }

        if let Some(built_queue) =
            rebuild_load_queue(player_chunk, &mut *loaded, QUEUE_BUILD_STEPS_PER_FRAME)
        {
            loaded.load_queue = built_queue;
            unload_distant_chunks(player_chunk, &mut *loaded, &*async_mesh, &mut *lod_manager);
        }
    }

    // ── 步骤 2.5：更新 LOD 管理器 ─────────────────────────────
    let to_rebuild = lod_manager.update(player_chunk, &*loaded);
    for (coord, new_lod) in to_rebuild {
        if let Some(entry) = loaded.entries.get(&coord) {
            commands.entity(entry.entity).insert(DirtyChunk);
            if let Some(entry) = loaded.entries.get_mut(&coord) {
                entry.lod_level = new_lod;
            }
        }
    }

    lru_evict(player_chunk, &mut *loaded, &*async_mesh, &mut *lod_manager);

    // ── 步骤 3：分帧提交异步任务 ────────────────────────────────
    let drain_count = CHUNKS_PER_FRAME.min(loaded.load_queue.len());
    let chunks_to_load: Vec<ChunkCoord> = loaded.load_queue.drain(..drain_count).collect();

    // 批量收集脏标记，最后一次性应用
    let mut dirty_neighbors: Vec<Entity> = Vec::new();
    let mut neighbor_dirty_remaining = NEIGHBOR_DIRTY_PER_FRAME;

    for coord in chunks_to_load {
        if loaded.entries.contains_key(&coord) {
            continue;
        }

        if async_mesh.is_pending(&coord) {
            continue;
        }

        let mut chunk = Chunk::filled(0);
        fill_terrain(&mut chunk, &coord);

        // 阶段2：生成树木（两阶段生成方案）
        let (local_tree_writes, cross_chunk_requests) =
            generate_trees_in_chunk(&chunk, coord, &loaded.tree_generator);

        // 应用本地树木写入
        for (pos, block_id) in local_tree_writes {
            let local_x = (pos.x - coord.cx as i32 * 32) as usize;
            let local_y = (pos.y - coord.cy as i32 * 32) as usize;
            let local_z = (pos.z - coord.cz as i32 * 32) as usize;
            chunk.set(local_x, local_y, local_z, block_id);
        }

        // 收集跨区块写入请求
        loaded.pending_tree_writes.extend(cross_chunk_requests);

        if is_air_chunk(&chunk) {
            continue;
        }

        let neighbors = collect_neighbors(coord, &*loaded);

        let dist_sq = (coord.cx - player_chunk.cx).pow(2)
            + (coord.cy - player_chunk.cy).pow(2)
            + (coord.cz - player_chunk.cz).pow(2);
        let lod_level = LodLevel::from_chunk_distance_sq(dist_sq);

        let placeholder_mesh = meshes.add(Mesh::new(
            bevy::render::render_resource::PrimitiveTopology::TriangleList,
            RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
        ));
        let placeholder_mat = shared_material.handle.clone();

        // 一次性创建带组件的实体（优化：减少 Commands 调用）
        let position = coord.to_world_origin();
        let entity = commands
            .spawn((
                chunk.clone(),
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
                data: chunk,
                last_accessed: current_frame,
                mesh_handle: placeholder_mesh.clone(),
                material_handle: placeholder_mat.clone(),
                lod_level,
            },
        );

        lod_manager.set_lod(coord, lod_level);

        let entry = loaded.entries.get(&coord).unwrap();
        async_mesh.submit_task(MeshTask::Generate {
            coord,
            data: entry.data.clone(),
            neighbors,
            lod_level: Some(lod_level),
        });

        // 收集脏标记（不立即应用）
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
                if is_air_chunk(&neighbor_entry.data) {
                    continue;
                }
                dirty_neighbors.push(neighbor_entry.entity);
                neighbor_dirty_remaining -= 1;
            }
        }
    }

    // 批量应用脏标记（优化：一次性插入多个实体的组件）
    for entity in dirty_neighbors {
        commands.entity(entity).insert(DirtyChunk);
    }

    // 阶段3：处理跨区块树木写入
    if !loaded.pending_tree_writes.is_empty() {
        // 先取出 pending_tree_writes，避免同时借用
        let requests: Vec<TreeWriteRequest> = std::mem::take(&mut loaded.pending_tree_writes);

        // 直接对已加载的区块应用树木写入
        let mut modified_chunks = Vec::new();
        for request in &requests {
            if let Some(entry) = loaded.entries.get_mut(&request.chunk) {
                let local_x = (request.world_pos.x - request.chunk.cx as i32 * 32) as usize;
                let local_y = (request.world_pos.y - request.chunk.cy as i32 * 32) as usize;
                let local_z = (request.world_pos.z - request.chunk.cz as i32 * 32) as usize;
                entry.data.set(local_x, local_y, local_z, request.block_id);

                if !modified_chunks.contains(&request.chunk) {
                    modified_chunks.push(request.chunk);
                }
            }
        }

        // 标记被修改的邻居区块为脏
        for chunk_coord in modified_chunks {
            if let Some(entry) = loaded.entries.get(&chunk_coord) {
                commands.entity(entry.entity).insert(DirtyChunk);
            }
        }
    }
}

/// 重建加载队列（分帧版本）
fn rebuild_load_queue(
    center: ChunkCoord,
    loaded: &mut LoadedChunks,
    steps_limit: usize,
) -> Option<Vec<ChunkCoord>> {
    if let Some(ref state) = loaded.load_queue_build_state {
        if state.center != center {
            loaded.load_queue_build_state = None;
        }
    }

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

    while steps_done < steps_limit {
        if state.dx * state.dx + state.dz * state.dz > RENDER_DISTANCE * RENDER_DISTANCE {
            state.dz += 1;
            if state.dz > RENDER_DISTANCE {
                state.dz = -RENDER_DISTANCE;
                state.dx += 1;
            }
            if state.dx > RENDER_DISTANCE {
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
            if state.dz > RENDER_DISTANCE {
                state.dz = -RENDER_DISTANCE;
                state.dx += 1;
            }
        }

        steps_done += 1;

        if state.dx > RENDER_DISTANCE {
            break;
        }
    }

    if state.dx > RENDER_DISTANCE {
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
        None
    }
}

/// 卸载超出加载范围的区块实体
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
        async_mesh.cancel_task(coord);
        lod_manager.remove(&coord);

        if let Some(entry) = loaded.entries.remove(&coord) {
            loaded.pending_deletions.push(PendingDeletion {
                entity: entry.entity,
                mesh_handle: entry.mesh_handle,
            });
        }
    }
}

/// LRU 缓存淘汰
fn lru_evict(
    center: ChunkCoord,
    loaded: &mut LoadedChunks,
    async_mesh: &AsyncMeshManager,
    lod_manager: &mut LodManager,
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

    candidates.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| b.2.cmp(&a.2)));

    let evict_count = (loaded.entries.len() - MAX_CACHED_CHUNKS)
        .min(LRU_UNLOADS_PER_FRAME)
        .min(candidates.len());

    for i in 0..evict_count {
        let coord = candidates[i].0;

        async_mesh.cancel_task(coord);
        lod_manager.remove(&coord);

        if let Some(entry) = loaded.entries.remove(&coord) {
            loaded.pending_deletions.push(PendingDeletion {
                entity: entry.entity,
                mesh_handle: entry.mesh_handle,
            });
        }
    }
}
