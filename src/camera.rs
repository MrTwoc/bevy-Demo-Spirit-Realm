//! Camera controller: rotation only. Movement is handled by the Player entity.

use bevy::{
    ecs::message::MessageReader,
    input::mouse::MouseMotion,
    prelude::*,
    window::{CursorGrabMode, CursorOptions},
};

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
