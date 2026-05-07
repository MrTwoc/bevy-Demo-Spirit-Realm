//! Chunk manager: loads chunks around the player and unloads distant ones.
//!
//! 使用分帧加载队列避免一帧内同步加载大量区块导致卡顿。
//! 区块按与玩家的距离排序，每帧只加载固定数量（`CHUNKS_PER_FRAME`）。
//! 使用LRU（最近最少使用）缓存淘汰机制，优先卸载最久未访问且距离较远的区块。

use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use std::collections::HashMap;

use crate::chunk::{Chunk, ChunkCoord, ChunkNeighbors, fill_terrain, spawn_chunk_entity};
use crate::chunk_dirty::{ChunkAtlasHandle, ChunkCoordComponent};
use crate::resource_pack::ResourcePackManager;

/// 渲染距离（区块数）。增大此值可以看到更远的世界，但需要更多区块加载。
pub const RENDER_DISTANCE: i32 = 8;
/// 卸载距离：超过此距离的区块会被卸载。比渲染距离大 1 避免边界闪烁。
pub const UNLOAD_DISTANCE: i32 = RENDER_DISTANCE + 1;
/// 每帧最多加载的区块数。控制加载速度，避免卡顿。
pub const CHUNKS_PER_FRAME: usize = 4;
/// 最大缓存区块数。当超过此数量时，使用LRU策略淘汰最久未访问的区块。
/// 默认值：渲染距离内约 8*8*π*9 ≈ 1800 个区块，设置为 2000 留有余量。
pub const MAX_CACHED_CHUNKS: usize = 2000;
/// LRU淘汰时每帧最多卸载的区块数。避免一帧内卸载太多导致卡顿。
pub const LRU_UNLOADS_PER_FRAME: usize = 16;

/// 已加载区块的条目，包含实体、区块数据和LRU访问信息
pub struct ChunkEntry {
    pub entity: Entity,
    pub data: Chunk,
    /// 最后访问时间戳（帧号），用于LRU淘汰
    pub last_accessed: u64,
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
pub fn setup_world(
    mut commands: Commands,
    mut loaded: ResMut<LoadedChunks>,
    resource_pack: Res<ResourcePackManager>,
    mut images: ResMut<Assets<Image>>,
) {
    // 从资源包创建 Atlas 纹理
    let atlas_handle = if let Some(atlas) = &resource_pack.atlas {
        let size = Extent3d {
            width: atlas.size.0,
            height: atlas.size.1,
            depth_or_array_layers: 1,
        };
        let bevy_image = Image::new(
            size,
            TextureDimension::D2,
            atlas.image.clone(),
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::default(),
        );
        images.add(bevy_image)
    } else {
        images.add(Image::default())
    };

    commands.insert_resource(AtlasTextureHandle {
        handle: atlas_handle.clone(),
    });

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

/// 每帧系统：分帧加载区块 + 卸载远处区块。
///
/// 工作流程：
/// 1. 检测玩家是否移动到新区块，若是则重建加载队列（按距离排序）
/// 2. 从队列头部取出最多 `CHUNKS_PER_FRAME` 个区块进行加载
/// 3. 卸载超出 `UNLOAD_DISTANCE` 的区块
pub fn chunk_loader_system(
    mut commands: Commands,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut loaded: ResMut<LoadedChunks>,
    camera_query: Query<&Transform, With<Camera3d>>,
    resource_pack: Res<ResourcePackManager>,
    atlas_handle: Res<AtlasTextureHandle>,
) {
    let Ok(cam_transform) = camera_query.single() else {
        return;
    };

    let player_chunk = ChunkCoord::from_world(cam_transform.translation);

    // 递增帧计数器
    loaded.frame_counter += 1;
    let current_frame = loaded.frame_counter;

    // 玩家移动到新区块时，重建加载队列
    if loaded.last_player_chunk != Some(player_chunk) {
        loaded.last_player_chunk = Some(player_chunk);
        rebuild_load_queue(player_chunk, &mut *loaded);
        unload_distant_chunks(player_chunk, &mut commands, &mut *loaded);
    }

    // LRU 淘汰：当缓存区块数超过上限时，淘汰最久未访问且距离较远的区块
    lru_evict(player_chunk, &mut commands, &mut *loaded);

    // 分帧加载：每帧最多加载 CHUNKS_PER_FRAME 个区块
    let drain_count = CHUNKS_PER_FRAME.min(loaded.load_queue.len());
    let chunks_to_load: Vec<ChunkCoord> = loaded.load_queue.drain(..drain_count).collect();

    for coord in chunks_to_load {
        // 跳过已加载的（可能在队列重建前已被加载）
        if loaded.entries.contains_key(&coord) {
            continue;
        }

        let mut chunk = Chunk::filled(0);
        fill_terrain(&mut chunk, &coord);

        // 收集邻居数据用于跨区块面剔除
        let neighbors = collect_neighbors(coord, &*loaded);

        let position = coord.to_world_origin();
        let entity = spawn_chunk_entity(
            &mut commands,
            &mut materials,
            &mut meshes,
            chunk.clone(),
            position,
            &resource_pack,
            &atlas_handle.handle,
            &neighbors,
        );

        commands.entity(entity).insert((
            ChunkAtlasHandle(atlas_handle.handle.clone()),
            ChunkCoordComponent(coord),
        ));

        loaded.entries.insert(
            coord,
            ChunkEntry {
                entity,
                data: chunk,
                last_accessed: current_frame,
            },
        );
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
fn unload_distant_chunks(center: ChunkCoord, commands: &mut Commands, loaded: &mut LoadedChunks) {
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
        if let Some(entry) = loaded.entries.remove(&coord) {
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
fn lru_evict(center: ChunkCoord, commands: &mut Commands, loaded: &mut LoadedChunks) {
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
        if let Some(entry) = loaded.entries.remove(&coord) {
            commands.entity(entry.entity).despawn();
        }
    }
}
