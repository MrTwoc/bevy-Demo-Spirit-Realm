//! Player entity: position, movement, and camera attachment.

use bevy::{core_pipeline::Skybox, prelude::*};

use crate::camera::CameraController;
use crate::character_controller::CharacterController;

/// The main player marker component.
#[derive(Component)]
pub struct Player;

/// 玩家模型标记组件（用于在视角切换时控制可见性）
#[derive(Component)]
pub struct PlayerModel;

/// Spawns the player entity with a Camera3d as a child.
/// Returns (player_entity, camera_entity) for HUD attachment.
/// Also spawns a player model (cube) as a child entity.
pub fn spawn_player(
    commands: &mut Commands,
    initial_pos: Vec3,
    meshes: &mut ResMut<Assets<Mesh>>,
    materials: &mut ResMut<Assets<StandardMaterial>>,
) -> (Entity, Entity) {
    let camera_entity = commands.spawn_empty().id();

    // 创建玩家模型（立方体）
    let player_model_entity = commands
        .spawn((
            PlayerModel,
            Mesh3d(meshes.add(Cuboid::new(0.6, 1.8, 0.6))),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: Color::srgb(0.2, 0.7, 0.3),
                ..default()
            })),
            Transform::from_xyz(0.0, 0.9, 0.0), // 底部对齐脚部
            Visibility::Hidden,                   // 默认隐藏（第一人称）
        ))
        .id();

    let player_entity = commands
        .spawn((
            Player,
            CharacterController::default(),
            Transform::from_translation(initial_pos),
            Visibility::default(),
        ))
        .add_child(camera_entity)
        .add_child(player_model_entity)
        .id();

    (player_entity, camera_entity)
}

/// Inserts the camera components after resource loading is complete.
/// Called from setup_world once Skybox handle is ready.
pub fn insert_camera_components(
    commands: &mut Commands,
    camera_entity: Entity,
    skybox_image: Handle<Image>,
) {
    commands.entity(camera_entity).insert((
        Camera3d::default(),
        CameraController::default(),
        Transform::from_translation(Vec3::new(0.0, 1.80, 0.0)), // 眼睛高度
        Skybox {
            image: skybox_image,
            brightness: 1000.0,
            ..default()
        },
    ));
}
