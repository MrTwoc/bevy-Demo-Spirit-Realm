//! Scene setup: point light.

use bevy::prelude::*;

/// Spawns scene lighting.
pub fn setup_lighting(mut commands: Commands) {
    let light_transform = Transform::from_xyz(1.8, 30.8, 1.8).looking_at(Vec3::ZERO, Vec3::Y);
    commands.spawn((PointLight::default(), light_transform));
}
