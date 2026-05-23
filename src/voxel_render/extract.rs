//! 数据提取模块
//!
//! 将主世界的 VoxelBuffers 提取到渲染世界。

use bevy::{
    prelude::*,
    render::{
        Extract,
        render_resource::Buffer,
    },
};

use super::buffers::{ChunkMeshData, VoxelBuffers};

/// 渲染状态：标记需要更新的区块，由 chunk_loader_system 填充
#[derive(Resource, Default)]
pub struct VoxelRenderState {
    /// 待上传的区块网格数据队列
    pub upload_queue: Vec<ChunkMeshData>,
    /// 待移除的区块坐标
    pub remove_queue: Vec<crate::chunk::ChunkCoord>,
    /// 是否需要更新
    pub dirty: bool,
}

/// 渲染世界中的 VoxelBuffer 引用
///
/// 每个字段都是 Option 因为 VoxelBuffers 在 finish() 阶段才创建，
/// 提取系统在第一帧可能还拿不到数据。
#[derive(Resource, Clone, Default)]
pub struct ExtractedVoxelBuffers {
    /// 全局顶点缓冲区
    pub vertex_buffer: Option<Buffer>,
    /// 全局索引缓冲区
    pub index_buffer: Option<Buffer>,
    /// Indirect 命令缓冲区（由 update_indirect_buffer 写入）
    pub indirect_buffer: Option<Buffer>,
    /// 区块偏移缓冲区（世界坐标）
    pub offset_buffer: Option<Buffer>,
    /// 当前可见区块数量
    pub chunk_count: u32,
    /// 最大区块数
    pub max_chunks: u32,
}

/// 从主世界 VoxelBuffers 提取数据到渲染世界
pub fn extract_voxel_buffers(
    mut extracted: ResMut<ExtractedVoxelBuffers>,
    voxel_buffers: Extract<Res<VoxelBuffers>>,
) {
    extracted.vertex_buffer = voxel_buffers.vertex_buffer.clone();
    extracted.index_buffer = voxel_buffers.index_buffer.clone();
    extracted.indirect_buffer = voxel_buffers.indirect_buffer.clone();
    extracted.offset_buffer = voxel_buffers.offset_buffer.clone();
    extracted.chunk_count = voxel_buffers.chunk_regions.len() as u32;
    extracted.max_chunks = super::config::MAX_CHUNKS as u32;
}

/// 相机视图数据（提取到渲染世界）
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ViewUniformRaw {
    pub view_proj: [[f32; 4]; 4],
}

/// 渲染世界中的视图数据资源
#[derive(Resource, Clone, Default)]
pub struct ExtractedViewData {
    pub view_uniform: ViewUniformRaw,
    pub is_valid: bool,
}

/// 提取相机视图数据
pub fn extract_view_data(
    mut view_data: ResMut<ExtractedViewData>,
    camera_query: Extract<Query<&Camera>>,
) {
    let Ok(camera) = camera_query.single() else {
        view_data.is_valid = false;
        return;
    };

    view_data.view_uniform = ViewUniformRaw {
        view_proj: camera.computed.clip_from_view.to_cols_array_2d(),
    };
    view_data.is_valid = true;
}
