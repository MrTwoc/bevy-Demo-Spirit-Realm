//! 绘制命令模块
//!
//! 当前阶段：只做数据收集，不实际渲染。
//! 原有 Bevy 标准渲染管线负责渲染。
//! 后续实现 MultiDrawIndirect 时再替换。

use bevy::prelude::*;

use super::bridge::RawChunkMeshes;

/// 合并后的渲染网格资源（当前未使用）
#[derive(Resource, Default)]
pub struct MergedVoxelMesh {
    pub entity: Option<Entity>,
    pub mesh_handle: Option<Handle<Mesh>>,
}

/// 数据收集系统
///
/// 每 60 帧输出一次统计信息，确认数据在正确收集。
pub fn debug_mesh_stats(raw_meshes: Res<RawChunkMeshes>, mut counter: Local<u32>) {
    *counter += 1;
    if *counter % 600 == 0 {
        let total_chunks = raw_meshes.meshes.len();
        let total_vertices: usize = raw_meshes.meshes.values().map(|m| m.positions.len()).sum();
        let total_indices: usize = raw_meshes.meshes.values().map(|m| m.indices.len()).sum();
        info!(
            "[VoxelRender] Stats: {} chunks, {} vertices, {} indices",
            total_chunks, total_vertices, total_indices
        );
    }
}

/// 绘制命令插件
pub struct VoxelRenderCommandPlugin;

impl Plugin for VoxelRenderCommandPlugin {
    fn build(&self, app: &mut App) {
        info!("[VoxelRender] VoxelRenderCommandPlugin::build (data collection only)");
        app.init_resource::<MergedVoxelMesh>();
        app.add_systems(Update, debug_mesh_stats.after(super::bridge::render_bridge_system));
    }
}
