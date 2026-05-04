mod camera;
mod chunk;
mod chunk_wire_frame;
mod cube;
mod hud;
mod input;

use bevy::{pbr::wireframe::WireframePlugin, prelude::*};
use crate::chunk_wire_frame::WireframeMode;

fn main() {
    App::new()
        .init_resource::<WireframeMode>()
        .add_plugins((
            DefaultPlugins,
            WireframePlugin::default(),
        ))
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
                chunk_wire_frame::toggle_wireframe,
                chunk_wire_frame::sync_chunk_wireframe,
                chunk_wire_frame::draw_wireframes,
                hud::update_hud,
            ),
        )
        .run();
}
