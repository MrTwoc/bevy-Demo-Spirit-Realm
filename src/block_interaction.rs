//! Block interaction: left-click to destroy, right-click to place.
//!
//! Uses the existing raycast system to determine which block the player is
//! looking at, then modifies the chunk data and marks it dirty for rebuild.

use bevy::prelude::*;

use crate::chunk::{BlockId, BlockPos, CHUNK_SIZE, ChunkData, world_to_chunk};
use crate::chunk_dirty::mark_chunk_dirty;
use crate::raycast::RayHitState;

/// The block type to place when right-clicking.
/// Default: grass (1). Can be changed later with a hotbar system.
const PLACE_BLOCK_ID: BlockId = 1;

/// Handles left-click (destroy) and right-click (place) block interactions.
pub fn block_interaction_system(
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    hit_state: Res<RayHitState>,
    mut chunk_query: Query<(Entity, &mut ChunkData, &Transform)>,
    mut commands: Commands,
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
        destroy_block(hit_pos, &mut chunk_query, &mut commands);
    }

    // Right-click → place block
    if mouse.just_pressed(MouseButton::Right) {
        if let Some(normal) = &hit_state.hit_normal {
            place_block(hit_pos, normal, &mut chunk_query, &mut commands);
        }
    }
}

/// Destroys the block at the given world position (sets it to air).
fn destroy_block(
    block_pos: &BlockPos,
    chunk_query: &mut Query<(Entity, &mut ChunkData, &Transform)>,
    commands: &mut Commands,
) {
    let Some((coord, _)) = world_to_chunk(*block_pos) else {
        return;
    };

    // Find the chunk entity that contains this block
    for (entity, mut chunk_data, transform) in chunk_query.iter_mut() {
        let chunk_origin = transform.translation;
        let cx = (chunk_origin.x / CHUNK_SIZE as f32).floor() as i32;
        let cy = (chunk_origin.y / CHUNK_SIZE as f32).floor() as i32;
        let cz = (chunk_origin.z / CHUNK_SIZE as f32).floor() as i32;

        if cx != coord.cx || cy != coord.cy || cz != coord.cz {
            continue;
        }

        // Convert world block pos to local chunk coordinates
        let lx = block_pos.x.rem_euclid(CHUNK_SIZE as i32) as usize;
        let ly = block_pos.y.rem_euclid(CHUNK_SIZE as i32) as usize;
        let lz = block_pos.z.rem_euclid(CHUNK_SIZE as i32) as usize;

        // Set to air
        chunk_data.set(lx, ly, lz, 0);
        mark_chunk_dirty(commands, entity);
        break;
    }
}

/// Places a block adjacent to the hit face.
fn place_block(
    block_pos: &BlockPos,
    normal: &IVec3,
    chunk_query: &mut Query<(Entity, &mut ChunkData, &Transform)>,
    commands: &mut Commands,
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

    // Find the chunk entity that contains the placement position
    for (entity, mut chunk_data, transform) in chunk_query.iter_mut() {
        let chunk_origin = transform.translation;
        let cx = (chunk_origin.x / CHUNK_SIZE as f32).floor() as i32;
        let cy = (chunk_origin.y / CHUNK_SIZE as f32).floor() as i32;
        let cz = (chunk_origin.z / CHUNK_SIZE as f32).floor() as i32;

        if cx != coord.cx || cy != coord.cy || cz != coord.cz {
            continue;
        }

        let lx = place_pos.x.rem_euclid(CHUNK_SIZE as i32) as usize;
        let ly = place_pos.y.rem_euclid(CHUNK_SIZE as i32) as usize;
        let lz = place_pos.z.rem_euclid(CHUNK_SIZE as i32) as usize;

        // Only place if the target position is currently air
        if chunk_data.get(lx, ly, lz) == 0 {
            chunk_data.set(lx, ly, lz, PLACE_BLOCK_ID);
            mark_chunk_dirty(commands, entity);
        }
        break;
    }
}
