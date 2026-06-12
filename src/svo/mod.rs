//! SVO (Sparse Voxel Octree) 可见性剔除系统
//!
//! 基于八叉树结构的 GPU 遍历管线，对已加载的区块进行视锥剔除和距离剔除。
//! 体素数据通过 `VoxelSource` trait 从现有的 Chunk 路径读取（Plan A），
//! 未来可切换为 GPU buffer 后端（Plan B）。
//!
//! # 架构
//!
//! ```text
//! svo_sync → NodeManager（八叉树构建，读 ChunkData）
//!                 ↓
//! gpu_traversal → Compute Shader（GPU 视锥/距离剔除）
//!                 ↓
//! visibility_bridge → 设置 Chunk 实体 Visibility（控制渲染开关）
//! ```
//!
//! # Plan A vs Plan B
//!
//! - **Plan A（当前）**：VoxelSource → ChunkData（内存读取）
//! - **Plan B（未来）**：VoxelSource → GPU voxel buffer
//!   切换只需实现新的 VoxelSource，NodeManager 保持完全不变。
//!
//! # 模块清单
//!
//! | 模块 | 职责 |
//! |------|------|
//! | `node_store` | 扁平节点存储（u64 × 4 per node） |
//! | `node_manager` | 八叉树构建/管理 |
//! | `voxel_source` | 体素数据源抽象（trait + ChunkData 适配器） |
//! | `svo_sync` | 同步 LoadedChunks → SVO top-level 节点 |
//! | `gpu_traversal` | GPU Compute Shader 遍历管线 |
//! | `visibility_bridge` | CPU 端可见性设置（备选路径） |
//! | `hierarchical_bitset` | 高效位集（NodeStore 底层） |

mod hierarchical_bitset;
mod node_store;
mod node_manager;
pub mod voxel_source;
mod svo_sync;
mod gpu_traversal;
mod visibility_bridge;

pub use node_manager::*;
pub use gpu_traversal::SvoRenderQueue;

use bevy::prelude::*;

/// SVO 系统插件
pub struct SvoPlugin;

impl Plugin for SvoPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NodeManager>();
        app.init_resource::<svo_sync::SvoSyncController>();

        // SVO 同步：根据玩家位置管理八叉树节点
        app.add_systems(Update, svo_sync::sync_svo_with_loaded_chunks);

        // 可见性系统：用 SVO 剔除结果控制 Chunk 实体 Visibility
        app.init_resource::<visibility_bridge::SvoVisibilityState>();
        app.add_systems(Update, visibility_bridge::apply_svo_visibility);

        // GPU 遍历管线
        app.add_plugins(gpu_traversal::SvoGpuTraversalPlugin);
    }
}

/// 配置常量
pub mod config {
    /// 最大节点数 (2^18 = 262k, 约 8MB 的节点数据)
    pub const MAX_NODES: usize = 1 << 18;

    /// 每个节点的字节数 (4x u64 = 32 bytes, 但 GPU 只需要 16 bytes)
    pub const NODE_SIZE_BYTES: usize = 32;

    /// GPU 端节点大小 (紧凑格式: 2x u64 = 16 bytes)
    pub const GPU_NODE_SIZE_BYTES: usize = 16;

    /// 节点数据总字节数 (GPU buffer)
    pub const NODE_BUFFER_SIZE: usize = MAX_NODES * GPU_NODE_SIZE_BYTES;

    /// Section (区块) 边长 (体素数)
    pub const SECTION_SIZE: u32 = 32;

    /// Section 体素总数
    pub const SECTION_VOLUME: usize = (SECTION_SIZE as usize).pow(3);

    /// 最大 LOD 层数
    pub const MAX_LOD: u32 = 4;

    /// Top-level section 边长 (LOD=4: 32<<4 = 512 体素)
    pub const TOP_LEVEL_SIZE: u32 = SECTION_SIZE << MAX_LOD;
}

