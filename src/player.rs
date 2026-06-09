//! Player entity: position, movement, and camera attachment.

use bevy::{core_pipeline::Skybox, prelude::*};

use crate::camera::CameraController;

/// Movement speed for the player in units per second.
pub const PLAYER_MOVE_SPEED: f32 = 30.0;

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

/// Handles WASD + Space/Shift player movement. Ctrl accelerates speed 3x.
/// Only moves when the cursor is locked (pointer grab active).
/// Movement direction is derived from the Camera child's orientation.
pub fn player_movement(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    cursor_options: Single<&bevy::window::CursorOptions>,
    camera_query: Query<&GlobalTransform, With<Camera3d>>,
    mut player_query: Query<&mut Transform, With<Player>>,
) {
    use bevy::window::CursorGrabMode;

    if cursor_options.grab_mode != CursorGrabMode::Locked {
        return;
    }

    let Ok(mut transform) = player_query.single_mut() else {
        return;
    };

    let mut movement = Vec3::ZERO;

    if keys.pressed(KeyCode::KeyW) {
        movement.z += 1.0;
    }
    if keys.pressed(KeyCode::KeyS) {
        movement.z -= 1.0;
    }
    if keys.pressed(KeyCode::KeyA) {
        movement.x -= 1.0;
    }
    if keys.pressed(KeyCode::KeyD) {
        movement.x += 1.0;
    }
    if keys.pressed(KeyCode::Space) {
        movement.y += 1.0;
    }
    if keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight) {
        movement.y -= 1.0;
    }

    let speed_multiplier =
        if keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight) {
            3.0
        } else {
            1.0
        };

    if movement != Vec3::ZERO {
        let normalized_movement = movement.normalize();

        // Use camera's world-space forward for movement direction.
        let Ok(cam_gt) = camera_query.single() else {
            return;
        };
        let forward = cam_gt.forward();
        let right = cam_gt.right();
        let horizontal_forward = Vec3::new(forward.x, 0.0, forward.z).normalize_or_zero();
        let horizontal_right = Vec3::new(right.x, 0.0, right.z).normalize_or_zero();

        let delta = (horizontal_forward * normalized_movement.z
            + horizontal_right * normalized_movement.x
            + Vec3::Y * normalized_movement.y)
            * PLAYER_MOVE_SPEED
            * speed_multiplier
            * time.delta_secs();

        transform.translation += delta;
    }
}
