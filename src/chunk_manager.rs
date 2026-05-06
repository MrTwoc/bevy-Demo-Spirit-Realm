//! Chunk manager: loads chunks around the player and unloads distant ones.
//!
//! 使用分帧加载队列避免一帧内同步加载大量区块导致卡顿。
//! 区块按与玩家的距离排序，每帧只加载固定数量（`CHUNKS_PER_FRAME`）。

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

/// 已加载区块的条目，包含实体和区块数据
pub struct ChunkEntry {
    pub entity: Entity,
    pub data: Chunk,
}

#[derive(Resource)]
pub struct LoadedChunks {
    pub entries: HashMap<ChunkCoord, ChunkEntry>,
    /// 分帧加载队列：按距离排序的待加载区块坐标
    pub load_queue: Vec<ChunkCoord>,
    /// 上一次玩家所在的区块坐标，用于检测玩家是否移动到了新区块
    pub last_player_chunk: Option<ChunkCoord>,
}

impl Default for LoadedChunks {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            load_queue: Vec::new(),
            last_player_chunk: None,
        }
    }
}

pub const Y_LAYERS: i32 = 1;

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
    let camera_transform = Transform::from_xyz(16.0, 20.0, 16.0);
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

    // 玩家移动到新区块时，重建加载队列
    if loaded.last_player_chunk != Some(player_chunk) {
        loaded.last_player_chunk = Some(player_chunk);
        rebuild_load_queue(player_chunk, &mut *loaded);
        unload_distant_chunks(player_chunk, &mut commands, &mut *loaded);
    }

    // 分帧加载：每帧最多加载 CHUNKS_PER_FRAME 个区块
    let drain_count = CHUNKS_PER_FRAME.min(loaded.load_queue.len());
    let chunks_to_load: Vec<ChunkCoord> = loaded.load_queue.drain(..drain_count).collect();

    for coord in chunks_to_load {
        // 跳过已加载的（可能在队列重建前已被加载）
        if loaded.entries.contains_key(&coord) {
            continue;
        }

        let mut chunk = Chunk::filled(0);
        fill_terrain(&mut chunk);

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
            },
        );
    }
}

/// 重建加载队列：收集渲染距离内所有未加载的区块坐标，按与玩家的距离排序。
///
/// 近处的区块排在前面，优先加载。
fn rebuild_load_queue(center: ChunkCoord, loaded: &mut LoadedChunks) {
    let cy_min = -Y_LAYERS / 2;
    let cy_max = Y_LAYERS / 2;

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

    // 按与玩家的曼哈顿距离排序（近处优先）
    missing.sort_by_key(|coord| {
        let dx = (coord.cx - center.cx).abs();
        let dz = (coord.cz - center.cz).abs();
        dx * dx + dz * dz
    });

    loaded.load_queue = missing;
}

/// 卸载超出 UNLOAD_DISTANCE 的区块实体。
fn unload_distant_chunks(center: ChunkCoord, commands: &mut Commands, loaded: &mut LoadedChunks) {
    let to_remove: Vec<ChunkCoord> = loaded
        .entries
        .keys()
        .filter(|coord| {
            let dx = (coord.cx - center.cx).abs();
            let dz = (coord.cz - center.cz).abs();
            dx > UNLOAD_DISTANCE || dz > UNLOAD_DISTANCE
        })
        .copied()
        .collect();

    for coord in to_remove {
        if let Some(entry) = loaded.entries.remove(&coord) {
            commands.entity(entry.entity).despawn();
        }
    }
}
