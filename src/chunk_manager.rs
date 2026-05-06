//! Chunk manager: loads chunks around the player and unloads distant ones.

use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use std::collections::HashMap;

use crate::chunk::{Chunk, ChunkCoord, ChunkNeighbors, fill_terrain, spawn_chunk_entity};
use crate::chunk_dirty::{ChunkAtlasHandle, ChunkCoordComponent};
use crate::resource_pack::ResourcePackManager;

pub const RENDER_DISTANCE: i32 = 2;
pub const UNLOAD_DISTANCE: i32 = RENDER_DISTANCE;

/// 已加载区块的条目，包含实体和区块数据
pub struct ChunkEntry {
    pub entity: Entity,
    pub data: Chunk,
}

#[derive(Resource, Default)]
pub struct LoadedChunks {
    pub entries: HashMap<ChunkCoord, ChunkEntry>,
}

pub const Y_LAYERS: i32 = 1;

/// 存储 Atlas 纹理句柄的资源
#[derive(Resource)]
pub struct AtlasTextureHandle {
    pub handle: Handle<Image>,
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

                if loaded.entries.contains_key(&coord) {
                    continue;
                }

                let mut chunk = Chunk::filled(0);
                fill_terrain(&mut chunk);

                // 收集邻居数据用于跨区块面剔除
                let neighbors = collect_neighbors(coord, loaded);

                let position = coord.to_world_origin();
                let entity = spawn_chunk_entity(
                    commands,
                    materials,
                    meshes,
                    chunk.clone(),
                    position,
                    resource_pack,
                    atlas_handle,
                    &neighbors,
                );

                commands.entity(entity).insert((
                    ChunkAtlasHandle(atlas_handle.clone()),
                    ChunkCoordComponent(coord),
                ));

                loaded.entries.insert(
                    coord,
                    ChunkEntry {
                        entity,
                        data: chunk,
                    },
                );
            }
        }
    }
}

/// Despawns chunks that are beyond UNLOAD_DISTANCE from the player.
fn unload_distant_chunks(center: ChunkCoord, commands: &mut Commands, loaded: &mut LoadedChunks) {
    let to_remove: Vec<ChunkCoord> = loaded
        .entries
        .keys()
        .filter(|coord| {
            let dx = (coord.cx - center.cx).abs();
            let dz = (coord.cz - center.cz).abs();
            dx > UNLOAD_DISTANCE || dz > UNLOAD_DISTANCE
        })
        .copied()
        .collect();

    for coord in to_remove {
        if let Some(entry) = loaded.entries.remove(&coord) {
            commands.entity(entry.entity).despawn();
        }
    }
}
