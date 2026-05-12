//! Block interaction: left-click to destroy, right-click to place.
//!
//! Uses the existing raycast system to determine which block the player is
//! looking at, then modifies the chunk data and marks it dirty for rebuild.
//! When a block at a chunk boundary is modified, neighbor chunks are also
//! marked dirty so their meshes can re-evaluate cross-boundary face culling.
//!
//! # 双副本同步
//!
//! `ChunkData` 存在两份副本：
//! 1. `LoadedChunks.entries[coord].data` — 用于射线检测（O(1) HashMap 查找）
//! 2. ECS 实体上的 `ChunkData` 组件 — 用于脏块重建时读取数据生成网格
//!
//! 修改方块时必须**同时更新两份副本**，否则射线检测和网格生成会看到不同的数据，
//! 导致"方块已被破坏但贴图仍在"的幽灵方块问题。

use bevy::prelude::*;

use crate::chunk::{BlockId, BlockPos, CHUNK_SIZE, ChunkCoord, ChunkData};
use crate::chunk_dirty::DirtyChunk;
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
///
/// 使用 `LoadedChunks` HashMap 进行 O(1) 区块查找，
/// 替代原来遍历所有实体的 O(N) 线性扫描。
///
/// 同时查询 ECS `ChunkData` 组件，确保破坏/放置方块时两份副本同步更新。
pub fn block_interaction_system(
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    hit_state: Res<RayHitState>,
    mut commands: Commands,
    mut loaded: ResMut<LoadedChunks>,
    mut chunk_query: Query<&mut ChunkData>,
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
        destroy_block(hit_pos, &mut commands, &mut loaded, &mut chunk_query);
    }

    // Right-click → place block
    if mouse.just_pressed(MouseButton::Right) {
        if let Some(normal) = &hit_state.hit_normal {
            place_block(
                hit_pos,
                normal,
                &mut commands,
                &mut loaded,
                &mut chunk_query,
            );
        }
    }
}

/// Destroys the block at the given world position (sets it to air).
///
/// 同时更新 `LoadedChunks` HashMap 和 ECS `ChunkData` 组件，
/// 确保射线检测和网格重建使用一致的数据。
fn destroy_block(
    block_pos: &BlockPos,
    commands: &mut Commands,
    loaded: &mut LoadedChunks,
    chunk_query: &mut Query<&mut ChunkData>,
) {
    let coord = ChunkCoord {
        cx: block_pos.x.div_euclid(CHUNK_SIZE as i32),
        cy: block_pos.y.div_euclid(CHUNK_SIZE as i32),
        cz: block_pos.z.div_euclid(CHUNK_SIZE as i32),
    };

    let lx = block_pos.x.rem_euclid(CHUNK_SIZE as i32) as usize;
    let ly = block_pos.y.rem_euclid(CHUNK_SIZE as i32) as usize;
    let lz = block_pos.z.rem_euclid(CHUNK_SIZE as i32) as usize;

    // O(1) HashMap 查找目标区块，同时获取 entity 用于后续 ECS 更新
    let target_entity = loaded.entries.get(&coord).map(|e| e.entity);

    if let Some(entry) = loaded.entries.get_mut(&coord) {
        // Set to air in LoadedChunks copy
        entry.data.set(lx, ly, lz, 0);
    }

    // 同步更新 ECS ChunkData 组件（脏块重建时从此处读取数据）
    if let Some(entity) = target_entity {
        if let Ok(mut chunk_data) = chunk_query.get_mut(entity) {
            chunk_data.set(lx, ly, lz, 0);
        }
        // 标记为脏
        commands.entity(entity).insert(DirtyChunk);
    }

    // 标记边界邻居为脏
    for nc in boundary_neighbor_coords(coord, (lx, ly, lz)) {
        if let Some(neighbor_entry) = loaded.entries.get(&nc) {
            commands.entity(neighbor_entry.entity).insert(DirtyChunk);
        }
    }
}

/// Places a block adjacent to the hit face.
///
/// 同时更新 `LoadedChunks` HashMap 和 ECS `ChunkData` 组件。
/// 如果目标区块不存在（被全空气跳过优化跳过），会按需创建该区块实体。
fn place_block(
    block_pos: &BlockPos,
    normal: &IVec3,
    commands: &mut Commands,
    loaded: &mut LoadedChunks,
    chunk_query: &mut Query<&mut ChunkData>,
) {
    // The new block goes at hit_pos + normal
    let place_pos = BlockPos {
        x: block_pos.x + normal.x,
        y: block_pos.y + normal.y,
        z: block_pos.z + normal.z,
    };

    let coord = ChunkCoord {
        cx: place_pos.x.div_euclid(CHUNK_SIZE as i32),
        cy: place_pos.y.div_euclid(CHUNK_SIZE as i32),
        cz: place_pos.z.div_euclid(CHUNK_SIZE as i32),
    };

    let lx = place_pos.x.rem_euclid(CHUNK_SIZE as i32) as usize;
    let ly = place_pos.y.rem_euclid(CHUNK_SIZE as i32) as usize;
    let lz = place_pos.z.rem_euclid(CHUNK_SIZE as i32) as usize;

    // O(1) HashMap 查找目标区块
    if let Some(entry) = loaded.entries.get_mut(&coord) {
        // Only place if the target position is currently air
        if entry.data.get(lx, ly, lz) == 0 {
            // 更新 LoadedChunks 副本
            entry.data.set(lx, ly, lz, PLACE_BLOCK_ID);

            let entity = entry.entity;

            // 同步更新 ECS ChunkData 组件
            if let Ok(mut chunk_data) = chunk_query.get_mut(entity) {
                chunk_data.set(lx, ly, lz, PLACE_BLOCK_ID);
            }

            // 标记为脏
            commands.entity(entity).insert(DirtyChunk);
        }
    } else {
        // 目标区块不存在（被全空气跳过优化跳过），按需创建
        let mut chunk = ChunkData::filled(0);
        chunk.set(lx, ly, lz, PLACE_BLOCK_ID);

        let position = coord.to_world_origin();

        let entity = commands
            .spawn((
                chunk.clone(),
                Transform::from_translation(position),
                Visibility::default(),
                DirtyChunk,
            ))
            .id();

        // 注册到 LoadedChunks
        loaded.entries.insert(
            coord,
            crate::chunk_manager::ChunkEntry {
                entity,
                data: chunk,
                last_accessed: loaded.frame_counter,
                mesh_handle: Handle::default(),
                material_handle: Handle::default(),
                lod_level: crate::lod::LodLevel::Lod0,
            },
        );
    }

    // 标记边界邻居为脏
    for nc in boundary_neighbor_coords(coord, (lx, ly, lz)) {
        if let Some(neighbor_entry) = loaded.entries.get(&nc) {
            commands.entity(neighbor_entry.entity).insert(DirtyChunk);
        }
    }
}
