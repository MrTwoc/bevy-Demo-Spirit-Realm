//! 准备阶段模块（简化版）
//!
//! 当前版本：仅定义数据结构
//! 后续需要实现 Buffer 创建和上传

use bevy::prelude::*;

use super::buffers::IndirectCommand;

/// 准备好的渲染数据
#[derive(Resource)]
pub struct PreparedVoxelData {
    /// 当前可见区块的Indirect命令
    pub indirect_commands: Vec<IndirectCommand>,
    /// 可见区块的偏移数据
    pub chunk_offsets: Vec<[f32; 4]>, // xyz + padding
    /// 可见区块数量
    pub visible_count: u32,
}

impl Default for PreparedVoxelData {
    fn default() -> Self {
        Self {
            indirect_commands: Vec::new(),
            chunk_offsets: Vec::new(),
            visible_count: 0,
        }
    }
}
