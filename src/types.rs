//! 核心类型定义（坐标、常量）。
//!
//! 集中定义跨模块共享的基础类型，消除 chunk.rs ↔ chunk_manager.rs ↔ lod.rs
//! 之间的循环类型依赖。

use bevy::prelude::*;

/// 区块单维度尺寸（32³ 体素/区块）。
pub const CHUNK_SIZE: usize = 32;

/// 区块总体素数（32³ = 32768）。
pub const CHUNK_VOLUME: usize = CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE;

/// 方块类型标识符。
pub type BlockId = u8;

// ══════════════════════════════════════════════════════════════════════════════
// 方块 ID 常量（全局规范定义）
// ══════════════════════════════════════════════════════════════════════════════

pub const AIR: BlockId = 0;
pub const GRASS: BlockId = 1;
pub const STONE: BlockId = 2;
pub const DIRT: BlockId = 3;
pub const SAND: BlockId = 4;
pub const WATER: BlockId = 5;
pub const TREE_TRUNK: BlockId = 6;
pub const TREE_LEAVES: BlockId = 7;
pub const SANDSTONE: BlockId = 8;
pub const SNOW_GRASS: BlockId = 9;
pub const GRAVEL: BlockId = 10;
pub const ROCK: BlockId = 11;
pub const MUD: BlockId = 12;

/// 区块空间中的区块坐标。
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ChunkCoord {
    pub cx: i32,
    pub cy: i32,
    pub cz: i32,
}

impl ChunkCoord {
    /// 从世界坐标计算区块坐标。
    pub fn from_world(world_pos: Vec3) -> Self {
        Self {
            cx: (world_pos.x / CHUNK_SIZE as f32).floor() as i32,
            cy: (world_pos.y / CHUNK_SIZE as f32).floor() as i32,
            cz: (world_pos.z / CHUNK_SIZE as f32).floor() as i32,
        }
    }

    /// 区块原点在世界空间中的位置。
    pub fn to_world_origin(self) -> Vec3 {
        Vec3::new(
            self.cx as f32 * CHUNK_SIZE as f32,
            self.cy as f32 * CHUNK_SIZE as f32,
            self.cz as f32 * CHUNK_SIZE as f32,
        )
    }
}

/// 世界空间中的方块位置。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BlockPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl BlockPos {
    /// 从世界坐标计算方块位置。
    pub fn from_world(world_pos: Vec3) -> Self {
        Self {
            x: world_pos.x.floor() as i32,
            y: world_pos.y.floor() as i32,
            z: world_pos.z.floor() as i32,
        }
    }

    /// 所属区块坐标。
    pub fn to_chunk_coord(self) -> ChunkCoord {
        ChunkCoord {
            cx: self.x.div_euclid(CHUNK_SIZE as i32),
            cy: self.y.div_euclid(CHUNK_SIZE as i32),
            cz: self.z.div_euclid(CHUNK_SIZE as i32),
        }
    }

    /// 区块内局部坐标。
    pub fn to_local(self) -> (usize, usize, usize) {
        (
            self.x.rem_euclid(CHUNK_SIZE as i32) as usize,
            self.y.rem_euclid(CHUNK_SIZE as i32) as usize,
            self.z.rem_euclid(CHUNK_SIZE as i32) as usize,
        )
    }
}
