//! Block interaction: left-click to destroy, right-click to place.
//!
//! Uses the existing raycast system to determine which block the player is
//! looking at, then modifies the chunk data and marks it dirty for rebuild.
//! When a block at a chunk boundary is modified, neighbor chunks are also
//! marked dirty so their meshes can re-evaluate cross-boundary face culling.

use bevy::prelude::*;

use crate::chunk::{BlockId, BlockPos, CHUNK_SIZE, ChunkCoord, ChunkData, world_to_chunk};
use crate::chunk_dirty::{ChunkCoordComponent, mark_chunk_dirty};
use crate::chunk_manager::LoadedChunks;
use crate::raycast::RayHitState;

/// The block type to place when right-clicking.
/// Default: grass (1). Can be changed later with a hotbar system.
const PLACE_BLOCK_ID: BlockId = 1;

/// 计算方块修改在区块边界时需要标记脏的邻居坐标列表。
fn boundary_neighbor_coords(
    coord: ChunkCoord,
    local_pos: (usize, usize, usize),
) -> Vec<ChunkCoord> {
    let (lx, ly, lz) = local_pos;
    let mut neighbors = Vec::new();

    if lx == 0 {
        neighbors.push(ChunkCoord {
            cx: coord.cx - 1,
            cy: coord.cy,
            cz: coord.cz,
        });
    }
    if lx == CHUNK_SIZE - 1 {
        neighbors.push(ChunkCoord {
            cx: coord.cx + 1,
            cy: coord.cy,
            cz: coord.cz,
        });
    }
    if ly == 0 {
        neighbors.push(ChunkCoord {
            cx: coord.cx,
            cy: coord.cy - 1,
            cz: coord.cz,
        });
    }
    if ly == CHUNK_SIZE - 1 {
        neighbors.push(ChunkCoord {
            cx: coord.cx,
            cy: coord.cy + 1,
            cz: coord.cz,
        });
    }
    if lz == 0 {
        neighbors.push(ChunkCoord {
            cx: coord.cx,
            cy: coord.cy,
            cz: coord.cz - 1,
        });
    }
    if lz == CHUNK_SIZE - 1 {
        neighbors.push(ChunkCoord {
            cx: coord.cx,
            cy: coord.cy,
            cz: coord.cz + 1,
        });
    }

    neighbors
}

/// Handles left-click (destroy) and right-click (place) block interactions.
pub fn block_interaction_system(
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    hit_state: Res<RayHitState>,
    mut chunk_query: Query<(Entity, &mut ChunkData, &Transform, &ChunkCoordComponent)>,
    mut commands: Commands,
    mut loaded: ResMut<LoadedChunks>,
    cursor_options: Single<&bevy::window::CursorOptions>,
) {
    // Only interact when cursor is locked (player is in game mode)
    if cursor_options.grab_mode != bevy::window::CursorGrabMode::Locked {
        return;
    }

    let Some(hit_pos) = &hit_state.hit_pos else {
        return; // not looking at any block
    };

    // Left-click or left Ctrl + left-click → destroy block
    if mouse.just_pressed(MouseButton::Left) && !keys.pressed(KeyCode::ControlLeft) {
        destroy_block(hit_pos, &mut chunk_query, &mut commands, &mut loaded);
    }

    // Right-click → place block
    if mouse.just_pressed(MouseButton::Right) {
        if let Some(normal) = &hit_state.hit_normal {
            place_block(
                hit_pos,
                normal,
                &mut chunk_query,
                &mut commands,
                &mut loaded,
            );
        }
    }
}

/// Destroys the block at the given world position (sets it to air).
fn destroy_block(
    block_pos: &BlockPos,
    chunk_query: &mut Query<(Entity, &mut ChunkData, &Transform, &ChunkCoordComponent)>,
    commands: &mut Commands,
    loaded: &mut LoadedChunks,
) {
    let Some((coord, _)) = world_to_chunk(*block_pos) else {
        return;
    };

    // 先收集需要标记脏的邻居坐标（在 mut 遍历之前计算）
    let lx = block_pos.x.rem_euclid(CHUNK_SIZE as i32) as usize;
    let ly = block_pos.y.rem_euclid(CHUNK_SIZE as i32) as usize;
    let lz = block_pos.z.rem_euclid(CHUNK_SIZE as i32) as usize;
    let neighbor_coords = boundary_neighbor_coords(coord, (lx, ly, lz));

    // Find the chunk entity that contains this block
    for (entity, mut chunk_data, _transform, coord_comp) in chunk_query.iter_mut() {
        if coord_comp.0 != coord {
            continue;
        }

        // Set to air
        chunk_data.set(lx, ly, lz, 0);

        // 同步更新 LoadedChunks 中存储的区块数据
        if let Some(entry) = loaded.entries.get_mut(&coord) {
            entry.data.set(lx, ly, lz, 0);
        }

        mark_chunk_dirty(commands, entity);
        break;
    }

    // 在 mut 遍历结束后，标记边界邻居为脏
    for nc in &neighbor_coords {
        for (entity, _, _, coord_comp) in chunk_query.iter() {
            if coord_comp.0 == *nc {
                mark_chunk_dirty(commands, entity);
                break;
            }
        }
    }
}

/// Places a block adjacent to the hit face.
fn place_block(
    block_pos: &BlockPos,
    normal: &IVec3,
    chunk_query: &mut Query<(Entity, &mut ChunkData, &Transform, &ChunkCoordComponent)>,
    commands: &mut Commands,
    loaded: &mut LoadedChunks,
) {
    // The new block goes at hit_pos + normal
    let place_pos = BlockPos {
        x: block_pos.x + normal.x,
        y: block_pos.y + normal.y,
        z: block_pos.z + normal.z,
    };

    let Some((coord, _)) = world_to_chunk(place_pos) else {
        return;
    };

    let lx = place_pos.x.rem_euclid(CHUNK_SIZE as i32) as usize;
    let ly = place_pos.y.rem_euclid(CHUNK_SIZE as i32) as usize;
    let lz = place_pos.z.rem_euclid(CHUNK_SIZE as i32) as usize;

    // 先收集需要标记脏的邻居坐标
    let neighbor_coords = boundary_neighbor_coords(coord, (lx, ly, lz));

    // Find the chunk entity that contains the placement position
    for (entity, mut chunk_data, _transform, coord_comp) in chunk_query.iter_mut() {
        if coord_comp.0 != coord {
            continue;
        }

        // Only place if the target position is currently air
        if chunk_data.get(lx, ly, lz) == 0 {
            chunk_data.set(lx, ly, lz, PLACE_BLOCK_ID);

            // 同步更新 LoadedChunks 中存储的区块数据
            if let Some(entry) = loaded.entries.get_mut(&coord) {
                entry.data.set(lx, ly, lz, PLACE_BLOCK_ID);
            }

            mark_chunk_dirty(commands, entity);
        }
        break;
    }

    // 在 mut 遍历结束后，标记边界邻居为脏
    for nc in &neighbor_coords {
        for (entity, _, _, coord_comp) in chunk_query.iter() {
            if coord_comp.0 == *nc {
                mark_chunk_dirty(commands, entity);
                break;
            }
        }
    }
}
