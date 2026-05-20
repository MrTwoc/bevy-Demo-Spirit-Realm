//! Chunk wireframe — 按 V 键切换线框显示/隐藏
//! 1) 方块线框（Bevy Wireframe 组件）：渲染 mesh 三角形的线框
//! 2) 区块包围盒（gizmos）：绘制区块外包 bounding box（调试用）
//!
//! 渲染范围限制为以摄像机为中心 3x3x3 的区块区域。

use bevy::{gizmos::gizmos::Gizmos, pbr::wireframe::Wireframe, prelude::*};

use crate::chunk::{self, ChunkComponent, ChunkCoord};
use crate::chunk_dirty::ChunkCoordComponent;

/// Tracks the current wireframe mode state.
#[derive(Resource, Default)]
pub struct WireframeMode(pub bool);

/// Press V to toggle wireframe on/off.
pub fn toggle_wireframe(keyboard: Res<ButtonInput<KeyCode>>, mut mode: ResMut<WireframeMode>) {
    if keyboard.just_pressed(KeyCode::KeyV) {
        mode.0 = !mode.0;
    }
}

/// 每帧同步：WireframeMode → 实体是否带 Wireframe 组件。
/// 进入线框模式时给 3x3x3 范围内的 Chunk 插入 Wireframe 组件，
/// 退出时移除。两种模式可以同时存在：mesh 线框 + bounding box。
pub fn sync_chunk_wireframe(
    mode: Res<WireframeMode>,
    mut commands: Commands,
    chunks: Query<(Entity, &ChunkCoordComponent), With<ChunkComponent>>,
    camera_query: Query<&Transform, With<Camera3d>>,
) {
    // 获取摄像机所在的区块坐标
    let Ok(cam_transform) = camera_query.single() else {
        return;
    };
    let player_chunk = ChunkCoord::from_world(cam_transform.translation);

    if mode.0 {
        // 进入线框模式：只对 3x3x3 范围内的 Chunk 插入 Wireframe
        for (entity, coord) in &chunks {
            if in_3x3x3_range(coord.0, player_chunk) {
                commands.entity(entity).try_insert(Wireframe);
            }
        }
    } else {
        // 退出线框模式：移除所有 Chunk 的 Wireframe 组件
        for (entity, _) in &chunks {
            commands.entity(entity).remove::<Wireframe>();
        }
    }
}

/// 绘制 3x3x3 范围内区块的外包 bounding box（调试用，半透明白色盒子）。
/// 只要 WireframeMode 开启就会绘制。
/// 只绘制当前对摄像机可见的区块，避免对不可见区块进行无效绘制。
pub fn draw_wireframes(
    mode: Res<WireframeMode>,
    chunks: Query<(
        &ChunkComponent,
        &Transform,
        &ChunkCoordComponent,
        &ViewVisibility,
    )>,
    camera_query: Query<&Transform, With<Camera3d>>,
    mut gizmos: Gizmos,
) {
    if !mode.0 {
        return;
    }

    // 获取摄像机所在的区块坐标
    let Ok(cam_transform) = camera_query.single() else {
        return;
    };
    let player_chunk = ChunkCoord::from_world(cam_transform.translation);

    let size = chunk::CHUNK_SIZE as f32;
    let color = Color::WHITE.with_alpha(0.6);

    for (_, transform, coord_comp, vis) in &chunks {
        // 只绘制 3x3x3 范围内的区块
        if !in_3x3x3_range(coord_comp.0, player_chunk) {
            continue;
        }

        // 只绘制对摄像机可见的区块
        if !vis.get() {
            continue;
        }

        // gizmos.cube: Transform.translation is the CENTER of the box.
        // For a 32x32x32 box, center = chunk_origin + (16, 16, 16).
        let center = transform.translation + Vec3::splat(size / 2.0);
        let gizmo_transform = Transform::from_translation(center).with_scale(Vec3::splat(size));
        gizmos.cube(gizmo_transform, color);
    }
}

/// 判断目标区块是否在以玩家区块为中心的 3x3x3 范围内。
#[inline]
fn in_3x3x3_range(target: ChunkCoord, player: ChunkCoord) -> bool {
    let dx = (target.cx - player.cx).abs();
    let dy = (target.cy - player.cy).abs();
    let dz = (target.cz - player.cz).abs();
    dx <= 1 && dy <= 1 && dz <= 1
}
