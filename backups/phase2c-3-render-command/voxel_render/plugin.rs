//! VoxelRenderPlugin - 核心渲染插件
//!
//! 初始化渲染系统，管理Buffer生命周期。

use bevy::prelude::*;
use bevy::render::renderer::RenderDevice;

use super::buffers::VoxelBuffers;
use super::draw::VoxelRenderCommandPlugin;
use super::extract::VoxelRenderState;

/// Voxel渲染插件
pub struct VoxelRenderPlugin;

impl Plugin for VoxelRenderPlugin {
    fn build(&self, app: &mut App) {
        // 注册主世界资源
        app.init_resource::<VoxelRenderState>();

        // 注册渲染命令插件
        app.add_plugins(VoxelRenderCommandPlugin);

        // 注册一个空的更新系统，用于后续扩展
        app.add_systems(Update, update_voxel_render_state);
    }

    fn finish(&self, app: &mut App) {
        // 在finish阶段初始化需要RenderDevice的资源
        let render_device = app.world().resource::<RenderDevice>();

        // 创建Buffer资源
        let mut buffers = VoxelBuffers::default();
        buffers.create_buffers(render_device);

        // 注册到主世界（临时方案，后续应该在Render World）
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
    // 如果没有更新，跳过
    if !render_state.dirty {
        return;
    }

    // 上传新的Mesh数据到GPU
    for mesh_data in render_state.upload_queue.drain(..) {
        buffers.upload_chunk_mesh(&render_queue, &mesh_data);
    }

    // 处理删除请求
    for coord in render_state.remove_queue.drain(..) {
        if let Some(region) = buffers.chunk_regions.remove(&coord) {
            buffers.allocator.free(region);
        }
    }

    // 更新Indirect命令缓冲区
    buffers.update_indirect_buffer(&render_queue);

    render_state.dirty = false;
}
