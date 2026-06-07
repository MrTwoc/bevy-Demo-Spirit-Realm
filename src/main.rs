mod async_mesh;
mod biome;
mod block_interaction;
mod camera;
mod spline;
mod terrain_noise;
mod chunk;
mod chunk_changes;
mod chunk_dirty;
mod chunk_manager;
mod chunk_wire_frame;
mod compact_vertex;
mod greedy_mesh;
mod hud;
mod input;
mod lighting;
mod lod;
mod perf_logger;
mod raycast;
mod resource_pack;
mod skybox;
mod svo;
mod tree_gen;

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
        .init_resource::<hud::DebugHudVisible>()
        .insert_resource(hud::TriangleUpdateTimer(Timer::from_seconds(
            0.1,
            TimerMode::Repeating,
        )))
        .insert_resource(hud::HardwareInfoTimer(Timer::from_seconds(
            0.5,
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
                skybox::setup_skybox,
            )
                .chain(),
        )
        .add_systems(Startup, raycast::setup_highlight_resources)
        // view-distance 是静态常量，只需在 Startup 写入一次
        .add_systems(Startup, hud::update_view_distance)
        .add_systems(
            First,
            (
                // GPU 上传优先于渲染，减少帧尾延迟
                chunk_manager::collect_and_upload_meshes,
                // 分帧处理待删除的区块实体
                chunk_manager::process_pending_deletions
                    .run_if(chunk_manager::has_pending_deletions),
            )
                .chain(),
        )
        // ── 天空盒：等待纹理加载完成后重解释 PNG 为 CubeMap ──
        .add_systems(Update, skybox::asset_loaded)
        // ── 相机/输入：不依赖 LoadedChunks，与区块管道完全并行 ──
        .add_systems(
            Update,
            (
                camera::camera_movement,
                camera::camera_rotation,
                input::cursor_grab_system,
                input::toggle_debug_hud,
            )
                .chain(),
        )
        // ── 渲染辅助：轻量级，无 LoadedChunks 依赖 ──
        .add_systems(
            Update,
            (
                chunk_wire_frame::toggle_wireframe,
                chunk_wire_frame::sync_chunk_wireframe
                    .run_if(resource_changed::<WireframeMode>),
                chunk_wire_frame::draw_wireframes
                    .run_if(|mode: Res<WireframeMode>| mode.0),
                raycast::raycast_highlight_system,
                block_interaction::block_interaction_system,
            ),
        )
        // ── 区块生命周期管道：共享 ResMut<LoadedChunks>，必须串行 ──
        // chain 保证脏块重建在新加载区块之后执行（需要邻居数据）
        // submit_prepare_tasks 已合并到 manage_chunk_load_state 末尾，减少一次系统调度
        .add_systems(
            Update,
            (
                chunk_manager::manage_chunk_load_state,
                chunk_manager::spawn_entities_from_prepare
                    .run_if(chunk_manager::has_pending_prepare_results),
                chunk_dirty::rebuild_dirty_chunks,
            )
                .chain(),
        )
        // ── HUD：只读 LoadedChunks，与区块管道可并行 ──
        .add_systems(
            Update,
            (
                hud::update_hud,
                hud::update_triangle_count,
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
