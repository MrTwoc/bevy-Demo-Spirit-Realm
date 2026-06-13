//! 体素数据源抽象层
//!
//! 将 SVO 八叉树构建与具体的体素存储解耦。
//! Plan A：从 LoadedChunks (ChunkData) 读取。
//! Plan B：可切换为 GPU voxel buffer 或其他后端，无需修改 NodeManager。
//!
//! # 设计原则
//!
//! 1. **单一职责**：VoxelSource 只回答"某区域是否有非空内容"。
//! 2. **零拷贝**：ChunkVoxelSource 仅持有对 LoadedChunks 的引用，不复制数据。
//! 3. **为 Plan B 预留**：trait 方法签名不依赖 ChunkCoord / ChunkData，
//!    只使用 SVO 坐标系统 (lvl, x, y, z)，确保 GPU-driven 后端可无缝接入。

use crate::chunk::{ChunkCoord, ChunkData};
use crate::chunk_manager::LoadedChunks;

// ── 核心 trait ───────────────────────────────────────────────────────────

/// 体素数据源，提供区域内容查询。
///
/// 所有坐标均为 SVO section 坐标系：
/// - `lvl`: LOD 级别（0=单 Section 32³, 4=top-level 512³）
/// - `(x, y, z)`: Section 坐标（非体素坐标）
///
/// Plan B 替代实现只需实现此 trait 即可。
pub trait VoxelSource: Send + Sync {
    /// 检查指定区域是否包含任何非空气内容。
    ///
    /// 区域大小 = (1 << lvl)³ 个 section。
    /// 递归实现：LOD=0 检查单个 chunk，LOD>0 检查 8 个子区域。
    fn region_has_content(&self, lvl: u32, x: i32, y: i32, z: i32) -> bool;
}

// ── LoadedChunks 适配器 ──────────────────────────────────────────────────

/// 基于 `LoadedChunks` 的 VoxelSource 实现。
///
/// 仅在构建期持有引用，不缓存或复制数据。
/// 每次 `region_has_content` 调用都直接查询 `LoadedChunks.entries`。
pub struct ChunkVoxelSource<'a> {
    /// 对 LoadedChunks 的不可变引用。
    /// ChunkData 通过 Arc 共享，读取无需锁。
    pub loaded: &'a LoadedChunks,
}

impl VoxelSource for ChunkVoxelSource<'_> {
    fn region_has_content(&self, lvl: u32, x: i32, y: i32, z: i32) -> bool {
        if lvl == 0 {
            // LOD=0：直接查询单个 chunk
            let coord = ChunkCoord {
                cx: x,
                cy: y,
                cz: z,
            };
            self.loaded
                .entries
                .get(&coord)
                .map(|entry| !is_air_chunk_data(&entry.data))
                .unwrap_or(false)
        } else {
            // LOD>0：递归检查 8 个子区域（短路求值，遇到第一个非空即返回）
            let child_lvl = lvl - 1;
            for ox in 0..2i32 {
                for oy in 0..2i32 {
                    for oz in 0..2i32 {
                        if self.region_has_content(
                            child_lvl,
                            x * 2 + ox,
                            y * 2 + oy,
                            z * 2 + oz,
                        ) {
                            return true;
                        }
                    }
                }
            }
            false
        }
    }
}

// ── 辅助函数 ─────────────────────────────────────────────────────────────

/// 检查 ChunkData 是否完全为空（无任何非空气方块）。
///
/// 注意：此函数与 `chunk_dirty::is_air_chunk` 功能相同，
/// 但为保持 `voxel_source.rs` 模块独立性（不依赖 chunk_dirty），
/// 在此内联实现。两者逻辑严格一致。
#[inline]
fn is_air_chunk_data(chunk: &ChunkData) -> bool {
    match chunk {
        ChunkData::Empty => true,
        ChunkData::Uniform(id) => *id == 0,
        ChunkData::Paletted(data) => data.is_empty(),
    }
}