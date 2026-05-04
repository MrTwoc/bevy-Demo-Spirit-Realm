//! Camera controller: position, rotation, and WASD movement.

use bevy::{
    ecs::message::MessageReader,
    input::mouse::MouseMotion,
    prelude::*,
    window::{CursorGrabMode, CursorOptions},
};

/// Movement speed for the camera in units per second.
pub const CAMERA_MOVE_SPEED: f32 = 5.0;

/// Mouse sensitivity for looking around.
pub const MOUSE_SENSITIVITY: f32 = 0.002;

/// Component storing the camera's rotation state (pitch and yaw).
#[derive(Component)]
pub struct CameraController {
    /// Vertical angle (up/down), clamped to prevent flipping.
    pub pitch: f32,
    /// Horizontal angle (left/right).
    pub yaw: f32,
}

impl Default for CameraController {
    fn default() -> Self {
        Self {
            pitch: -0.3,
            yaw: -0.8,
        }
    }
}

/// Handles WASD + Space/Shift camera movement.
pub fn camera_movement(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mut query: Query<(&mut Transform, &CameraController), With<Camera3d>>,
) {
    let Ok((mut transform, _controller)) = query.single_mut() else {
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

    if movement != Vec3::ZERO {
        let normalized_movement = movement.normalize();

        // Project onto XZ plane so movement stays ground-relative.
        let forward = transform.forward();
        let right = transform.right();
        let horizontal_forward = Vec3::new(forward.x, 0.0, forward.z).normalize();
        let horizontal_right = Vec3::new(right.x, 0.0, right.z).normalize();

        let delta = (horizontal_forward * normalized_movement.z
            + horizontal_right * normalized_movement.x
            + Vec3::Y * normalized_movement.y)
            * CAMERA_MOVE_SPEED
            * time.delta_secs();

        transform.translation += delta;
    }
}

/// Handles mouse look by consuming MouseMotion events.
/// Only rotates when the cursor is locked (pointer grab active).
pub fn camera_rotation(
    mut mouse_motion: MessageReader<MouseMotion>,
    cursor_options: Single<&CursorOptions>,
    mut query: Query<(&mut Transform, &mut CameraController), With<Camera3d>>,
) {
    // Only rotate when cursor is locked.
    if cursor_options.grab_mode != CursorGrabMode::Locked {
        return;
    }

    let Ok((mut transform, mut controller)) = query.single_mut() else {
        return;
    };

    for event in mouse_motion.read() {
        controller.yaw -= event.delta.x * MOUSE_SENSITIVITY;
        controller.pitch -= event.delta.y * MOUSE_SENSITIVITY;

        controller.pitch = controller.pitch.clamp(-1.54, 1.54);

        transform.rotation = Quat::from_euler(EulerRot::YXZ, controller.yaw, controller.pitch, 0.0);
    }
}
