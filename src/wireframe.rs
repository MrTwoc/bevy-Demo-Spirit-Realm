//! Wireframe mode toggle — press V to switch between normal rendering
//! and wireframe overlay drawn via bevy_gizmos.

use bevy::{gizmos::gizmos::Gizmos, prelude::*};

use crate::chunk::{self, Chunk};

/// Tracks the current wireframe mode state.
#[derive(Resource, Default)]
pub struct WireframeMode(pub bool);

/// Press V to toggle wireframe on/off.
/// When enabled, draws wireframe boxes around all Chunk entities.
pub fn toggle_wireframe(keyboard: Res<ButtonInput<KeyCode>>, mut mode: ResMut<WireframeMode>) {
    if keyboard.just_pressed(KeyCode::KeyV) {
        mode.0 = !mode.0;
    }
}

/// Draws wireframe boxes for all Chunk entities when wireframe mode is active.
pub fn draw_wireframes(
    mode: Res<WireframeMode>,
    chunks: Query<(&Chunk, &Transform)>,
    mut gizmos: Gizmos,
) {
    if !mode.0 {
        return;
    }

    let size = chunk::CHUNK_SIZE as f32;
    let color = Color::WHITE.with_alpha(0.6);

    for (_, transform) in &chunks {
        // gizmos.cube: Transform.translation is the CENTER of the box.
        // For a 32x32x32 box, center = chunk_origin + (16, 16, 16).
        let center = transform.translation + Vec3::splat(size / 2.0);
        let gizmo_transform =
            Transform::from_translation(center).with_scale(Vec3::splat(size));
        gizmos.cube(gizmo_transform, color);
    }
}
