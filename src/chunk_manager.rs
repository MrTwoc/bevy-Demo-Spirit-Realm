//! Chunk manager: loads chunks around the player and unloads distant ones.

use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use std::collections::HashMap;

use crate::chunk::{Chunk, ChunkCoord, fill_terrain, spawn_chunk_entity};
use crate::chunk_dirty::ChunkAtlasHandle;
use crate::resource_pack::ResourcePackManager;

pub const RENDER_DISTANCE: i32 = 2;
pub const UNLOAD_DISTANCE: i32 = RENDER_DISTANCE + 1;

#[derive(Resource, Default)]
pub struct LoadedChunks {
    pub entities: HashMap<ChunkCoord, Entity>,
}

pub const Y_LAYERS: i32 = 1;

/// 存储 Atlas 纹理句柄的资源
#[derive(Resource)]
pub struct AtlasTextureHandle {
    pub handle: Handle<Image>,
}

/// Startup system: spawns the camera and HUD, then loads initial chunks.
pub fn setup_world(
    mut commands: Commands,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut loaded: ResMut<LoadedChunks>,
    resource_pack: Res<ResourcePackManager>,
    mut images: ResMut<Assets<Image>>,
) {
    // 从资源包创建 Atlas 纹理
    let atlas_handle = if let Some(atlas) = &resource_pack.atlas {
        let size = Extent3d {
            width: atlas.size.0,
            height: atlas.size.1,
            depth_or_array_layers: 1,
        };
        let bevy_image = Image::new(
            size,
            TextureDimension::D2,
            atlas.image.clone(),
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::default(),
        );
        images.add(bevy_image)
    } else {
        images.add(Image::default())
    };

    commands.insert_resource(AtlasTextureHandle {
        handle: atlas_handle.clone(),
    });

    use crate::camera::CameraController;
    let camera_transform = Transform::from_xyz(16.0, 20.0, 16.0);
    let camera_entity = commands
        .spawn((
            Camera3d::default(),
            camera_transform,
            CameraController::default(),
        ))
        .id();

    crate::hud::setup_hud(&mut commands, camera_entity);

    load_chunks_around(
        ChunkCoord {
            cx: 0,
            cy: 0,
            cz: 0,
        },
        &mut commands,
        &mut materials,
        &mut meshes,
        &mut loaded,
        &resource_pack,
        &atlas_handle,
    );
}

/// Each frame: load missing chunks around the player, unload distant ones.
pub fn chunk_loader_system(
    mut commands: Commands,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut loaded: ResMut<LoadedChunks>,
    camera_query: Query<&Transform, With<Camera3d>>,
    resource_pack: Res<ResourcePackManager>,
    atlas_handle: Res<AtlasTextureHandle>,
) {
    let Ok(cam_transform) = camera_query.single() else {
        return;
    };

    let player_chunk = ChunkCoord::from_world(cam_transform.translation);

    load_chunks_around(
        player_chunk,
        &mut commands,
        &mut materials,
        &mut meshes,
        &mut loaded,
        &resource_pack,
        &atlas_handle.handle,
    );

    unload_distant_chunks(player_chunk, &mut commands, &mut loaded);
}

/// Loads all chunks within RENDER_DISTANCE of `center` that aren't already loaded.
fn load_chunks_around(
    center: ChunkCoord,
    commands: &mut Commands,
    materials: &mut Assets<StandardMaterial>,
    meshes: &mut Assets<Mesh>,
    loaded: &mut LoadedChunks,
    resource_pack: &ResourcePackManager,
    atlas_handle: &Handle<Image>,
) {
    let cy_min = -Y_LAYERS / 2;
    let cy_max = Y_LAYERS / 2;

    for dx in -RENDER_DISTANCE..=RENDER_DISTANCE {
        for dz in -RENDER_DISTANCE..=RENDER_DISTANCE {
            for cy in cy_min..=cy_max {
                let coord = ChunkCoord {
                    cx: center.cx + dx,
                    cy,
                    cz: center.cz + dz,
                };

                if loaded.entities.contains_key(&coord) {
                    continue;
                }

                let mut chunk = Chunk::filled(0);
                fill_terrain(&mut chunk);

                let position = coord.to_world_origin();
                let entity = spawn_chunk_entity(
                    commands,
                    materials,
                    meshes,
                    chunk,
                    position,
                    resource_pack,
                    atlas_handle,
                );

                commands
                    .entity(entity)
                    .insert(ChunkAtlasHandle(atlas_handle.clone()));

                loaded.entities.insert(coord, entity);
            }
        }
    }
}

/// Despawns chunks that are beyond UNLOAD_DISTANCE from the player.
fn unload_distant_chunks(center: ChunkCoord, commands: &mut Commands, loaded: &mut LoadedChunks) {
    let to_remove: Vec<ChunkCoord> = loaded
        .entities
        .keys()
        .filter(|coord| {
            let dx = (coord.cx - center.cx).abs();
            let dz = (coord.cz - center.cz).abs();
            dx > UNLOAD_DISTANCE || dz > UNLOAD_DISTANCE
        })
        .copied()
        .collect();

    for coord in to_remove {
        if let Some(entity) = loaded.entities.remove(&coord) {
            commands.entity(entity).despawn();
        }
    }
}
