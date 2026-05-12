use bevy::{
    diagnostic::{DiagnosticPath, DiagnosticsStore},
    prelude::*,
    text::TextFont,
    time::Timer,
    ui::{BackgroundColor, Node, PositionType, UiRect, UiTargetCamera, Val},
};
use sysinfo::{Pid, System};

use crate::chunk_manager::LoadedChunks;

/// Spawns a white Minecraft-style crosshair centered on screen.
/// Must be called with a valid camera entity so the UI targets the correct camera.
pub fn spawn_crosshair(commands: &mut Commands, camera_entity: Entity) {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Percent(50.0),
                top: Val::Percent(50.0),
                ..default()
            },
            UiTargetCamera(camera_entity),
        ))
        .with_children(|parent| {
            // Horizontal bar (20×2px), centered via flex on the crosshair point.
            parent.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    width: Val::Px(20.0),
                    height: Val::Px(2.0),
                    left: Val::Px(-10.0),
                    top: Val::Px(-1.0),
                    ..default()
                },
                BackgroundColor(Color::WHITE.with_alpha(0.9).into()),
            ));
            // Vertical bar (2×20px), centered via flex on the crosshair point.
            parent.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    width: Val::Px(2.0),
                    height: Val::Px(20.0),
                    left: Val::Px(-1.0),
                    top: Val::Px(-10.0),
                    ..default()
                },
                BackgroundColor(Color::WHITE.with_alpha(0.9).into()),
            ));
        });
}

#[derive(Component)]
pub(crate) struct PositionText;

#[derive(Component)]
pub(crate) struct TargetText;

#[derive(Component)]
pub(crate) struct TriangleCountText;

#[derive(Component)]
pub(crate) struct ChunkCountText;

#[derive(Component)]
pub(crate) struct ViewDistanceText;

#[derive(Resource)]
pub struct TriangleUpdateTimer(pub Timer);

pub fn setup_hud(commands: &mut Commands, camera_entity: Entity) {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(12.0),
                top: Val::Px(12.0),
                flex_direction: FlexDirection::Column,
                padding: UiRect::new(Val::Px(8.0), Val::Px(8.0), Val::Px(6.0), Val::Px(6.0)),
                ..default()
            },
            BackgroundColor(Color::BLACK.with_alpha(0.6)),
            UiTargetCamera(camera_entity),
        ))
        .with_children(|parent| {
            parent.spawn((
                Text::new("xyz: 0.0, 0.0, 0.0"),
                TextFont {
                    font_size: 16.0,
                    ..default()
                },
                TextColor(Color::WHITE),
                PositionText,
            ));
            parent.spawn((
                Text::new("Target: --"),
                TextFont {
                    font_size: 16.0,
                    ..default()
                },
                TextColor(Color::WHITE),
                TargetText,
            ));
            parent.spawn((
                Text::new("Triangle Count: --"),
                TextFont {
                    font_size: 16.0,
                    ..default()
                },
                TextColor(Color::WHITE),
                TriangleCountText,
            ));
            parent.spawn((
                Text::new("Chunks: 0"),
                TextFont {
                    font_size: 16.0,
                    ..default()
                },
                TextColor(Color::WHITE),
                ChunkCountText,
            ));
            parent.spawn((
                Text::new("view-distance: 0"),
                TextFont {
                    font_size: 16.0,
                    ..default()
                },
                TextColor(Color::WHITE),
                ViewDistanceText,
            ));
        });

    // Spawn crosshair tied to the same camera
    spawn_crosshair(commands, camera_entity);
}

pub fn update_hud(
    query: Query<&Transform, With<Camera3d>>,
    mut pos_query: Query<&mut Text, (With<PositionText>, Without<TargetText>)>,
    mut target_query: Query<&mut Text, (With<TargetText>, Without<PositionText>)>,
    hit_state: Res<crate::raycast::RayHitState>,
) {
    let Ok(transform) = query.single() else {
        return;
    };

    let p = transform.translation;
    if let Ok(mut text) = pos_query.single_mut() {
        **text = format!("xyz: {:.1}, {:.1}, {:.1}", p.x, p.y, p.z);
    }
    if let Ok(mut text) = target_query.single_mut() {
        if let Some(pos) = &hit_state.hit_pos {
            **text = format!("Target: ({}, {}, {})", pos.x, pos.y, pos.z);
        } else {
            **text = "Target: --".to_string();
        }
    }
}

