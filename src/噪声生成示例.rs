use bevy::{
    dev_tools::fps_overlay::{FpsOverlayConfig, FpsOverlayPlugin}, diagnostic::{FrameTimeDiagnosticsPlugin, LogDiagnosticsPlugin}, prelude::*, render::render_resource::Face
};
use bevy_flycam::prelude::*;
use fastnoise_lite::*;

#[derive(Component)]
struct Block;

fn main() {
    App::new()
        .add_plugins((
            DefaultPlugins,
            FpsOverlayPlugin {
                config: FpsOverlayConfig {
                    text_config: TextStyle {
                        // Here we define size of our overlay
                        font_size: 50.0,
                        // We can also change color of the overlay
                        color: Color::srgb(0.0, 1.0, 0.0),
                        // If we want, we can use a custom font
                        font: default(),
                    },
                },
            },
            PlayerPlugin,
        ))
        .add_systems(Startup, setup)
        .run();
}

/// set up a simple 3D scene
fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {

    let mut noise = FastNoiseLite::new();
    noise.set_noise_type(Some(NoiseType::OpenSimplex2));
    noise.set_seed(Some(420));
    let cube_mesh = meshes.add(Cuboid::new(1.0, 1.0, 1.0));
    let material = materials.add(StandardMaterial{
        cull_mode: Some(Face::Back),        // 剔除背面
        ..default()
    });

    const CHUNK_WIDTH: usize = 32;
    const CHUNK_HEIGHT: usize = 32;
    let mut noise_data = [[0.;CHUNK_HEIGHT]; CHUNK_WIDTH];

    for x in 0..CHUNK_WIDTH {
        for z in 0..CHUNK_HEIGHT {
            let negative_1_to_1 = noise.get_noise_2d(x as f32, z as f32);
            noise_data[x][z] = (negative_1_to_1 + 1.) / 2.;
            // 根据noise值生成方块高度
            let y = noise_data[x][z] * 32.;
            //Y轴高度取整，不然方块有间隙
            let y = y.floor() as u32;
            for y in 0..y {
                commands.spawn((PbrBundle {
                    mesh: cube_mesh.clone(),
                    material: material.clone(),
                    transform: Transform::from_xyz(x as f32, y as f32, z as f32),
                    ..default()
                },Block));
            }
        }
    }
}