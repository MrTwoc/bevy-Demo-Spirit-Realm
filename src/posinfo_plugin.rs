use bevy::prelude::*;
use bevy_flycam::FlyCam;

pub struct PosInfoPlugin;

#[derive(Component)]
struct CameraPosInfo;
#[derive(Component)]
struct ChunkPosInfo;

const CHUNK_XYZ: i32 = 32;

impl Plugin for PosInfoPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, pos_info);
        app.add_systems(Update, (show_pos_info, show_chunkpos_info));
    }
}

fn pos_info(mut commands: Commands) {
    // 绘制屏幕text，显示各种坐标信息
    commands
        .spawn((Text::new("GameInfo: \n"), Node { position_type: PositionType::Absolute, bottom: Val::Px(5.0), left: Val::Px(15.0), ..default() }))
        .with_child((TextSpan::new("Unknown"), CameraPosInfo))
        .with_child((TextSpan::new("Unknown"), ChunkPosInfo));
}

fn show_pos_info(mut camera_pos: Query<&mut TextSpan, With<CameraPosInfo>>, camera_query: Query<&Transform, With<FlyCam>>) {
    if let Ok(transform) = camera_query.get_single() {
        camera_pos.single_mut().0 = format!("Camera Pos: {},{},{}\n", &transform.translation[0], &transform.translation[1], &transform.translation[2]);
    }
}

fn show_chunkpos_info(mut chunk_pos: Query<&mut TextSpan, With<ChunkPosInfo>>, camera_query: Query<&Transform, With<FlyCam>>) {
    if let Ok(transform) = camera_query.get_single() {
        chunk_pos.single_mut().0 = format!(
            "Chunk Pos: {},{},{}\n",
            (&transform.translation[0] / CHUNK_XYZ as f32) as i32,
            (&transform.translation[1] / CHUNK_XYZ as f32) as i32,
            (&transform.translation[2] / CHUNK_XYZ as f32) as i32
        );
    }
}