/// 已知的渲染三角形诊断路径（Bevy 0.18.1 RenderDiagnosticsPlugin 动态生成）。
/// 每个渲染 Pass 会生成类似 `render_pass/{pass_name}/triangles_primitives_in` 的路径。
const KNOWN_TRIANGLE_PATHS: &[&str] = &[
    "render_pass/main_opaque_pass_3d/triangles_primitives_in",
    "render_pass/main_transparent_pass_3d/triangles_primitives_in",
    "render_pass/shadows/triangles_primitives_in",
];

pub fn update_triangle_count(
    time: Res<Time>,
    mut timer: ResMut<TriangleUpdateTimer>,
    diagnostics: Res<DiagnosticsStore>,
    meshes: Res<Assets<Mesh>>,
    mesh_query: Query<&Mesh3d>,
    mut text_query: Query<&mut Text, With<TriangleCountText>>,
) {
    timer.0.tick(time.delta());
    if !timer.0.just_finished() {
        return;
    }

    // 优先尝试从 RenderDiagnosticsPlugin 获取 GPU 实际渲染的三角形数
    let mut gpu_triangles: Option<f64> = None;
    for path_str in KNOWN_TRIANGLE_PATHS {
        let path = DiagnosticPath::new(*path_str);
        if let Some(diag) = diagnostics.get(&path) {
            if let Some(value) = diag.smoothed() {
                *gpu_triangles.get_or_insert(0.0) += value;
            }
        }
    }

    let display_text = if let Some(value) = gpu_triangles {
        format!("Triangle Count(GPU): {:.0}", value)
    } else {
        // 回退：统计 Mesh 数据中的三角形数
        let total: u32 = mesh_query
            .iter()
            .map(|h| {
                meshes.get(&h.0).map_or(0, |mesh| match mesh.indices() {
                    Some(indices) => indices.len() as u32 / 3,
                    None => mesh.count_vertices() as u32 / 3,
                })
            })
            .sum();
        format!("Triangle Count: {}", total)
    };

    if let Ok(mut text) = text_query.single_mut() {
        **text = display_text;
    }
}

/// 每帧更新 HUD 中显示的已加载区块数量。
pub fn update_chunk_count(
    loaded: Res<LoadedChunks>,
    mut text_query: Query<&mut Text, With<ChunkCountText>>,
) {
    if let Ok(mut text) = text_query.single_mut() {
        **text = format!("Chunks: {}", loaded.entries.len());
    }
}

/// 每帧更新 HUD 中显示的视距。
pub fn update_view_distance(mut text_query: Query<&mut Text, With<ViewDistanceText>>) {
    if let Ok(mut text) = text_query.single_mut() {
        **text = format!("view-distance: {}", crate::chunk_manager::RENDER_DISTANCE);
    }
}

// ============================================================================
// 硬件信息 HUD（右上角）
// ============================================================================

/// 标记 CPU 名称文本的组件。
#[derive(Component)]
pub(crate) struct CpuInfoText;

/// 标记 CPU 使用率文本的组件。
#[derive(Component)]
pub(crate) struct CpuUsageText;

/// 标记 GPU 信息文本的组件。
#[derive(Component)]
pub(crate) struct GpuInfoText;

/// 标记内存信息文本的组件。
#[derive(Component)]
pub(crate) struct MemoryInfoText;

/// 硬件信息刷新定时器。
#[derive(Resource)]
pub struct HardwareInfoTimer(pub Timer);

/// 硬件信息资源，缓存 sysinfo::System 以避免每帧重建。
#[derive(Resource)]
pub struct HardwareInfo {
    pub system: System,
    pub current_pid: Pid,
    pub cpu_name: String,
    pub gpu_name: String,
}

impl Default for HardwareInfo {
    fn default() -> Self {
        let mut system = System::new_all();
        system.refresh_all();

        // 获取当前进程 PID
        let current_pid = Pid::from_u32(std::process::id());

        // 提取 CPU 名称
        let cpu_name = system
            .cpus()
            .first()
            .map(|cpu| cpu.brand().to_string())
            .unwrap_or_else(|| "Unknown CPU".to_string());

        // GPU 名称（sysinfo 不直接提供 GPU 信息，使用占位符）
        let gpu_name = "Detecting...".to_string();

        Self {
            system,
            current_pid,
            cpu_name,
            gpu_name,
        }
    }
}

