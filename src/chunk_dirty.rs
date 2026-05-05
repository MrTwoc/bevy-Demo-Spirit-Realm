//! Dirty-flag driven chunk mesh rebuild system.
//!
//! When a block is placed or destroyed, the chunk it belongs to is marked
//! dirty. Each frame, the `rebuild_dirty_chunks` system detects dirty chunks,
//! regenerates their mesh, and clears the flag.

use bevy::{
    asset::RenderAssetUsages,
    mesh::Indices,
    prelude::*,
    render::render_resource::PrimitiveTopology,
};

use crate::chunk::{generate_chunk_mesh, BlockId, ChunkData, CHUNK_SIZE};

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

/// Tag component. Presence of this component means the chunk needs mesh rebuild.
/// Does not carry data — just a marker.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
pub struct DirtyChunk;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Mark a chunk entity as needing mesh rebuild.
pub fn mark_chunk_dirty(commands: &mut Commands, entity: Entity) {
    commands.entity(entity).insert(DirtyChunk);
}

/// Returns true if the chunk data is "air-only" (Empty or Uniform(0)).
/// Air-only chunks have nothing to render and can skip mesh generation.
pub fn is_air_chunk(chunk: &ChunkData) -> bool {
    match chunk {
        ChunkData::Empty => true,
        ChunkData::Uniform(id) => *id == 0,
        ChunkData::Mixed(_) => false,
    }
}

// ---------------------------------------------------------------------------
// Update system
// ---------------------------------------------------------------------------

/// Looks up the chunk data, rebuilds the mesh from scratch, replaces the
/// handles on the existing entity, removes the DirtyChunk tag.
///
/// Note: the old mesh/material handles are **not** explicitly removed from
/// their asset collections. Old mesh data will accumulate in `Assets<Mesh>`
/// and `Assets<StandardMaterial>` over time. For a production game, store
/// the old handles in a `ChunkMeshHandle` component (see TODO below) and call
/// `meshes.remove(handle)` during rebuild to free GPU memory.
///
/// ```ignore
/// // TODO: add ChunkMeshHandle component to track old handles for cleanup
/// pub struct ChunkMeshHandle {
///     pub mesh: Handle<Mesh>,
///     pub material: Handle<StandardMaterial>,
/// }
/// ```
pub fn rebuild_dirty_chunks(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    dirty_chunks: Query<(Entity, &ChunkData, &Mesh3d, &MeshMaterial3d<StandardMaterial>, &Transform), With<DirtyChunk>>,
) {
    for (entity, chunk_data, _old_mesh, _old_mat, transform) in &dirty_chunks {
        // Air-only chunks: no mesh to generate, just clear dirty flag.
        if is_air_chunk(chunk_data) {
            commands.entity(entity).remove::<DirtyChunk>();
            continue;
        }

        // Regenerate mesh
        let (positions, uvs, normals, colors, indices) = generate_chunk_mesh(chunk_data);

        let mesh = meshes.add(
            Mesh::new(
                PrimitiveTopology::TriangleList,
                RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
            )
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
            .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
            .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
            .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colors)
            .with_inserted_indices(Indices::U32(indices)),
        );

        let mat = materials.add(StandardMaterial {
            base_color: Color::WHITE,
            ..default()
        });

        // Replace handles on the existing entity (Bevy drops old handle refs automatically)
        commands.entity(entity).insert((
            Mesh3d(mesh),
            MeshMaterial3d(mat),
        ));

        // Remove dirty flag
        commands.entity(entity).remove::<DirtyChunk>();
    }
}

// ---------------------------------------------------------------------------
// Block modification helpers (used by future place/destroy logic)
// ---------------------------------------------------------------------------

/// Standalone block modification: set a block and mark the chunk dirty.
/// This is the main entry point for external block-change code.
pub fn set_block_dirty(
    commands: &mut Commands,
    chunk_entity: Entity,
    chunk_data: &mut ChunkData,
    local_pos: (usize, usize, usize),
    new_id: BlockId,
) {
    chunk_data.set(local_pos.0, local_pos.1, local_pos.2, new_id);
    mark_chunk_dirty(commands, chunk_entity);
}
