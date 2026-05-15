//! 渲染桥接模块
//!
//! 将现有的异步网格系统（AsyncMeshManager）连接到新的MultiDrawIndirect渲染系统。
//! 负责将 MeshResult 转换为 ChunkMeshData，并提交到 VoxelRenderState。

use bevy::prelude::*;

use crate::async_mesh::{AsyncMeshManager, MESH_UPLOADS_PER_FRAME};
use crate::chunk_manager::LoadedChunks;
use crate::lod::LodLevel;

use super::buffers::ChunkMeshData;
use super::extract::VoxelRenderState;

/// 渲染桥接系统
///
/// 在每个帧的 Update 阶段运行，收集异步网格结果并转换为新渲染格式。
/// 这个系统替代了原有的 chunk_loader_system 中的 Mesh 上传逻辑。
pub fn render_bridge_system(
    async_mesh: Res<AsyncMeshManager>,
    loaded: Res<LoadedChunks>,
    mut render_state: ResMut<VoxelRenderState>,
) {
    // 收集异步网格结果
    let results = async_mesh.collect_results(MESH_UPLOADS_PER_FRAME);

    for result in results {
        // 检查区块是否仍然加载
        if !loaded.entries.contains_key(&result.coord) {
            // 区块已卸载，跳过
            continue;
        }

        // 获取区块的LOD级别
        let lod_level = loaded
            .entries
            .get(&result.coord)
            .map(|entry| entry.lod_level)
            .unwrap_or(LodLevel::Lod0);

        // 转换为新的渲染格式
        let mesh_data = ChunkMeshData {
            coord: result.coord,
            positions: result.positions,
            normals: result.normals,
            uvs: result.uvs,
            indices: result.indices,
            lod_level,
        };

        // 提交到渲染状态
        render_state.upload_queue.push(mesh_data);
        render_state.dirty = true;
    }
}

/// 渲染桥接插件
pub struct RenderBridgePlugin;

impl Plugin for RenderBridgePlugin {
    fn build(&self, app: &mut App) {
        // 注册渲染状态资源
        app.init_resource::<VoxelRenderState>();

        // 注册桥接系统
        // 在 Update 阶段运行，在 chunk_loader_system 之后
        app.add_systems(
            Update,
            render_bridge_system.after(crate::chunk_manager::chunk_loader_system),
        );
    }
}
