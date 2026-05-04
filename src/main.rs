mod camera;
mod chunk;
mod cube;
mod hud;
mod input;
mod wireframe;

use bevy::prelude::*;
use crate::wireframe::WireframeMode;

fn main() {
    App::new()
        .init_resource::<WireframeMode>()
        .add_plugins(DefaultPlugins)
        .add_systems(Startup, (
            cube::setup_lighting,
            chunk::spawn_initial_chunks,
            hud::spawn_crosshair,
        ))
        .add_systems(
            Update,
            (
                camera::camera_movement,
                camera::camera_rotation,
                input::cursor_grab_system,
                wireframe::toggle_wireframe,
                wireframe::draw_wireframes,
                hud::update_hud,
            ),
        )
        .run();
}