/// 位置编码函数
/// 编码: (lvl:4) | (x:20) | (y:20) | (z:20)
/// 注意: 这里的坐标是 section 坐标 (非体素坐标)
#[inline]
pub fn encode_position(lvl: u32, x: i32, y: i32, z: i32) -> u64 {
    let lvl = (lvl as u64) << 60;
    let x = ((x as u64) & 0xFFFFF) << 40;
    let y = ((y as u64) & 0xFFFFF) << 20;
    let z = (z as u64) & 0xFFFFF;
    lvl | x | y | z
}

#[inline]
pub fn decode_level(pos: u64) -> u32 {
    ((pos >> 60) & 0xF) as u32
}

#[inline]
pub fn decode_x(pos: u64) -> i32 {
    ((pos << 4) as i64 >> 44) as i32
}

#[inline]
pub fn decode_y(pos: u64) -> i32 {
    ((pos << 24) as i64 >> 44) as i32
}

#[inline]
pub fn decode_z(pos: u64) -> i32 {
    ((pos << 44) as i64 >> 44) as i32
}

/// 生成子节点的位置 (octant 0-7)
#[inline]
pub fn make_child_pos(parent_pos: u64, child_idx: u32) -> u64 {
    let lvl = decode_level(parent_pos);
    let x = decode_x(parent_pos);
    let y = decode_y(parent_pos);
    let z = decode_z(parent_pos);

    let child_lvl = lvl - 1;
    let bit_x = (child_idx & 1) as i32;
    let bit_y = ((child_idx >> 1) & 1) as i32;
    let bit_z = ((child_idx >> 2) & 1) as i32;

    encode_position(
        child_lvl,
        x * 2 + bit_x,
        y * 2 + bit_y,
        z * 2 + bit_z,
    )
}

/// 生成父节点位置
#[inline]
pub fn make_parent_pos(child_pos: u64) -> u64 {
    let lvl = decode_level(child_pos);
    let x = decode_x(child_pos);
    let y = decode_y(child_pos);
    let z = decode_z(child_pos);
    encode_position(lvl + 1, x >> 1, y >> 1, z >> 1)
}

/// 格式化的位置字符串 (调试用)
pub fn format_pos(pos: u64) -> String {
    format!(
        "L{}@[{},{},{}]",
        decode_level(pos),
        decode_x(pos),
        decode_y(pos),
        decode_z(pos),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_position_encoding() {
        let pos = encode_position(0, 10, 20, 30);
        assert_eq!(decode_level(pos), 0);
        assert_eq!(decode_x(pos), 10);
        assert_eq!(decode_y(pos), 20);
        assert_eq!(decode_z(pos), 30);
    }

    #[test]
    fn test_position_encoding_negative() {
        let pos = encode_position(2, -5, -10, -15);
        assert_eq!(decode_level(pos), 2);
        assert_eq!(decode_x(pos), -5);
        assert_eq!(decode_y(pos), -10);
        assert_eq!(decode_z(pos), -15);
    }

    #[test]
    fn test_position_encoding_large() {
        let pos = encode_position(4, 100, 200, 300);
        assert_eq!(decode_level(pos), 4);
        assert_eq!(decode_x(pos), 100);
        assert_eq!(decode_y(pos), 200);
        assert_eq!(decode_z(pos), 300);
    }

    #[test]
    fn test_child_parent_roundtrip() {
        let parent = encode_position(2, 1, 1, 1);
        for i in 0..8 {
            let child = make_child_pos(parent, i);
            assert_eq!(decode_level(child), 1);
            let parent2 = make_parent_pos(child);
            assert_eq!(parent, parent2);
        }
    }

    #[test]
    fn test_child_position_generation() {
        let parent = encode_position(2, 0, 0, 0);

        // 子节点 0: (0,0,0)
        let child0 = make_child_pos(parent, 0);
        assert_eq!(decode_level(child0), 1);
        assert_eq!(decode_x(child0), 0);
        assert_eq!(decode_y(child0), 0);
        assert_eq!(decode_z(child0), 0);

        // 子节点 7: (1,1,1)
        let child7 = make_child_pos(parent, 7);
        assert_eq!(decode_level(child7), 1);
        assert_eq!(decode_x(child7), 1);
        assert_eq!(decode_y(child7), 1);
        assert_eq!(decode_z(child7), 1);
    }
}
