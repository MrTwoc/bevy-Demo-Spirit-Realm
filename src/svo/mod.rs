//! SVO (Sparse Voxel Octree) 渲染系统
//!
//! 参考 voxy-dev 架构：
//! - Section: 32×32×32 体素区块 (对应原有 Chunk)
//! - Node: 八叉树节点，扁平数组存储，16 字节/节点
//! - NodeManager: CPU 端树结构管理
//! - GPU Compute: 遍历八叉树生成 indirect draw calls
//!
//! 设计要点：
//! 1. 位置编码: u64, (lvl:4) | (y:8) | (z:24) | (x:24) | (pad:4)
//! 2. 节点类型: LEAF(有几何体) / INNER(有子节点) / EMPTY(无内容)
//! 3. non-empty children: 8-bit mask, 表示 8 个 octant 是否有体素
//! 4. 扁平数组存储，GPU 作为 SSBO 直接读取

mod node_store;
mod node_manager;
mod section;
mod section_tracker;
mod render_distance;
mod terrain_bridge;
mod gpu_traversal;
mod visibility_bridge;

pub use node_manager::*;
pub use section::*;
pub use section_tracker::*;
pub use render_distance::*;

use bevy::prelude::*;

/// SVO 系统插件
pub struct SvoPlugin;

impl Plugin for SvoPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SectionTracker>();
        app.init_resource::<NodeManager>();
        app.init_resource::<RenderDistanceController>();

        // 设置 section 创建回调 (自动填充地形数据)
        let mut tracker = app.world_mut().resource_mut::<SectionTracker>();
        tracker.on_create_section = Some(terrain_bridge::make_section_filler());

        app.add_systems(Update, render_distance::process_render_distance);

        // SVO 可见性系统（已禁用：每帧遍历所有 chunk 但实际未隐藏实体，纯 CPU 浪费）
        // 等 SVO GPU 剔除 → Indirect Draw 链路打通后，用 GPU 驱动的 visibility 替代
        // app.init_resource::<visibility_bridge::SvoVisibilityState>();
        // app.add_systems(Update, visibility_bridge::apply_svo_visibility);

        // 注册 GPU 遍历插件
        app.add_plugins(gpu_traversal::SvoGpuTraversalPlugin);
    }
}

/// 配置常量
pub mod config {
    /// 最大节点数 (2^18 = 262k, 约 4MB 的节点数据)
    pub const MAX_NODES: usize = 1 << 18;

    /// 每个节点的字节数 (2x u64 = 16 bytes)
    pub const NODE_SIZE_BYTES: usize = 16;

    /// 节点数据总字节数 (GPU buffer)
    pub const NODE_BUFFER_SIZE: usize = MAX_NODES * NODE_SIZE_BYTES;

    /// Section (区块) 边长 (体素数)
    pub const SECTION_SIZE: u32 = 32;

    /// Section 体素总数
    pub const SECTION_VOLUME: usize = (SECTION_SIZE as usize).pow(3);

    /// 最大 LOD 层数
    pub const MAX_LOD: u32 = 4;

    /// Top-level section 边长 (LOD=4: 32<<4 = 512 体素)
    pub const TOP_LEVEL_SIZE: u32 = SECTION_SIZE << MAX_LOD;

    /// 二级缓存最大 section 数
    pub const SECONDARY_CACHE_SIZE: usize = 4096;

    /// Top-level LRU 缓存大小
    pub const TOP_LEVEL_CACHE_SIZE: usize = 1024;
}

/// 位置编码函数
/// 编码: (lvl:4) | (y:8) | (z:24) | (x:24) | (pad:4)
/// 注意: 这里的坐标是 section 坐标 (非体素坐标)
#[inline]
pub fn encode_position(lvl: u32, x: i32, y: i32, z: i32) -> u64 {
    let lvl = (lvl as u64) << 60;
    let y = ((y as u64) & 0xFF) << 52;
    let z = ((z as u64) & 0xFFFFFF) << 28;
    let x = ((x as u64) & 0xFFFFFF) << 4;
    lvl | y | z | x
}

#[inline]
pub fn decode_level(pos: u64) -> u32 {
    ((pos >> 60) & 0xF) as u32
}

#[inline]
pub fn decode_x(pos: u64) -> i32 {
    ((pos << 36) as i64 >> 40) as i32
}

#[inline]
pub fn decode_y(pos: u64) -> i32 {
    ((pos << 4) as i64 >> 56) as i32
}

#[inline]
pub fn decode_z(pos: u64) -> i32 {
    ((pos << 12) as i64 >> 40) as i32
}

/// 生成子节点的位置 (octant 0-7)
#[inline]
pub fn make_child_pos(parent_pos: u64, child_idx: u32) -> u64 {
    let lvl = decode_level(parent_pos);
    let x = decode_x(parent_pos);
    let y = decode_y(parent_pos);
    let z = decode_z(parent_pos);

    let child_lvl = lvl - 1;
    let half = 1i32 << (child_lvl); // 父节点的一半大小
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
    fn test_child_parent_roundtrip() {
        let parent = encode_position(2, 1, 1, 1);
        for i in 0..8 {
            let child = make_child_pos(parent, i);
            assert_eq!(decode_level(child), 1);
            let parent2 = make_parent_pos(child);
            assert_eq!(parent, parent2);
        }
    }
}
