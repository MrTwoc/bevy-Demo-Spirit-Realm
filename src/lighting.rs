//! Scene setup: directional sunlight + ambient.

use bevy::prelude::*;

/// Spawns a directional "sun" light and ambient light.
pub fn setup_lighting(mut commands: Commands) {
    // Directional light simulating sunlight — shines from upper-right at ~45°.
    commands.spawn((
        DirectionalLight {
            color: Color::srgb(1.0, 0.95, 0.8), // warm sunlight tint
            illuminance: 5000.0,                // bright enough for outdoor scene
            shadows_enabled: false,
            ..default()
        },
        Transform::from_rotation(Quat::from_euler(EulerRot::XYZ, -0.8, 0.4, 0.0)),
    ));

    // Soft ambient light so shadowed faces aren't pitch-black.
    commands.spawn(AmbientLight {
        color: Color::srgb(0.7, 0.75, 0.9), // cool blue-ish ambient
        brightness: 200.0,
        affects_lightmapped_meshes: true,
    });
}
