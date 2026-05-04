mod camera;
mod chunk;
mod cube;
mod hud;
mod input;

use bevy::prelude::*;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_systems(Startup, (cube::setup_lighting, chunk::spawn_initial_chunks))
        .add_systems(
            Update,
            (
                camera::camera_movement,
                camera::camera_rotation,
                input::cursor_grab_system,
                hud::update_hud,
            ),
        )
        .run();
}
