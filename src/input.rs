//! Input handling: cursor grab (pointer lock) and debug HUD toggle.

use bevy::{
    prelude::*,
    window::{CursorGrabMode, CursorOptions},
};

use crate::camera::ViewMode;
use crate::hud::{DebugHudLeftPanel, DebugHudRightPanel, DebugHudVisible};
use crate::player::PlayerModel;

/// Toggles cursor lock on ESC (Minecraft-style: ESC toggles locked↔free).
/// Left-click is no longer used for grabbing — only ESC controls the lock state.
pub fn cursor_grab_system(
    mut cursor_options: Single<&mut CursorOptions>,
    key: Res<ButtonInput<KeyCode>>,
) {
    if key.just_pressed(KeyCode::Escape) {
        match cursor_options.grab_mode {
            CursorGrabMode::Locked => {
                // Unlock: show cursor, stop capturing
                cursor_options.visible = true;
                cursor_options.grab_mode = CursorGrabMode::None;
            }
            CursorGrabMode::None | CursorGrabMode::Confined => {
                // Lock: hide cursor, capture it
                cursor_options.visible = false;
                cursor_options.grab_mode = CursorGrabMode::Locked;
            }
        }
    }
}

/// 按 F3 键切换调试 HUD 面板（左侧信息面板 + 右侧硬件信息面板）的显示/隐藏。
pub fn toggle_debug_hud(
    key: Res<ButtonInput<KeyCode>>,
    mut visible: ResMut<DebugHudVisible>,
    mut queries: bevy::ecs::system::ParamSet<(
        Query<'static, 'static, &'static mut Visibility, With<DebugHudLeftPanel>>,
        Query<'static, 'static, &'static mut Visibility, With<DebugHudRightPanel>>,
    )>,
) {
    if !key.just_pressed(KeyCode::F3) {
        return;
    }

    visible.0 = !visible.0;
    let new_visibility = if visible.0 {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };

    if let Ok(mut vis) = queries.p0().single_mut() {
        *vis = new_visibility;
    }
    if let Ok(mut vis) = queries.p1().single_mut() {
        *vis = new_visibility;
    }
}

/// 按 F5 键切换视角模式（第一人称/第三人称）
/// 同时更新玩家模型的可见性
pub fn toggle_view_mode(
    key: Res<ButtonInput<KeyCode>>,
    mut view_mode: ResMut<ViewMode>,
    mut query: Query<&mut Visibility, With<PlayerModel>>,
) {
    if !key.just_pressed(KeyCode::F5) {
        return;
    }

    // 切换视角模式
    *view_mode = match *view_mode {
        ViewMode::FirstPerson => ViewMode::ThirdPerson,
        ViewMode::ThirdPerson => ViewMode::FirstPerson,
    };

    // 更新玩家模型可见性
    let new_visibility = match *view_mode {
        ViewMode::FirstPerson => Visibility::Hidden,
        ViewMode::ThirdPerson => Visibility::Inherited,
    };

    if let Ok(mut vis) = query.single_mut() {
        *vis = new_visibility;
    }
}
