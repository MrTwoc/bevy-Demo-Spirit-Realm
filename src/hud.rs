use bevy::{
    diagnostic::{DiagnosticPath, DiagnosticsStore},
    prelude::*,
    text::TextFont,
    time::Timer,
    ui::{BackgroundColor, Node, PositionType, UiRect, UiTargetCamera, Val},
};

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
pub(crate) struct HudText;

#[derive(Component)]
pub(crate) struct TriangleCountText;

#[derive(Resource)]
pub struct TriangleUpdateTimer(pub Timer);

pub fn setup_hud(mut commands: Commands, camera_entity: Entity) {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(12.0),
                top: Val::Px(12.0),
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
                HudText,
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
        });

    // Spawn crosshair tied to the same camera
    spawn_crosshair(&mut commands, camera_entity);
}

pub fn update_hud(
    query: Query<&Transform, With<Camera3d>>,
    mut text_query: Query<&mut Text, With<HudText>>,
    hit_state: Res<crate::raycast::RayHitState>,
) {
    let Ok(transform) = query.single() else {
        return;
    };
    let Ok(mut text) = text_query.single_mut() else {
        return;
    };

    let p = transform.translation;
    if let Some(pos) = &hit_state.hit_pos {
        **text = format!(
            "xyz: {:.1}, {:.1}, {:.1}\nTarget: ({}, {}, {})",
            p.x, p.y, p.z, pos.x, pos.y, pos.z
        );
    } else {
        **text = format!("xyz: {:.1}, {:.1}, {:.1}", p.x, p.y, p.z);
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
