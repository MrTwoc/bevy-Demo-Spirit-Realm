//! Voxel cube mesh and scene setup.

use bevy::{
    asset::RenderAssetUsages,
    mesh::Indices,
    prelude::*,
    render::render_resource::PrimitiveTopology,
};

/// Marker component for the custom-UV voxel mesh.
#[derive(Component)]
pub struct CustomUV;

/// Sets up the scene: spawns the voxel cube, a point light, and the camera.
pub fn setup_scene(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let texture_handle: Handle<Image> = asset_server.load("textures/array_texture.png");
    let cube_handle: Handle<Mesh> = meshes.add(create_voxel_mesh());

    commands.spawn((
        Mesh3d(cube_handle),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color_texture: Some(texture_handle),
            ..default()
        })),
        CustomUV,
    ));

    let light_transform = Transform::from_xyz(1.8, 1.8, 1.8).looking_at(Vec3::ZERO, Vec3::Y);
    commands.spawn((PointLight::default(), light_transform));

    let camera_transform = Transform::from_xyz(2.5, 2.0, 2.5);
    use crate::camera::CameraController;
    commands.spawn((
        Camera3d::default(),
        camera_transform,
        CameraController::default(),
    ));
}

#[rustfmt::skip]
fn create_voxel_mesh() -> Mesh {
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(
            Mesh::ATTRIBUTE_POSITION,
            vec![
                // top (+y)
                [-0.5, 0.5, -0.5],
                [0.5, 0.5, -0.5],
                [0.5, 0.5, 0.5],
                [-0.5, 0.5, 0.5],
                // bottom (-y)
                [-0.5, -0.5, -0.5],
                [0.5, -0.5, -0.5],
                [0.5, -0.5, 0.5],
                [-0.5, -0.5, 0.5],
                // right (+x)
                [0.5, -0.5, -0.5],
                [0.5, -0.5, 0.5],
                [0.5, 0.5, 0.5],
                [0.5, 0.5, -0.5],
                // left (-x)
                [-0.5, -0.5, -0.5],
                [-0.5, -0.5, 0.5],
                [-0.5, 0.5, 0.5],
                [-0.5, 0.5, -0.5],
                // back (+z)
                [-0.5, -0.5, 0.5],
                [-0.5, 0.5, 0.5],
                [0.5, 0.5, 0.5],
                [0.5, -0.5, 0.5],
                // forward (-z)
                [-0.5, -0.5, -0.5],
                [-0.5, 0.5, -0.5],
                [0.5, 0.5, -0.5],
                [0.5, -0.5, -0.5],
            ],
        )
        .with_inserted_attribute(
            Mesh::ATTRIBUTE_UV_0,
            vec![
                [0.0, 0.2], [0.0, 0.0], [1.0, 0.0], [1.0, 0.2],
                [0.0, 0.45], [0.0, 0.25], [1.0, 0.25], [1.0, 0.45],
                [1.0, 0.45], [0.0, 0.45], [0.0, 0.2], [1.0, 0.2],
                [1.0, 0.45], [0.0, 0.45], [0.0, 0.2], [1.0, 0.2],
                [0.0, 0.45], [0.0, 0.2], [1.0, 0.2], [1.0, 0.45],
                [0.0, 0.45], [0.0, 0.2], [1.0, 0.2], [1.0, 0.45],
            ],
        )
        .with_inserted_attribute(
            Mesh::ATTRIBUTE_NORMAL,
            vec![
                [0.0, 1.0, 0.0], [0.0, 1.0, 0.0], [0.0, 1.0, 0.0], [0.0, 1.0, 0.0],
                [0.0, -1.0, 0.0], [0.0, -1.0, 0.0], [0.0, -1.0, 0.0], [0.0, -1.0, 0.0],
                [1.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 0.0, 0.0],
                [-1.0, 0.0, 0.0], [-1.0, 0.0, 0.0], [-1.0, 0.0, 0.0], [-1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0], [0.0, 0.0, 1.0], [0.0, 0.0, 1.0], [0.0, 0.0, 1.0],
                [0.0, 0.0, -1.0], [0.0, 0.0, -1.0], [0.0, 0.0, -1.0], [0.0, 0.0, -1.0],
            ],
        )
        .with_inserted_indices(Indices::U32(vec![
            0,3,1 , 1,3,2,
            4,5,7 , 5,6,7,
            8,11,9 , 9,11,10,
            12,13,15 , 13,14,15,
            16,19,17 , 17,19,18,
            20,21,23 , 21,22,23,
        ]))
}