/// 在屏幕右上角生成硬件信息 HUD。
/// 必须传入有效的 camera entity 以确保 UI 绑定到正确的相机。
pub fn setup_hardware_info_hud(commands: &mut Commands, camera_entity: Entity) {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                right: Val::Px(12.0),
                top: Val::Px(12.0),
                flex_direction: FlexDirection::Column,
                padding: UiRect::new(Val::Px(8.0), Val::Px(8.0), Val::Px(6.0), Val::Px(6.0)),
                ..default()
            },
            BackgroundColor(Color::BLACK.with_alpha(0.6)),
            UiTargetCamera(camera_entity),
        ))
        .with_children(|parent| {
            // 标题
            parent.spawn((
                Text::new("── Hardware Info ──"),
                TextFont {
                    font_size: 14.0,
                    ..default()
                },
                TextColor(Color::srgb(0.6, 0.9, 1.0)), // 淡蓝色标题
            ));
            // CPU 型号
            parent.spawn((
                Text::new("CPU: Loading..."),
                TextFont {
                    font_size: 13.0,
                    ..default()
                },
                TextColor(Color::WHITE),
                CpuInfoText,
            ));
            // CPU 使用率
            parent.spawn((
                Text::new("CPU Usage: --%"),
                TextFont {
                    font_size: 13.0,
                    ..default()
                },
                TextColor(Color::WHITE),
                CpuUsageText,
            ));
            // GPU 信息
            parent.spawn((
                Text::new("GPU: Loading..."),
                TextFont {
                    font_size: 13.0,
                    ..default()
                },
                TextColor(Color::WHITE),
                GpuInfoText,
            ));
            // 内存信息
            parent.spawn((
                Text::new("RAM: Loading..."),
                TextFont {
                    font_size: 13.0,
                    ..default()
                },
                TextColor(Color::WHITE),
                MemoryInfoText,
            ));
        });
}

/// 定期刷新硬件信息并更新 HUD 文本。
#[allow(clippy::type_complexity)]
pub fn update_hardware_info(
    time: Res<Time>,
    mut timer: ResMut<HardwareInfoTimer>,
    mut hw_info: ResMut<HardwareInfo>,
    mut cpu_name_query: Query<
        &mut Text,
        (
            With<CpuInfoText>,
            Without<CpuUsageText>,
            Without<GpuInfoText>,
            Without<MemoryInfoText>,
        ),
    >,
    mut cpu_usage_query: Query<
        &mut Text,
        (
            With<CpuUsageText>,
            Without<CpuInfoText>,
            Without<GpuInfoText>,
            Without<MemoryInfoText>,
        ),
    >,
    mut gpu_query: Query<
        &mut Text,
        (
            With<GpuInfoText>,
            Without<CpuInfoText>,
            Without<CpuUsageText>,
            Without<MemoryInfoText>,
        ),
    >,
    mut mem_query: Query<
        &mut Text,
        (
            With<MemoryInfoText>,
            Without<CpuInfoText>,
            Without<CpuUsageText>,
            Without<GpuInfoText>,
        ),
    >,
) {
    timer.0.tick(time.delta());
    if !timer.0.just_finished() {
        return;
    }

    // 刷新系统信息和当前进程信息
    hw_info.system.refresh_all();

    // CPU 名称（静态信息，仅首次有意义）
    if let Ok(mut text) = cpu_name_query.single_mut() {
        **text = format!("CPU: {}", hw_info.cpu_name);
    }

    // 当前进程 CPU 使用率
    let proc_cpu = hw_info
        .system
        .process(hw_info.current_pid)
        .map(|p| p.cpu_usage())
        .unwrap_or(0.0);
    if let Ok(mut text) = cpu_usage_query.single_mut() {
        **text = format!("Process CPU: {:.1}%", proc_cpu);
    }

    // GPU 信息（静态）
    if let Ok(mut text) = gpu_query.single_mut() {
        **text = format!("GPU: {}", hw_info.gpu_name);
    }

    // 当前进程内存使用
    if let Some(proc) = hw_info.system.process(hw_info.current_pid) {
        let proc_mem_mb = proc.memory() as f64 / (1024.0 * 1024.0);
        let total_sys_mem_gb = hw_info.system.total_memory() as f64 / (1024.0 * 1024.0 * 1024.0);
        if let Ok(mut text) = mem_query.single_mut() {
            **text = format!(
                "Process RAM: {:.1} MB (System {:.1} GB)",
                proc_mem_mb, total_sys_mem_gb
            );
        }
    }
}
