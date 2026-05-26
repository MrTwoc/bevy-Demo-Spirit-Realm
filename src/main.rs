mod async_mesh;
mod block_interaction;
mod camera;
mod chunk;
mod chunk_changes;
mod chunk_dirty;
mod chunk_manager;
mod chunk_wire_frame;
mod greedy_mesh;
mod hud;
mod input;
mod lighting;
mod lod;
mod perf_logger;
mod raycast;
mod resource_pack;
mod svo;
mod tree_gen;
mod voxel_render;

use crate::chunk_wire_frame::WireframeMode;
use bevy::{
    diagnostic::FrameTimeDiagnosticsPlugin,
    ecs::schedule::common_conditions::resource_changed,
    pbr::wireframe::WireframePlugin, prelude::*,
    render::diagnostic::RenderDiagnosticsPlugin,
};
use resource_pack::VoxelMaterial;

fn main() {
    App::new()
        .insert_resource(ClearColor(Color::srgb(0.53, 0.81, 0.92))) // 天空蓝背景
        .init_resource::<WireframeMode>()
        .init_resource::<raycast::RayHitState>()
        .init_resource::<chunk_manager::LoadedChunks>()
        .init_resource::<lod::LodManager>()
        .init_resource::<hud::HardwareInfo>()
        .init_resource::<hud::CachedTriangleCount>()
        .insert_resource(hud::TriangleUpdateTimer(Timer::from_seconds(
            0.1,
            TimerMode::Repeating,
        )))
        .insert_resource(hud::HardwareInfoTimer(Timer::from_seconds(
            2.0,
            TimerMode::Repeating,
        )))
        .init_resource::<tree_gen::TreeConfig>()
        .insert_resource(tree_gen::TreeNoise::default())
        .add_plugins((
            DefaultPlugins,
            WireframePlugin::default(),
            FrameTimeDiagnosticsPlugin::default(),
            RenderDiagnosticsPlugin,
            perf_logger::PerfLoggerPlugin,
            resource_pack::ResourcePackPlugin,
            MaterialPlugin::<VoxelMaterial>::default(),
            svo::SvoPlugin,
        ))
        .add_systems(
            Startup,
            (
                resource_pack::load_resource_pack_system,
                lighting::setup_lighting,
                chunk_manager::setup_world,
            )
                .chain(),
        )
        .add_systems(Startup, raycast::setup_highlight_resources)
        // view-distance 是静态常量，只需在 Startup 写入一次
        .add_systems(Startup, hud::update_view_distance)
        .add_systems(First, chunk_manager::chunk_loader_system)
        .add_systems(
            Update,
            (
                camera::camera_movement,
                camera::camera_rotation,
                input::cursor_grab_system,
                chunk_wire_frame::toggle_wireframe,
                // 线框同步：仅在 WireframeMode 变更时运行（按 V 切换时触发）
                chunk_wire_frame::sync_chunk_wireframe
                    .run_if(resource_changed::<WireframeMode>),
                // 线框绘制：仅在线框模式开启时运行
                chunk_wire_frame::draw_wireframes
                    .run_if(|mode: Res<WireframeMode>| mode.0),
                chunk_dirty::rebuild_dirty_chunks,
                raycast::raycast_highlight_system,
                block_interaction::block_interaction_system,
                hud::update_hud,
                hud::update_triangle_count,
                // merge FPS 更新到硬件信息定时器间隔（2秒）
                hud::update_fps_and_hardware_info,
            ),
        )
        // 区块数量仅在实际变化时更新
        .add_systems(Update, hud::update_chunk_count.run_if(hud::chunk_count_changed))
        // 世界类型仅在变更时更新
        .add_systems(
            Update,
            hud::update_world_type
                .run_if(resource_changed::<crate::chunk::WorldTypeResource>),
        )
        .run();
}
