use bevy::{
    diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin},
    prelude::*,
    text::TextFont,
    ui::{BackgroundColor, Node, PositionType, UiRect, Val},
};

/// FPS 叠加层的配置资源。
/// 可通过修改此资源来控制 FPS 显示的外观和行为。
#[derive(Resource)]
pub struct FpsOverlayConfig {
    /// 是否显示 FPS 叠加层
    pub enabled: bool,
    /// 文字大小
    pub font_size: f32,
    /// 文字颜色
    pub text_color: Color,
    /// 背景颜色
    pub background_color: Color,
    /// 距离左上角的 X 偏移（像素）
    pub left_offset: f32,
    /// 距离左上角的 Y 偏移（像素），会自动计算在 HUD 面板下方
    pub top_offset: f32,
}

impl Default for FpsOverlayConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            font_size: 16.0,
            text_color: Color::WHITE,
            background_color: Color::BLACK.with_alpha(0.6),
            left_offset: 12.0,
            top_offset: 120.0,
        }
    }
}

/// FPS 文本标记组件
#[derive(Component)]
struct FpsText;

/// FPS 叠加层的根节点标记组件
#[derive(Component)]
struct FpsOverlayRoot;

/// FPS 显示插件
pub struct FpsOverlayPlugin;

impl Plugin for FpsOverlayPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FpsOverlayConfig>()
            .add_systems(PostStartup, spawn_fps_overlay)
            .add_systems(Update, (update_fps_text, toggle_fps_overlay));
    }
}

/// 生成 FPS 叠加层 UI
fn spawn_fps_overlay(
    mut commands: Commands,
    config: Res<FpsOverlayConfig>,
    camera_query: Query<Entity, With<Camera3d>>,
) {
    if !config.enabled {
        return;
    }

    // 获取相机实体以设置 UiTargetCamera
    let camera_entity = camera_query.single().ok();

    let mut entity_cmds = commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(config.left_offset),
            top: Val::Px(config.top_offset),
            padding: UiRect::new(Val::Px(8.0), Val::Px(8.0), Val::Px(4.0), Val::Px(4.0)),
            ..default()
        },
        BackgroundColor(config.background_color),
        FpsOverlayRoot,
    ));

    // 如果找到相机实体，绑定到该相机的 UI 层
    if let Some(cam) = camera_entity {
        entity_cmds.insert(UiTargetCamera(cam));
    }

    entity_cmds.with_children(|parent| {
        parent.spawn((
            Text::new("FPS: --"),
            TextFont {
                font_size: config.font_size,
                ..default()
            },
            TextColor(config.text_color),
            FpsText,
        ));
    });
}

/// 更新 FPS 文本内容
fn update_fps_text(diagnostics: Res<DiagnosticsStore>, mut query: Query<&mut Text, With<FpsText>>) {
    let Ok(mut text) = query.single_mut() else {
        return;
    };

    if let Some(fps) = diagnostics.get(&FrameTimeDiagnosticsPlugin::FPS) {
        if let Some(value) = fps.smoothed() {
            **text = format!("FPS: {:.0}", value);
        }
    }
}

/// 根据配置切换 FPS 叠加层的显示/隐藏
fn toggle_fps_overlay(
    config: Res<FpsOverlayConfig>,
    mut query: Query<&mut Visibility, With<FpsOverlayRoot>>,
) {
    if !config.is_changed() {
        return;
    }

    if let Ok(mut visibility) = query.single_mut() {
        *visibility = if config.enabled {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }
}
