//! Dirty-flag driven chunk mesh rebuild system.

use bevy::{
    asset::RenderAssetUsages, mesh::Indices, prelude::*, render::render_resource::PrimitiveTopology,
};

use crate::chunk::{ChunkData, generate_chunk_mesh};
use crate::resource_pack::ResourcePackManager;

/// Tag component: chunk needs mesh rebuild.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
pub struct DirtyChunk;

/// 存储每个 chunk 实体的 Atlas 纹理句柄，用于脏块重建时获取正确的纹理
#[derive(Component, Clone)]
pub struct ChunkAtlasHandle(pub Handle<Image>);

/// Mark a chunk entity as needing mesh rebuild.
pub fn mark_chunk_dirty(commands: &mut Commands, entity: Entity) {
    commands.entity(entity).insert(DirtyChunk);
}

/// Returns true if the chunk data is "air-only".
pub fn is_air_chunk(chunk: &ChunkData) -> bool {
    match chunk {
        ChunkData::Empty => true,
        ChunkData::Uniform(id) => *id == 0,
        ChunkData::Mixed(_) => false,
    }
}

/// Rebuilds dirty chunk meshes using the resource pack for UV coordinates.
pub fn rebuild_dirty_chunks(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    resource_pack: Res<ResourcePackManager>,
    dirty_chunks: Query<
        (
            Entity,
            &ChunkData,
            &Mesh3d,
            &MeshMaterial3d<StandardMaterial>,
            &Transform,
            &ChunkAtlasHandle,
        ),
        With<DirtyChunk>,
    >,
) {
    for (entity, chunk_data, _old_mesh, _old_mat, _transform, atlas_handle) in &dirty_chunks {
        if is_air_chunk(chunk_data) {
            commands.entity(entity).remove::<DirtyChunk>();
            continue;
        }

        let (positions, uvs, normals, indices) = generate_chunk_mesh(chunk_data, &resource_pack);

        let mesh = meshes.add(
            Mesh::new(
                PrimitiveTopology::TriangleList,
                RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
            )
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
            .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
            .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
            .with_inserted_indices(Indices::U32(indices)),
        );

        let mat = materials.add(StandardMaterial {
            base_color: Color::WHITE,
            base_color_texture: Some(atlas_handle.0.clone()),
            ..default()
        });

        commands
            .entity(entity)
            .insert((Mesh3d(mesh), MeshMaterial3d(mat)))
            .remove::<DirtyChunk>();
    }
}
