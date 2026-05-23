//! 排队阶段模块
//!
//! 管理 Indirect Draw 的渲染世界初始化：
//! - 创建 count buffer（供 multi_draw_indexed_indirect_count 使用）
//! - 注册渲染图节点

use bevy::{
    core_pipeline::core_3d::graph::{Core3d, Node3d},
    prelude::*,
    render::{
        render_graph::{self, RenderGraph},
        render_resource::*,
        renderer::{RenderDevice, RenderQueue},
        Extract, ExtractSchedule, Render, RenderApp, RenderStartup, RenderSystems,
    },
};

use super::draw::VoxelIndirectLabel;
use super::extract::ExtractedVoxelBuffers;

/// 计数缓冲区：Indirect Draw 的 draw count
///
/// 每帧由 queue_voxel_draw 系统更新，
/// 供 multi_draw_indexed_indirect_count 读取。
#[derive(Resource)]
pub struct VoxelDrawCountBuffer {
    pub buffer: Buffer,
}

impl VoxelDrawCountBuffer {
    pub fn new(render_device: &RenderDevice) -> Self {
        let buffer = render_device.create_buffer(&BufferDescriptor {
            label: Some("voxel_draw_count_buffer"),
            size: std::mem::size_of::<u32>() as u64,
            usage: BufferUsages::INDIRECT | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self { buffer }
    }
}

/// 排队阶段：更新 draw count 缓冲区
pub fn queue_voxel_draw(
    draw_count: Res<VoxelDrawCountBuffer>,
    extracted: Res<ExtractedVoxelBuffers>,
    render_queue: Res<RenderQueue>,
) {
    let count: u32 = extracted.chunk_count;
    let bytes = bytemuck::bytes_of(&count);
    render_queue.write_buffer(&draw_count.buffer, 0, bytes);
}

/// 创建 draw count 缓冲区（RenderStartup 阶段，此时 RenderDevice 已就绪）
fn create_draw_count_buffer(mut commands: Commands, render_device: Res<RenderDevice>) {
    commands.insert_resource(VoxelDrawCountBuffer::new(&render_device));
}

/// 注册 Indirect Draw 的渲染图节点
///
/// 将体素渲染节点插入到 Core3d 子图（StartMainPass 之前），
/// 此时 CameraDriverNode 已经设置了 view_entity。
pub fn setup_voxel_indirect_graph(app: &mut App) {
    let render_app = app.sub_app_mut(RenderApp);

    // 获取 Core3d 子图（由 Core3dPlugin 注册的相机子图）
    let mut render_graph = render_app.world_mut().resource_mut::<RenderGraph>();
    let Some(core3d) = render_graph.get_sub_graph_mut(Core3d) else {
        // Core3d 尚未注册，跳过节点注册（后续帧由 CameraDriver 驱动）
        info!("VoxelIndirectGraph: Core3d sub-graph not yet registered, skipping");
        return;
    };

    // 注册节点到 Core3d 子图
    core3d.add_node(VoxelIndirectLabel, super::draw::VoxelIndirectNode);
    core3d.add_node_edge(Node3d::EndPrepasses, VoxelIndirectLabel);
    core3d.add_node_edge(VoxelIndirectLabel, Node3d::StartMainPass);

    info!("VoxelIndirectGraph: registered in Core3d sub-graph");

    // Buffer 创建移入 RenderStartup 系统，确保 RenderDevice 已就绪
    render_app.add_systems(RenderStartup, create_draw_count_buffer);
}
