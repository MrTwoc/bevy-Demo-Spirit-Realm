mod atlas;
mod camera;
mod chunk;
mod chunk_dirty;
mod chunk_wire_frame;
mod cube;
mod hud;
mod input;
mod raycast;

use crate::chunk_wire_frame::WireframeMode;
use bevy::{pbr::wireframe::WireframePlugin, prelude::*};

fn main() {
    App::new()
        .init_resource::<WireframeMode>()
        .init_resource::<raycast::RayHitState>()
        .add_plugins((DefaultPlugins, WireframePlugin::default()))
        .add_systems(
            Startup,
            (
                cube::setup_lighting,
                chunk::spawn_initial_chunks,
                hud::spawn_crosshair,
            ),
        )
        .add_systems(
            Update,
            (
                camera::camera_movement,
                camera::camera_rotation,
                input::cursor_grab_system,
                chunk_wire_frame::toggle_wireframe,
                chunk_wire_frame::sync_chunk_wireframe,
                chunk_wire_frame::draw_wireframes,
                chunk_dirty::rebuild_dirty_chunks,
                raycast::raycast_highlight_system,
                hud::update_hud,
            ),
        )
        .run();
}
