//! VoxelRenderPlugin - 核心渲染插件
//!
//! 初始化渲染系统，管理Buffer生命周期。

use bevy::prelude::*;
use bevy::render::renderer::RenderDevice;

use super::buffers::VoxelBuffers;
use super::culling::GpuCullingPlugin;
use super::draw::VoxelRenderCommandPlugin;
use super::extract::VoxelRenderState;
use crate::chunk_manager::LoadedChunks;

/// Voxel渲染插件
pub struct VoxelRenderPlugin;

impl Plugin for VoxelRenderPlugin {
    fn build(&self, app: &mut App) {
        // 注册主世界资源
        app.init_resource::<VoxelRenderState>();

        // 注册子插件
        app.add_plugins((VoxelRenderCommandPlugin, GpuCullingPlugin));

        // 注册更新系统
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
    loaded: Res<LoadedChunks>,
) {
    // 检测已卸载的区块
    // 遍历 chunk_regions，移除不在 loaded.entries 中的区块
    let coords_to_remove: Vec<_> = buffers.chunk_regions.keys()
        .filter(|coord| !loaded.entries.contains_key(coord))
        .cloned()
        .collect();
    
    for coord in coords_to_remove {
        buffers.remove_chunk(&coord);
        debug!("Removed unloaded chunk from render system: {:?}", coord);
    }

    // 如果没有更新，跳过
    if !render_state.dirty && !buffers.dirty {
        return;
    }

    // 处理删除请求
    for coord in render_state.remove_queue.drain(..) {
        buffers.remove_chunk(&coord);
    }

    // 上传新的Mesh数据到GPU
    for mesh_data in render_state.upload_queue.drain(..) {
        // 再次检查区块是否仍然加载
        if loaded.entries.contains_key(&mesh_data.coord) {
            buffers.upload_chunk_mesh(&render_queue, &mesh_data);
        }
    }

    // 更新Indirect命令缓冲区
    buffers.update_indirect_buffer(&render_queue);

    render_state.dirty = false;
}
