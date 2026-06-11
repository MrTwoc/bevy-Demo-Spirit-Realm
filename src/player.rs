//! Player entity: position, movement, and camera attachment.

use bevy::{core_pipeline::Skybox, prelude::*};

use crate::camera::CameraController;
use crate::character_controller::CharacterController;

/// The main player marker component.
#[derive(Component)]
pub struct Player;

/// Spawns the player entity with a Camera3d as a child.
/// Returns (player_entity, camera_entity) for HUD attachment.
pub fn spawn_player(commands: &mut Commands, initial_pos: Vec3) -> (Entity, Entity) {
    let camera_entity = commands.spawn_empty().id();

    let player_entity = commands
        .spawn((
            Player,
            CharacterController::default(),
            Transform::from_translation(initial_pos),
            Visibility::default(),
        ))
        .add_child(camera_entity)
        .id();

    (player_entity, camera_entity)
}

/// Inserts the camera components after resource loading is complete.
/// Called from setup_world once Skybox handle is ready.
pub fn insert_camera_components(
    commands: &mut Commands,
    camera_entity: Entity,
    skybox_image: Handle<Image>,
) {
    commands.entity(camera_entity).insert((
        Camera3d::default(),
        CameraController::default(),
        Skybox {
            image: skybox_image,
            brightness: 1000.0,
            ..default()
        },
    ));
}
