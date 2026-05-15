//! Voxel Render Plugin - MultiDrawIndirect 渲染系统
//!
//! 实现方案B：使用 DrawIndexedIndirect 命令批量渲染所有区块，
//! 将 Draw Call 从 5000+ 降低到 10-20。
//!
//! # 当前状态
//!
//! 骨架框架，所有系统为空操作。后续逐步实现：
//! 1. CPU端视锥体剔除
//! 2. Buffer 创建和上传
//! 3. 渲染命令集成
//!
//! # 模块结构
//!
//! - `bridge`: 桥接模块，连接现有异步网格系统和新渲染系统
//! - `buffers`: 缓冲区管理，定义数据结构和Buffer分配器
//! - `extract`: 数据提取，将主世界数据提取到渲染世界
//! - `prepare`: 准备阶段，更新GPU缓冲区
//! - `queue`: 排队阶段，将绘制命令加入渲染队列
//! - `draw`: 绘制命令，执行MultiDrawIndirect调用
//! - `pipeline`: 渲染管线，定义BindGroup和Pipeline
//! - `plugin`: 插件入口，注册所有系统和资源

mod bridge;
mod buffers;
mod draw;
mod extract;
mod pipeline;
mod plugin;
mod prepare;
mod queue;

pub use bridge::RenderBridgePlugin;
pub use buffers::{ChunkMeshData, VoxelBuffers};
pub use draw::{MergedVoxelMesh, VoxelRenderCommandPlugin};
pub use extract::VoxelRenderState;
pub use plugin::VoxelRenderPlugin;

/// 配置常量
pub mod config {
    /// 最大区块数量（用于预分配Buffer）
    /// 32区块视距 × 5层Y × 圆形裁剪 ≈ 1600 区块
    pub const MAX_CHUNKS: usize = 2048;

    /// 每个区块最大顶点数（LOD0: 32³ × 6面 × 4顶点 / 2 ≈ 24000）
    pub const MAX_VERTICES_PER_CHUNK: usize = 24000;

    /// 每个区块最大索引数
    pub const MAX_INDICES_PER_CHUNK: usize = 36000;

    /// 全局顶点缓冲区大小
    pub const VERTEX_BUFFER_SIZE: usize = MAX_CHUNKS * MAX_VERTICES_PER_CHUNK;

    /// 全局索引缓冲区大小
    pub const INDEX_BUFFER_SIZE: usize = MAX_CHUNKS * MAX_INDICES_PER_CHUNK;
}
