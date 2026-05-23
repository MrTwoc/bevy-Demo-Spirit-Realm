//! VoxelRenderPlugin - 核心渲染插件
//!
//! 初始化渲染系统，管理Buffer生命周期。

use bevy::prelude::*;
use bevy::render::renderer::RenderDevice;
use bevy::render::{ExtractSchedule, Render, RenderApp, RenderStartup, RenderSystems};

use super::buffers::VoxelBuffers;
use super::draw::VoxelRenderCommandPlugin;
use super::extract::{
    ExtractedVoxelBuffers, ExtractedViewData, VoxelRenderState, extract_view_data,
    extract_voxel_buffers,
};
use super::pipeline::{
    VoxelBindGroup, VoxelRenderPipeline, create_voxel_render_pipeline,
    prepare_voxel_bind_groups,
};
use super::queue::{queue_voxel_draw, setup_voxel_indirect_graph};

/// Voxel渲染插件
pub struct VoxelRenderPlugin;

impl Plugin for VoxelRenderPlugin {
    fn build(&self, app: &mut App) {
        // 注册主世界资源
        app.init_resource::<VoxelRenderState>();

        // 注册子插件
        app.add_plugins(VoxelRenderCommandPlugin);

        // 注册更新系统
        app.add_systems(Update, update_voxel_render_state);

        // ── 渲染世界设置 ────────────────────────────────────
        let render_app = app.sub_app_mut(RenderApp);

        // 初始化渲染世界资源
        render_app.init_resource::<ExtractedVoxelBuffers>();
        render_app.init_resource::<ExtractedViewData>();

        // 添加提取系统
        render_app.add_systems(
            ExtractSchedule,
            (extract_voxel_buffers, extract_view_data),
        );

        // 创建渲染管线（RenderStartup 阶段）
        render_app.add_systems(
            RenderStartup,
            create_voxel_render_pipeline,
        );

        // 每帧准备 BindGroup
        render_app.add_systems(
            Render,
            prepare_voxel_bind_groups.in_set(RenderSystems::PrepareBindGroups),
        );

        // 排队阶段：更新 draw count
        render_app.add_systems(
            Render,
            queue_voxel_draw.in_set(RenderSystems::Queue),
        );

        // 注册渲染图节点
        setup_voxel_indirect_graph(app);
    }

    fn finish(&self, app: &mut App) {
        // 在finish阶段初始化需要RenderDevice的资源
        let render_device = app.world().resource::<RenderDevice>();

        // 创建Buffer资源
        let mut buffers = VoxelBuffers::default();
        buffers.create_buffers(render_device);

        // 注册到主世界
        app.insert_resource(buffers);

        info!("VoxelRenderPlugin initialized");
    }
}

/// 更新渲染状态的系统
pub fn update_voxel_render_state(
    mut render_state: ResMut<VoxelRenderState>,
    mut buffers: ResMut<VoxelBuffers>,
    render_queue: Res<bevy::render::renderer::RenderQueue>,
) {
    if !render_state.dirty && render_state.upload_queue.is_empty() && render_state.remove_queue.is_empty() {
        return;
    }

    // 移除被卸载的区块
    for coord in render_state.remove_queue.drain(..) {
        if let Some(region) = buffers.chunk_regions.remove(&coord) {
            buffers.allocator.free(region);
        }
    }

    // 上传新的Mesh数据到GPU
    for mesh_data in render_state.upload_queue.drain(..) {
        buffers.upload_chunk_mesh(&render_queue, &mesh_data);
    }

    // 更新Indirect命令缓冲区
    buffers.update_indirect_buffer(&render_queue);

    render_state.dirty = false;
}
