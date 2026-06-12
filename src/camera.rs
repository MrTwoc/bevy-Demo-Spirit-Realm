//! Camera controller: rotation only. Movement is handled by the Player entity.

use bevy::{
    ecs::message::MessageReader,
    input::mouse::MouseMotion,
    prelude::*,
    window::{CursorGrabMode, CursorOptions},
};

/// Mouse sensitivity for looking around.
pub const MOUSE_SENSITIVITY: f32 = 0.002;

/// 视角模式枚举
#[derive(Resource, PartialEq, Eq, Clone, Copy)]
pub enum ViewMode {
    /// 第一人称：相机在眼睛位置，玩家模型隐藏
    FirstPerson,
    /// 第三人称：相机在玩家身后，玩家模型可见
    ThirdPerson,
}

impl Default for ViewMode {
    fn default() -> Self {
        Self::FirstPerson
    }
}

/// Component storing the camera's rotation state (pitch and yaw).
#[derive(Component)]
pub struct CameraController {
    /// Vertical angle (up/down), clamped to prevent flipping.
    pub pitch: f32,
    /// Horizontal angle (left/right).
    pub yaw: f32,
}

impl Default for CameraController {
    fn default() -> Self {
        Self {
            pitch: -0.3,
            yaw: -0.8,
        }
    }
}

/// Handles mouse look by consuming MouseMotion events.
/// Only rotates when the cursor is locked (pointer grab active).
/// Camera position is always updated based on ViewMode.
pub fn camera_rotation(
    mut mouse_motion: MessageReader<MouseMotion>,
    cursor_options: Single<&CursorOptions>,
    view_mode: Res<ViewMode>,
    mut query: Query<(&mut Transform, &mut CameraController), With<Camera3d>>,
) {
    let Ok((mut transform, mut controller)) = query.single_mut() else {
        return;
    };

    // 只在光标锁定时处理鼠标旋转
    if cursor_options.grab_mode == CursorGrabMode::Locked {
        for event in mouse_motion.read() {
            controller.yaw -= event.delta.x * MOUSE_SENSITIVITY;
            // 第一人称和第三人称的 pitch 方向相反
            match *view_mode {
                ViewMode::FirstPerson => {
                    // 第一人称：pitch 增加 → 视角向上
                    controller.pitch -= event.delta.y * MOUSE_SENSITIVITY;
                }
                ViewMode::ThirdPerson => {
                    // 第三人称：pitch 减小 → 视角向上（因为相机位置更低）
                    controller.pitch += event.delta.y * MOUSE_SENSITIVITY;
                }
            }
            controller.pitch = controller.pitch.clamp(-1.54, 1.54);
        }
    }

    // 根据视角模式调整相机位置和旋转（每帧更新，确保 F5 切换立即生效）
    match *view_mode {
        ViewMode::FirstPerson => {
            transform.translation = Vec3::new(0.0, 1.80, 0.0);
            transform.rotation = Quat::from_euler(EulerRot::YXZ, controller.yaw, controller.pitch, 0.0);
        }
        ViewMode::ThirdPerson => {
            let head_pos = Vec3::new(0.0, 1.8, 0.0); // 玩家头部位置（本地坐标）
            let distance = 4.0;
            let yaw = controller.yaw;

            // 限制轨道俯仰角，防止相机翻转到头顶/脚下
            let orbit_pitch = controller.pitch.clamp(-1.2, 1.2);

            // 相机绕头部位置做球面轨道运动
            transform.translation = Vec3::new(
                yaw.sin() * orbit_pitch.cos() * distance,
                head_pos.y + orbit_pitch.sin() * distance,
                yaw.cos() * orbit_pitch.cos() * distance,
            );

            // 始终看向玩家头部，确保头部保持在屏幕中央（准星位置）
            transform.look_at(head_pos, Vec3::Y);
        }
    }
}
