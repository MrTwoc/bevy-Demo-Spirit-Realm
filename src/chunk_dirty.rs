//! Dirty-flag driven chunk mesh rebuild system.

use bevy::{
    asset::RenderAssetUsages, mesh::Indices, prelude::*, render::render_resource::PrimitiveTopology,
};

use crate::chunk::{ChunkCoord, ChunkData, ChunkNeighbors, generate_chunk_mesh};
use crate::chunk_manager::LoadedChunks;
use crate::resource_pack::ResourcePackManager;

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
    pub material: Handle<StandardMaterial>,
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

/// Rebuilds dirty chunk meshes using the resource pack for UV coordinates.
///
/// 重建时会移除旧的 mesh 和 material 资源，避免 GPU 内存泄漏。
pub fn rebuild_dirty_chunks(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    resource_pack: Res<ResourcePackManager>,
    loaded: Res<LoadedChunks>,
    dirty_chunks: Query<
        (
            Entity,
            &ChunkData,
            &Transform,
            &ChunkAtlasHandle,
            Option<&ChunkCoordComponent>,
            Option<&ChunkMeshHandle>,
        ),
        With<DirtyChunk>,
    >,
) {
    for (entity, chunk_data, _transform, atlas_handle, coord_comp, old_handles) in &dirty_chunks {
        if is_air_chunk(chunk_data) {
            commands.entity(entity).remove::<DirtyChunk>();
            continue;
        }

        // 移除旧的 mesh 和 material 资源，避免内存泄漏
        if let Some(old) = old_handles {
            meshes.remove(&old.mesh);
            materials.remove(&old.material);
        }

        // 收集邻居数据用于跨区块面剔除
        let neighbors = if let Some(coord_comp) = coord_comp {
            collect_neighbors(coord_comp.0, &loaded)
        } else {
            ChunkNeighbors::empty()
        };

        let (positions, uvs, normals, indices) =
            generate_chunk_mesh(chunk_data, &resource_pack, &neighbors);

        let mesh_handle = meshes.add(
            Mesh::new(
                PrimitiveTopology::TriangleList,
                RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
            )
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
            .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
            .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
            .with_inserted_indices(Indices::U32(indices)),
        );

        let mat_handle = materials.add(StandardMaterial {
            base_color: Color::WHITE,
            base_color_texture: Some(atlas_handle.0.clone()),
            ..default()
        });

        commands
            .entity(entity)
            .insert((
                Mesh3d(mesh_handle.clone()),
                MeshMaterial3d(mat_handle.clone()),
                ChunkMeshHandle {
                    mesh: mesh_handle,
                    material: mat_handle,
                },
            ))
            .remove::<DirtyChunk>();
    }
}
