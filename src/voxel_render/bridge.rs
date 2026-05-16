//! 渲染桥接模块
//!
//! 将现有的异步网格系统（AsyncMeshManager）连接到渲染系统。
//! 收集异步结果并存储原始 Mesh 数据，供 draw.rs 合并渲染。

use bevy::prelude::*;

use crate::chunk::ChunkCoord;
use crate::chunk_manager::LoadedChunks;

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

/// 渲染桥接系统（当前为占位，不消耗异步网格结果）
///
/// ⚠️ 注意：此系统**不再调用 `async_mesh.collect_results()`**，避免与
/// `chunk_loader_system`（First 阶段）争夺异步结果，导致区块实体
/// 的 `Mesh3d` 组件永不被更新、永久显示占位空网格。
///
/// 数据来源当前设计为 WIP（只收集不渲染）。未来启用时需改为直接从
/// ECS 实体读取已应用的 `ChunkMeshHandle` 以获取最新网格数据。
pub fn render_bridge_system(
    _async_mesh: Res<crate::async_mesh::AsyncMeshManager>,
    _loaded: Res<LoadedChunks>,
    _raw_meshes: ResMut<RawChunkMeshes>,
) {
    // 空操作：异步网格结果由 `chunk_loader_system` 统一收集并上传 GPU，
    // 此处不再重复消费结果。
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
