//! Scene setup: directional sunlight + ambient.
//!
//! 注意：方块（voxel）使用自定义 shader（voxel.wgsl），不依赖 Bevy PBR 灯光组件。
//! 这里的 DirectionalLight / AmbientLight 仅影响非方块实体（如粒子、UI 3D 元素等）。
//! 方块的光照参数在 `assets/shaders/voxel.wgsl` 中硬编码。

use bevy::prelude::*;

/// Spawns a directional "sun" light and ambient light.
pub fn setup_lighting(mut commands: Commands) {
    // Directional light 模拟太阳光 —— 与 voxel.wgsl 中 light_dir 方向对齐：
    //   shader: normalize(vec3<f32>(0.2, 0.85, 0.3))
    //   对应旋转：绕 X 轴 -1.0 rad（~57° 仰角），绕 Y 轴 +0.25 rad
    commands.spawn((
        DirectionalLight {
            // color: Color::srgb(1.0, 0.95, 0.8), // warm sunlight tint
            illuminance: 10_000.0,                 // 晴天亮度 (lux)
            shadows_enabled: false,
            ..default()
        },
        Transform::from_rotation(Quat::from_euler(EulerRot::XYZ, -1.0, 0.25, 0.0)),
    ));

    // 柔和环境光，防止完全黑面（仅影响非方块实体）
    commands.spawn(AmbientLight {
        color: Color::srgb(0.7, 0.75, 0.9), // cool blue-ish ambient
        brightness: 500.0,                    // 配合 illuminance 单位
        affects_lightmapped_meshes: false,
    });
}
