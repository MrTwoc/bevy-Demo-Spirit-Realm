mod block_interaction;
mod camera;
mod chunk;
mod chunk_dirty;
mod chunk_wire_frame;
mod fps_overlay;
mod hud;
mod input;
mod lighting;
mod raycast;

use crate::chunk_wire_frame::WireframeMode;
use bevy::{
    diagnostic::FrameTimeDiagnosticsPlugin, pbr::wireframe::WireframePlugin, prelude::*,
    render::diagnostic::RenderDiagnosticsPlugin,
};

fn main() {
    App::new()
        .init_resource::<WireframeMode>()
        .init_resource::<raycast::RayHitState>()
        .insert_resource(hud::TriangleUpdateTimer(Timer::from_seconds(
            1.0,
            TimerMode::Repeating,
        )))
        .add_plugins((
            DefaultPlugins,
            WireframePlugin::default(),
            FrameTimeDiagnosticsPlugin::default(),
            RenderDiagnosticsPlugin,
            fps_overlay::FpsOverlayPlugin,
        ))
        .add_systems(
            Startup,
            (lighting::setup_lighting, chunk::spawn_initial_chunks),
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
                block_interaction::block_interaction_system,
                hud::update_hud,
                hud::update_triangle_count,
            ),
        )
        .run();
}
