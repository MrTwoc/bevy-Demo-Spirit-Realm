use bevy::{
    asset::RenderAssetUsages,
    ecs::message::MessageReader,
    input::mouse::MouseMotion,
    mesh::Indices,
    prelude::*,
    render::render_resource::PrimitiveTopology,
    window::{CursorGrabMode, CursorOptions},
};

#[derive(Component)]
struct CustomUV;

/// Movement speed for the camera in units per second.
const CAMERA_MOVE_SPEED: f32 = 5.0;

/// Mouse sensitivity for looking around.
const MOUSE_SENSITIVITY: f32 = 0.002;

/// Component to store camera rotation state (pitch and yaw).
#[derive(Component)]
struct CameraController {
    pitch: f32,
    yaw: f32,
}

impl Default for CameraController {
    fn default() -> Self {
        Self {
            // Slight downward angle so the cube and ground are visible.
            pitch: -0.3,
            // Face toward the cube (it's near origin).
            yaw: -0.8,
        }
    }
}

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (camera_movement, camera_rotation, cursor_grab_system),
        )
        .run();
}

/// Sets up the scene: spawns the cube, light, and a controlled camera.
fn setup(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    // Import the custom texture.
    let custom_texture_handle: Handle<Image> = asset_server.load("textures/array_texture.png");
    // Create and save a handle to the mesh.
    let cube_mesh_handle: Handle<Mesh> = meshes.add(create_cube_mesh());

    // Render the mesh with the custom texture, and add the marker.
    commands.spawn((
        Mesh3d(cube_mesh_handle),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color_texture: Some(custom_texture_handle),
            ..default()
        })),
        CustomUV,
    ));

    // Fixed position for the point light, looking at origin.
    let light_transform = Transform::from_xyz(1.8, 1.8, 1.8).looking_at(Vec3::ZERO, Vec3::Y);

    // Light up the scene.
    commands.spawn((PointLight::default(), light_transform));

    // Camera in 3D space with controller component.
    // Initial rotation is set in CameraController::default().
    let camera_transform = Transform::from_xyz(2.5, 2.0, 2.5);
    commands.spawn((
        Camera3d::default(),
        camera_transform,
        CameraController::default(),
    ));
}

/// Handles WASD + Space/Shift camera movement.
fn camera_movement(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mut query: Query<(&mut Transform, &CameraController), With<Camera3d>>,
) {
    let Ok((mut transform, _controller)) = query.single_mut() else {
        return;
    };

    let mut movement = Vec3::ZERO;

    if keys.pressed(KeyCode::KeyW) {
        movement.z += 1.0;
    }
    if keys.pressed(KeyCode::KeyS) {
        movement.z -= 1.0;
    }
    if keys.pressed(KeyCode::KeyA) {
        movement.x -= 1.0;
    }
    if keys.pressed(KeyCode::KeyD) {
        movement.x += 1.0;
    }
    if keys.pressed(KeyCode::Space) {
        movement.y += 1.0;
    }
    if keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight) {
        movement.y -= 1.0;
    }

    if movement != Vec3::ZERO {
        let normalized_movement = movement.normalize();

        // Project onto XZ plane so movement stays ground-relative (no flying when looking up/down).
        let forward = transform.forward();
        let right = transform.right();
        let horizontal_forward = Vec3::new(forward.x, 0.0, forward.z).normalize();
        let horizontal_right = Vec3::new(right.x, 0.0, right.z).normalize();

        let delta = (horizontal_forward * normalized_movement.z
            + horizontal_right * normalized_movement.x
            + Vec3::Y * normalized_movement.y)
            * CAMERA_MOVE_SPEED
            * time.delta_secs();

        transform.translation += delta;
    }
}

/// Handles mouse look by consuming MouseMotion events.
fn camera_rotation(
    mut mouse_motion: MessageReader<MouseMotion>,
    mut query: Query<(&mut Transform, &mut CameraController), With<Camera3d>>,
) {
    let Ok((mut transform, mut controller)) = query.single_mut() else {
        return;
    };

    for event in mouse_motion.read() {
        controller.yaw -= event.delta.x * MOUSE_SENSITIVITY;
        controller.pitch -= event.delta.y * MOUSE_SENSITIVITY;

        // Clamp pitch so the camera doesn't flip upside down.
        controller.pitch = controller.pitch.clamp(-1.54, 1.54);

        // Apply rotation (YXZ Euler: yaw first, then pitch).
        transform.rotation = Quat::from_euler(EulerRot::YXZ, controller.yaw, controller.pitch, 0.0);
    }
}

/// Grabs the cursor when left mouse button is pressed, releases it on Escape.
fn cursor_grab_system(
    mut cursor_options: Single<&mut CursorOptions>,
    mouse: Res<ButtonInput<MouseButton>>,
    key: Res<ButtonInput<KeyCode>>,
) {
    if mouse.just_pressed(MouseButton::Left) {
        cursor_options.visible = false;
        cursor_options.grab_mode = CursorGrabMode::Locked;
    }

    if key.just_pressed(KeyCode::Escape) {
        cursor_options.visible = true;
        cursor_options.grab_mode = CursorGrabMode::None;
    }
}

