//! Voxel Render Plugin - MultiDrawIndirect 渲染系统

mod bridge;
mod buffers;
mod draw;
mod extract;
mod pipeline;
mod plugin;
mod prepare;
mod queue;

pub use bridge::{RawChunkMeshes, RenderBridgePlugin};
pub use buffers::{ChunkMeshData, VoxelBuffers};
pub use extract::VoxelRenderState;
pub use plugin::VoxelRenderPlugin;

/// 配置常量
pub mod config {
    pub const MAX_CHUNKS: usize = 2048;
    pub const MAX_VERTICES_PER_CHUNK: usize = 24000;
    pub const MAX_INDICES_PER_CHUNK: usize = 36000;
    pub const VERTEX_BUFFER_SIZE: usize = MAX_CHUNKS * MAX_VERTICES_PER_CHUNK;
    pub const INDEX_BUFFER_SIZE: usize = MAX_CHUNKS * MAX_INDICES_PER_CHUNK;
}
