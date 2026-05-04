use bevy::{
    prelude::*,
    text::TextFont,
    ui::{BackgroundColor, Node, PositionType, UiTargetCamera, Val, UiRect},
};

/// Spawns a white Minecraft-style crosshair centered on screen.
pub fn spawn_crosshair(mut commands: Commands) {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: Val::Percent(50.0),
                top: Val::Percent(50.0),
                ..default()
            },
        ))
        .with_children(|parent| {
            // Horizontal bar (20×2px), centered via flex on the crosshair point.
            parent.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    width: Val::Px(20.0),
                    height: Val::Px(2.0),
                    // Place center of this bar at parent's top-left (the crosshair center).
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
                    // Place center of this bar at parent's top-left (the crosshair center).
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
    });
}

pub fn update_hud(
    query: Query<&Transform, With<Camera3d>>,
    mut text_query: Query<&mut Text, With<HudText>>,
) {
    let Ok(transform) = query.single() else {
        return;
    };
    let Ok(mut text) = text_query.single_mut() else {
        return;
    };

    let p = transform.translation;
    **text = format!("xyz: {:.1}, {:.1}, {:.1}", p.x, p.y, p.z);
}
