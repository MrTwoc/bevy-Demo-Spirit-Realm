//! Input handling: cursor grab (pointer lock).

use bevy::{
    prelude::*,
    window::{CursorGrabMode, CursorOptions},
};

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
