//! Dirty-flag driven chunk mesh rebuild system.
//!
//! 脏块重建现在通过异步网格生成系统处理：
//! 1. 检测到 DirtyChunk 组件时，提交异步网格生成任务
//! 2. 移除 DirtyChunk 组件（标记为已提交）
//! 3. 异步结果由 `chunk_loader_system` 统一收集并上传 GPU

use bevy::prelude::*;

use crate::async_mesh::{AsyncMeshManager, MeshTask};
use crate::chunk::{ChunkCoord, ChunkData, ChunkNeighbors};
use crate::chunk_manager::LoadedChunks;
use crate::lod::{LodLevel, LodManager};
use crate::resource_pack::VoxelMaterial;

/// Tag component: chunk needs mesh rebuild.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
pub struct DirtyChunk;

/// 存储每个 chunk 实体的 Atlas 纹理句柄，用于脏块重建时获取正确的纹理
#[derive(Component, Clone)]
pub struct ChunkAtlasHandle(pub Handle<Image>);

/// 存储每个 chunk 实体的区块坐标，用于脏块重建时查找邻居
#[derive(Component, Clone, Copy, Debug)]
pub struct ChunkCoordComponent(pub ChunkCoord);

/// 追踪 chunk 实体的 mesh 和 material handle，用于重建时移除旧资源避免内存泄漏
#[derive(Component, Clone)]
pub struct ChunkMeshHandle {
    pub mesh: Handle<Mesh>,
    pub material: Handle<VoxelMaterial>,
}

/// Mark a chunk entity as needing mesh rebuild.
pub fn mark_chunk_dirty(commands: &mut Commands, entity: Entity) {
    commands.entity(entity).insert(DirtyChunk);
}

/// Returns true if the chunk data is "air-only".
pub fn is_air_chunk(chunk: &ChunkData) -> bool {
    match chunk {
        ChunkData::Empty => true,
        ChunkData::Uniform(id) => *id == 0,
        ChunkData::Paletted(data) => data.is_empty(),
    }
}

/// 6 个方向的偏移量，与 chunk.rs 中 FACES 顺序一致：[+X, -X, +Y, -Y, +Z, -Z]
const NEIGHBOR_OFFSETS: [(i32, i32, i32); 6] = [
    (1, 0, 0),  // +X (Right)
    (-1, 0, 0), // -X (Left)
    (0, 1, 0),  // +Y (Top)
    (0, -1, 0), // -Y (Bottom)
    (0, 0, 1),  // +Z (Front)
    (0, 0, -1), // -Z (Back)
];

/// 从已加载区块中收集指定坐标的 6 个邻居数据
fn collect_neighbors(coord: ChunkCoord, loaded: &LoadedChunks) -> ChunkNeighbors {
    let mut neighbors = ChunkNeighbors::empty();

    for (i, (dx, dy, dz)) in NEIGHBOR_OFFSETS.iter().enumerate() {
        let neighbor_coord = ChunkCoord {
            cx: coord.cx + dx,
            cy: coord.cy + dy,
            cz: coord.cz + dz,
        };

        if let Some(entry) = loaded.entries.get(&neighbor_coord) {
            neighbors.neighbor_data[i] = Some(entry.data.to_vec());
        }
    }

    neighbors
}

/// 检测脏块并提交异步网格重建任务。
///
/// 工作流程：
/// 1. 遍历所有带 `DirtyChunk` 组件的实体
/// 2. 全空气区块：清理旧 Mesh/Material 资源，替换为空 Mesh
/// 3. 非空气区块：收集邻居数据，提交异步网格生成任务（携带 LOD 级别）
/// 4. 移除 `DirtyChunk` 组件（结果将由 `chunk_loader_system` 统一处理）
///
/// 异步结果通过 `chunk_loader_system` 中的 `AsyncMeshManager::collect_results()` 收集，
/// 并根据 `ChunkCoord` 匹配到正确的实体进行 GPU 上传。
pub fn rebuild_dirty_chunks(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    async_mesh: Res<AsyncMeshManager>,
    mut loaded: ResMut<LoadedChunks>,
    dirty_chunks: Query<
        (Entity, &ChunkData, &ChunkCoordComponent, &ChunkMeshHandle),
        With<DirtyChunk>,
    >,
    shared_material: Res<crate::chunk_manager::SharedVoxelMaterial>,
    lod_manager: Res<LodManager>,
) {
    for (entity, chunk_data, coord_comp, mesh_handle) in &dirty_chunks {
        let coord = coord_comp.0;

        // 全空气区块：清理旧 Mesh 资源，替换为空 Mesh
        if is_air_chunk(chunk_data) {
            // 移除旧的 mesh 资源（材质使用全局共享实例，不单独移除）
            meshes.remove(&mesh_handle.mesh);

            // 创建空 Mesh（无顶点数据，不渲染任何内容）
            let empty_mesh = meshes.add(Mesh::new(
                bevy::render::render_resource::PrimitiveTopology::TriangleList,
                bevy::asset::RenderAssetUsages::MAIN_WORLD
                    | bevy::asset::RenderAssetUsages::RENDER_WORLD,
            ));
            // 使用全局共享材质实例
            let empty_mat = shared_material.handle.clone();

            // 更新实体组件
            commands.entity(entity).insert((
                Mesh3d(empty_mesh.clone()),
                MeshMaterial3d(empty_mat.clone()),
                ChunkMeshHandle {
                    mesh: empty_mesh.clone(),
                    material: empty_mat.clone(),
                },
            ));

            // 更新 LoadedChunks 中的句柄
            if let Some(entry) = loaded.entries.get_mut(&coord) {
                entry.mesh_handle = empty_mesh;
                entry.material_handle = empty_mat;
            }

            commands.entity(entity).remove::<DirtyChunk>();
            continue;
        }

        // 获取该区块的当前 LOD 级别
        let lod_level = lod_manager.get_lod(&coord);

        // 收集邻居数据用于跨区块面剔除
        let neighbors = collect_neighbors(coord, &loaded);

        // 提交异步网格生成任务（携带 LOD 级别）
        let submitted = async_mesh.submit_task(MeshTask::Generate {
            coord,
            data: chunk_data.clone(),
            neighbors,
            lod_level: Some(lod_level),
        });

        // 只有任务成功提交时才移除脏标记；
        // 如果因该区块已有异步任务在处理中而被跳过，保留脏标记以便下帧重试。
        // 这修复了快速放置+破坏方块时的"幽灵方块"竞态条件：
        // 旧任务完成后会上传过时网格，但脏标记保留确保会再次重建。
        if submitted {
            commands.entity(entity).remove::<DirtyChunk>();
        }
    }
}
