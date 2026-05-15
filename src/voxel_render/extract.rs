//! 数据提取模块（简化版）
//!
//! 当前版本：仅定义数据结构，不实现实际提取
//! 后续需要实现 Main World -> Render World 的数据传递

use bevy::prelude::*;

use super::buffers::ChunkMeshData;

/// 主世界的渲染状态（控制提取行为）
#[derive(Resource)]
pub struct VoxelRenderState {
    /// 是否需要更新Buffer
    pub dirty: bool,
    /// 待上传的Mesh数据队列
    pub upload_queue: Vec<ChunkMeshData>,
    /// 待删除的区块列表
    pub remove_queue: Vec<crate::chunk::ChunkCoord>,
}

impl Default for VoxelRenderState {
    fn default() -> Self {
        Self {
            dirty: false,
            upload_queue: Vec::new(),
            remove_queue: Vec::new(),
        }
    }
}
