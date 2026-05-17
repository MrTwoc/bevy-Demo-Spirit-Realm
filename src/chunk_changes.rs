//! Fine-grained change markers for chunk rebuild differentiation.
//!
//! These marker components let `rebuild_dirty_chunks` distinguish between
//! distinct reasons why a chunk was tagged for mesh rebuild:
//!
//! | Marker               | Meaning                                  | Set by                  |
//! |----------------------|------------------------------------------|-------------------------|
//! | `DataChangedFlag`    | Block data in ChunkComponent was changed | `block_interaction`     |
//! | `LodChangedFlag`     | LOD level changed (data unmodified)      | `chunk_manager`         |
//! | `NeighborChangedFlag`| Neighbor chunk was loaded/modified       | both modules            |
//!
//! All three are always inserted in tandem with `DirtyChunk`, so removing
//! `DirtyChunk` implies removing its associated flag(s) as well.
//!
//! # Future extensibility
//!
//! Adding a new trigger source (e.g. biome update, player edit tool) only
//! requires: (1) inserting the appropriate flag alongside `DirtyChunk`,
//! and (2) optionally adding a new marker if an existing one doesn't fit.

use bevy::prelude::*;

/// Flag: chunk block data was modified (destroy / place block).
///
/// The `ChunkComponent` itself was mutated (via `Arc::make_mut`), so
/// `Changed<ChunkComponent>` also fires. This flag primarily serves
/// as documentation and as a retry anchor when async submission is
/// throttled.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
pub struct DataChangedFlag;

/// Flag: chunk's LOD level changed; block data is untouched.
///
/// The mesh should be regenerated at the new LOD detail level, but
/// no `ChunkComponent` mutation occurred — `Changed<ChunkComponent>`
/// will **not** fire for this case.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
pub struct LodChangedFlag;

/// Flag: a neighboring chunk was loaded, unloaded, or had its data changed.
///
/// This chunk must rebuild its mesh to re-evaluate **boundary face culling**
/// across the shared face, even though its own voxel data is unchanged.
/// Like `LodChangedFlag`, the `ChunkComponent` is not mutated.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
pub struct NeighborChangedFlag;

/// Convenience: remove all change markers from an entity in one call.
///
/// Call this when a dirty-chunk has been fully processed (successfully
/// submitted for rebuild *or* replaced with an air-mesh).
pub fn clear_change_markers(commands: &mut Commands, entity: Entity) {
    commands.entity(entity).remove::<(
        DataChangedFlag,
        LodChangedFlag,
        NeighborChangedFlag,
    )>();
}
