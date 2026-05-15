//! 渲染管线定义（简化版）
//!
//! 当前版本：仅定义数据结构
//! 后续需要实现 BindGroup Layout 和 Pipeline

use bevy::prelude::*;

/// 渲染管线资源（占位）
#[derive(Resource)]
pub struct VoxelRenderPipeline {
    /// 是否已初始化
    pub initialized: bool,
}

impl Default for VoxelRenderPipeline {
    fn default() -> Self {
        Self {
            initialized: false,
        }
    }
}
