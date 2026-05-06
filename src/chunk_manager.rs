//! Chunk manager: loads chunks around the player and unloads distant ones.
//!
//! Each frame, the system checks the player's position and ensures all chunks
//! within `RENDER_DISTANCE` are loaded. Chunks beyond `UNLOAD_DISTANCE` are
//! despawned to free memory.
//!
//! TODO(P0): 当前只加载 Y=0 一层区块（cy 硬编码为 0），需支持多层 Y 轴加载。
//! 目标方案（参见 docs/架构总纲.md §4.3）：
//!   - 引入 load_radius_v 参数控制垂直加载半径（如 ±4 层 = ±128 米）
//!   - 玩家所在 Y 层 ± load_radius_v 范围内的区块都应加载
//!   - 更高/更低的 SubChunk → Empty 或 Uniform（空气）
//!   - 迁移到 16×32×16 SubChunk 后，Y 方向 1280 个 SubChunk/列，
//!     实际只加载 ~16 个（±4 个 Y 层），其余按需流式加载

use bevy::prelude::*;
use std::collections::HashMap;

use crate::chunk::{Chunk, ChunkCoord, fill_terrain, spawn_chunk_entity};

/// Number of chunks in each direction from the player to load.
/// RENDER_DISTANCE=2 means a 5×5 grid of chunks (2 in each direction + center).
pub const RENDER_DISTANCE: i32 = 2;

/// Chunks beyond this distance (in chunk coordinates) are unloaded.
/// Set slightly larger than RENDER_DISTANCE to avoid thrashing.
pub const UNLOAD_DISTANCE: i32 = RENDER_DISTANCE + 1;

/// Resource tracking which chunks are currently loaded.
/// Maps chunk coordinate → entity.
#[derive(Resource, Default)]
pub struct LoadedChunks {
    pub entities: HashMap<ChunkCoord, Entity>,
}

/// Number of Y-axis chunk layers to load (centered on player's chunk layer).
/// TODO(P0): 后期改为 load_radius_v 参数，支持动态垂直加载范围。
pub const Y_LAYERS: i32 = 1; // 当前只加载 1 层（cy=0），后期改为 ±4

/// Startup system: spawns the camera and HUD, then loads initial chunks.
pub fn setup_world(
    mut commands: Commands,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut loaded: ResMut<LoadedChunks>,
) {
    // Camera starts above the center of the initial chunk area.
    use crate::camera::CameraController;
    let camera_transform = Transform::from_xyz(16.0, 20.0, 16.0);
    let camera_entity = commands
        .spawn((
            Camera3d::default(),
            camera_transform,
            CameraController::default(),
        ))
        .id();

    // Create HUD tied to this camera entity
    crate::hud::setup_hud(&mut commands, camera_entity);

    // Load initial chunks around origin
    load_chunks_around(
        ChunkCoord {
            cx: 0,
            cy: 0,
            cz: 0,
        },
        &mut commands,
        &mut materials,
        &mut meshes,
        &mut loaded,
    );
}

/// Each frame: load missing chunks around the player, unload distant ones.
pub fn chunk_loader_system(
    mut commands: Commands,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut loaded: ResMut<LoadedChunks>,
    camera_query: Query<&Transform, With<Camera3d>>,
) {
    let Ok(cam_transform) = camera_query.single() else {
        return;
    };

    // Determine which chunk the player is in
    let player_chunk = ChunkCoord::from_world(cam_transform.translation);

    // Load chunks within render distance
    load_chunks_around(
        player_chunk,
        &mut commands,
        &mut materials,
        &mut meshes,
        &mut loaded,
    );

    // Unload chunks beyond unload distance
    unload_distant_chunks(player_chunk, &mut commands, &mut loaded);
}

/// Loads all chunks within RENDER_DISTANCE of `center` that aren't already loaded.
fn load_chunks_around(
    center: ChunkCoord,
    commands: &mut Commands,
    materials: &mut Assets<StandardMaterial>,
    meshes: &mut Assets<Mesh>,
    loaded: &mut LoadedChunks,
) {
    // TODO(P0): 当前 Y_LAYERS=1 只加载 cy=0 一层，后期改为基于玩家 Y 坐标的动态范围
    let cy_min = -Y_LAYERS / 2;
    let cy_max = Y_LAYERS / 2;

    for dx in -RENDER_DISTANCE..=RENDER_DISTANCE {
        for dz in -RENDER_DISTANCE..=RENDER_DISTANCE {
            for cy in cy_min..=cy_max {
                let coord = ChunkCoord {
                    cx: center.cx + dx,
                    cy,
                    cz: center.cz + dz,
                };

                if loaded.entities.contains_key(&coord) {
                    continue; // already loaded
                }

                // Generate terrain for this chunk
                let mut chunk = Chunk::filled(0);
                fill_terrain(&mut chunk);

                let position = coord.to_world_origin();
                let entity = spawn_chunk_entity(commands, materials, meshes, chunk, position);
                loaded.entities.insert(coord, entity);
            } // end cy loop
        }
    }
}

/// Despawns chunks that are beyond UNLOAD_DISTANCE from the player.
fn unload_distant_chunks(center: ChunkCoord, commands: &mut Commands, loaded: &mut LoadedChunks) {
    let to_remove: Vec<ChunkCoord> = loaded
        .entities
        .keys()
        .filter(|coord| {
            let dx = (coord.cx - center.cx).abs();
            let dz = (coord.cz - center.cz).abs();
            dx > UNLOAD_DISTANCE || dz > UNLOAD_DISTANCE
        })
        .copied()
        .collect();

    for coord in to_remove {
        if let Some(entity) = loaded.entities.remove(&coord) {
            commands.entity(entity).despawn();
        }
    }
}