#[rustfmt::skip]
fn create_cube_mesh() -> Mesh {
    // Keep the mesh data accessible in future frames to be able to mutate it in toggle_texture.
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(
            Mesh::ATTRIBUTE_POSITION,
            // Each array is an [x, y, z] coordinate in local space.
            // The camera coordinate space is right-handed x-right, y-up, z-back. This means "forward" is -Z.
            // Meshes always rotate around their local [0, 0, 0] when a rotation is applied to their Transform.
            // By centering our mesh around the origin, rotating the mesh preserves its center of mass.
            vec![
                // top (facing towards +y)
                [-0.5, 0.5, -0.5], // vertex with index 0
                [0.5, 0.5, -0.5], // vertex with index 1
                [0.5, 0.5, 0.5], // etc. until 23
                [-0.5, 0.5, 0.5],
                // bottom   (-y)
                [-0.5, -0.5, -0.5],
                [0.5, -0.5, -0.5],
                [0.5, -0.5, 0.5],
                [-0.5, -0.5, 0.5],
                // right    (+x)
                [0.5, -0.5, -0.5],
                [0.5, -0.5, 0.5],
                [0.5, 0.5, 0.5], // This vertex is at the same position as vertex with index 2, but they'll have different UV and normal
                [0.5, 0.5, -0.5],
                // left     (-x)
                [-0.5, -0.5, -0.5],
                [-0.5, -0.5, 0.5],
                [-0.5, 0.5, 0.5],
                [-0.5, 0.5, -0.5],
                // back     (+z)
                [-0.5, -0.5, 0.5],
                [-0.5, 0.5, 0.5],
                [0.5, 0.5, 0.5],
                [0.5, -0.5, 0.5],
                // forward  (-z)
                [-0.5, -0.5, -0.5],
                [-0.5, 0.5, -0.5],
                [0.5, 0.5, -0.5],
                [0.5, -0.5, -0.5],
            ],
        )
        // Set-up UV coordinates to point to the upper (V < 0.5), "dirt+grass" part of the texture.
        // Take a look at the custom image (assets/textures/array_texture.png)
        // so the UV coords will make more sense
        // Note: (0.0, 0.0) = Top-Left in UV mapping, (1.0, 1.0) = Bottom-Right in UV mapping
        .with_inserted_attribute(
            Mesh::ATTRIBUTE_UV_0,
            vec![
                // Assigning the UV coords for the top side.
                [0.0, 0.2], [0.0, 0.0], [1.0, 0.0], [1.0, 0.2],
                // Assigning the UV coords for the bottom side.
                [0.0, 0.45], [0.0, 0.25], [1.0, 0.25], [1.0, 0.45],
                // Assigning the UV coords for the right side.
                [1.0, 0.45], [0.0, 0.45], [0.0, 0.2], [1.0, 0.2],
                // Assigning the UV coords for the left side.
                [1.0, 0.45], [0.0, 0.45], [0.0, 0.2], [1.0, 0.2],
                // Assigning the UV coords for the back side.
                [0.0, 0.45], [0.0, 0.2], [1.0, 0.2], [1.0, 0.45],
                // Assigning the UV coords for the forward side.
                [0.0, 0.45], [0.0, 0.2], [1.0, 0.2], [1.0, 0.45],
            ],
        )
        // For meshes with flat shading, normals are orthogonal (pointing out) from the direction of
        // the surface.
        // Normals are required for correct lighting calculations.
        // Each array represents a normalized vector, which length should be equal to 1.0.
        .with_inserted_attribute(
            Mesh::ATTRIBUTE_NORMAL,
            vec![
                // Normals for the top side (towards +y)
                [0.0, 1.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 1.0, 0.0],
                // Normals for the bottom side (towards -y)
                [0.0, -1.0, 0.0],
                [0.0, -1.0, 0.0],
                [0.0, -1.0, 0.0],
                [0.0, -1.0, 0.0],
                // Normals for the right side (towards +x)
                [1.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                // Normals for the left side (towards -x)
                [-1.0, 0.0, 0.0],
                [-1.0, 0.0, 0.0],
                [-1.0, 0.0, 0.0],
                [-1.0, 0.0, 0.0],
                // Normals for the back side (towards +z)
                [0.0, 0.0, 1.0],
                [0.0, 0.0, 1.0],
                [0.0, 0.0, 1.0],
                [0.0, 0.0, 1.0],
                // Normals for the forward side (towards -z)
                [0.0, 0.0, -1.0],
                [0.0, 0.0, -1.0],
                [0.0, 0.0, -1.0],
                [0.0, 0.0, -1.0],
            ],
        )
        // Create the triangles out of the 24 vertices we created.
        // To construct a square, we need 2 triangles, therefore 12 triangles in total.
        // To construct a triangle, we need the indices of its 3 defined vertices, adding them one
        // by one, in a counter-clockwise order (relative to the position of the viewer, the order
        // should appear counter-clockwise from the front of the triangle, in this case from outside the cube).
        // Read more about how to correctly build a mesh manually in the Bevy documentation of a Mesh,
        // further examples and the implementation of the built-in shapes.
        //
        // The first two defined triangles look like this (marked with the vertex indices,
        // and the axis), when looking down on the top (+y) of the cube:
        //   -Z
        //   ^
        // 0---1
        // |  /|
        // | / | -> +X
        // |/  |
        // 3---2
        //
        // The right face's (+x) triangles look like this, seen from the outside of the cube.
        //   +Y
        //   ^
        // 10--11
        // |  /|
        // | / | -> -Z
        // |/  |
        // 9---8
        //
        // The back face's (+z) triangles look like this, seen from the outside of the cube.
        //   +Y
        //   ^
        // 17--18
        // |\  |
        // | \ | -> +X
        // |  \|
        // 16--19
        .with_inserted_indices(Indices::U32(vec![
            0,3,1 , 1,3,2, // triangles making up the top (+y) facing side.
            4,5,7 , 5,6,7, // bottom (-y)
            8,11,9 , 9,11,10, // right (+x)
            12,13,15 , 13,14,15, // left (-x)
            16,19,17 , 17,19,18, // back (+z)
            20,21,23 , 21,22,23, // forward (-z)
        ]))
}
