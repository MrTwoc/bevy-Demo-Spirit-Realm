//! 渲染桥接模块
//!
//! 将现有的异步网格系统（AsyncMeshManager）连接到渲染系统。
//! 收集异步结果并存储原始 Mesh 数据，供 draw.rs 合并渲染。

use bevy::prelude::*;

use crate::async_mesh::{AsyncMeshManager, MESH_UPLOADS_PER_FRAME};
use crate::chunk::ChunkCoord;
use crate::chunk_manager::LoadedChunks;
use crate::lod::LodLevel;

/// 单个区块的原始 Mesh 数据
#[derive(Clone, Debug)]
pub struct RawChunkMesh {
    pub coord: ChunkCoord,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

/// 所有已加载区块的原始 Mesh 数据
///
/// draw.rs 从这里读取数据并合并为一个大 Mesh。
#[derive(Resource, Default)]
pub struct RawChunkMeshes {
    /// 区块坐标 → Mesh 数据
    pub meshes: std::collections::HashMap<ChunkCoord, RawChunkMesh>,
    /// 是否有更新
    pub dirty: bool,
}

/// 渲染桥接系统
///
/// 收集异步网格结果，存储到 RawChunkMeshes 中。
pub fn render_bridge_system(
    async_mesh: Res<AsyncMeshManager>,
    loaded: Res<LoadedChunks>,
    mut raw_meshes: ResMut<RawChunkMeshes>,
) {
    let results = async_mesh.collect_results(MESH_UPLOADS_PER_FRAME * 2);

    for result in results {
        // 检查区块是否仍然加载
        if !loaded.entries.contains_key(&result.coord) {
            continue;
        }

        let raw_mesh = RawChunkMesh {
            coord: result.coord,
            positions: result.positions,
            normals: result.normals,
            uvs: result.uvs,
            indices: result.indices,
        };

        raw_meshes.meshes.insert(result.coord, raw_mesh);
        raw_meshes.dirty = true;
    }
}

/// 渲染桥接插件
pub struct RenderBridgePlugin;

impl Plugin for RenderBridgePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RawChunkMeshes>();
        app.add_systems(
            Update,
            render_bridge_system.after(crate::chunk_manager::chunk_loader_system),
        );
    }
}
