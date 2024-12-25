use std::collections::{HashMap, HashSet};

use bevy::{asset::RenderAssetUsages, color::palettes::css::WHITE, dev_tools::fps_overlay::{FpsOverlayConfig, FpsOverlayPlugin}, pbr::{self, wireframe::WireframeConfig}, prelude::*, render::mesh::{Indices, PrimitiveTopology}, text::FontSmoothing};
use bevy_flycam::prelude::*;
use fastnoise_lite::{FastNoiseLite, FractalType, NoiseType};
// 绘制线框的插件
use pbr::wireframe::WireframePlugin;
// 显示世界内参数
use bevy_inspector_egui::quick::WorldInspectorPlugin;

// 显示实时FPS
struct OverlayColor;
impl OverlayColor {
    const GREEN: Color = Color::srgb(0.0, 1.0, 0.0);
}
type BlockId = u8;
type Pos = [i32;3];
// 单个区块的直径为32
const CHUNK_XYZ:i32 = 32;
type ChunkStartPos = [i32;3];
type ChunkPos = [i32;3];


#[derive(Resource)]
struct CountManager {
    chunks: HashMap<ChunkPos, i32>,
    // 玩家可视半径 2 = 前后左右上下各2个区块
    render_distance: i32,
}


fn main(){
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(WorldInspectorPlugin::new())   // 显示世界内参数 
        .add_plugins((
            PlayerPlugin,           //  可移动摄像机插件
            WireframePlugin,        // 绘制线框插件
            FpsOverlayPlugin {      // 实时显示FPS插件
                config: FpsOverlayConfig {
                    text_config: TextFont {
                        // Here we define size of our overlay
                        font_size: 42.0,
                        // If we want, we can use a custom font
                        font: default(),
                        // We could also disable font smoothing,
                        font_smoothing: FontSmoothing::default(),
                    },
                    // We can also change color of the overlay
                    text_color: OverlayColor::GREEN,
                    enabled: true,
                },
            },
        ))
        .add_systems(Startup, setup)
        .add_systems(Update, (
            manage_chunks,
        ))

        // 将需要加载的区块存入HashMap
        .insert_resource(CountManager {
            chunks: HashMap::new(),
            render_distance: 3,
        })

        // 绘制线框需要的资源
        .insert_resource(WireframeConfig {
            // The global wireframe config enables drawing of wireframes on every mesh,
            // except those with `NoWireframe`. Meshes with `Wireframe` will always have a wireframe,
            // regardless of the global configuration.
            global: true,
            // Controls the default color of all wireframes. Used as the default color for global wireframes.
            // Can be changed per mesh using the `WireframeColor` component.
            default_color: WHITE.into(),
        })

        // 设置摄像机属性
        .insert_resource(MovementSettings {
            sensitivity: 0.00015, // default: 0.00012
            speed: 48.0, // default: 12.0
        })
        .run();
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let block_mesh_handle = create_cube_mesh();
    let cube_mesh = meshes.add(block_mesh_handle);
    let cube_materials = materials.add(
        StandardMaterial{
            ..Default::default()
        }
    );
    commands.spawn((
        Mesh3d(cube_mesh.clone()),
        MeshMaterial3d(cube_materials.clone()),
        Transform::from_xyz(0.0, 0.0, 0.0),
    ));
}

fn manage_chunks(
    mut count_manager: ResMut<CountManager>,
    camera_query: Query<&Transform, With<FlyCam>>,
    mut previous_position: Local<Option<Vec3>>,
) {
    if let Ok(transform) = camera_query.get_single() {
        // 检查摄像机位置是否发生变化
        if let Some(prev_pos) = *previous_position {
            if prev_pos != transform.translation {
                // 摄像机位置发生变化，执行区块检测逻辑
                let _chunk_start_pos = world_pos_2_chunk_start_pos(&transform.translation);
                let chunk_pos = world_pos_2_block_pos(&transform.translation);
                
                let mut new_chunks = HashSet::new();
                for x in -count_manager.render_distance ..= count_manager.render_distance {
                    for y in -count_manager.render_distance ..= count_manager.render_distance {
                        for z in -count_manager.render_distance ..= count_manager.render_distance {
                            let chunk_pos = [
                                chunk_pos[0] + x, 
                                chunk_pos[1] + y, 
                                chunk_pos[2] + z
                            ];
                            new_chunks.insert(chunk_pos);
                        }
                    }
                }
                // 移除超出范围的区块
                count_manager.chunks.retain(|pos, _| new_chunks.contains(pos));

                // 添加新区块
                for chunk_pos in new_chunks {
                    if !count_manager.chunks.contains_key(&chunk_pos) {
                        count_manager.chunks.insert(chunk_pos, 1);
                    }
                }
                println!("当前区块数量：{}", count_manager.chunks.len());   // 可视半径=3 -> 7x7x7 = 输出343

                // TODO: 在这里添加区块检测逻辑
                *previous_position = Some(transform.translation);
            }
        } else {
            // 第一次运行时，记录初始位置
            *previous_position = Some(transform.translation);
        }
    }
}

fn world_pos_2_chunk_start_pos(pos: &Vec3) -> ChunkStartPos {
    let chunk_x = (pos.x as i32) / CHUNK_XYZ;
    let chunk_y = (pos.y as i32) / CHUNK_XYZ;
    let chunk_z = (pos.z as i32) / CHUNK_XYZ;

    let chunk_start_x = chunk_x * CHUNK_XYZ;
    let chunk_start_y = chunk_y * CHUNK_XYZ;
    let chunk_start_z = chunk_z * CHUNK_XYZ;

    [chunk_start_x, chunk_start_y, chunk_start_z]
}

fn world_pos_2_block_pos(pos: &Vec3) -> ChunkPos {
    let chunk_x = (pos.x as i32) / CHUNK_XYZ;
    let chunk_y = (pos.y as i32) / CHUNK_XYZ;
    let chunk_z = (pos.z as i32) / CHUNK_XYZ;
    [chunk_x, chunk_y, chunk_z]
}

// 实现区块管理的化，这里应该需要传递区块坐标，生成该区块的方块，再生成、返回mesh
fn create_cube_mesh() -> Mesh {
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut uvs = Vec::new();
    let mut indices = Vec::new();

    // 将方块坐标存在hashmap中，k:pos, v:block_id
    let mut chunk_blocks:HashMap<Pos, BlockId> = HashMap::new();

    // 根据噪声生成
    let mut noise = FastNoiseLite::new();
    noise.set_noise_type(Some(NoiseType::OpenSimplex2));
    // 噪声种子为420
    noise.set_seed(Some(420));
    // 设置FBm噪声参数
    noise.set_fractal_type(Some(FractalType::FBm));
    noise.set_fractal_gain(Some(0.5));
    noise.set_fractal_octaves(Some(4));
    noise.set_frequency(Some(0.01));

    for x in 0..CHUNK_XYZ {
        for z in 0..CHUNK_XYZ {
            let negative_1_to_1 = noise.get_noise_2d(x as f32, z as f32);
            let noise_date = (negative_1_to_1 + 1.) / 2.;
            
            let block_y = (noise_date * CHUNK_XYZ as f32) as i32;
            for y in 0..block_y {
                // 可以从这里判断当前坐标的方块是否需要绘制
                // get 方块坐标，判断是否四周是空气还是实体方块，如果是实体方块则删掉该顶点：坐标的  噪声值 < 阈值 = 空气
                if !chunk_blocks.contains_key(&[x, y, z]){
                    chunk_blocks.insert([x, y, z], 1);
                }
            }
        }
    }

    // 遍历chunk_blocks中所有方块，判断是否需要绘制
    // println!("方块数量：{}", chunk_blocks.len());
    for (pos, _block_id) in chunk_blocks.iter(){
        let pos = [pos[0], pos[1], pos[2]];
        
        if !(
            chunk_blocks.contains_key(&[pos[0], pos[1] + 1, pos[2]]) &&
            chunk_blocks.contains_key(&[pos[0], pos[1] - 1, pos[2]]) &&
            chunk_blocks.contains_key(&[pos[0] + 1, pos[1], pos[2]]) &&
            chunk_blocks.contains_key(&[pos[0] - 1, pos[1], pos[2]]) &&
            chunk_blocks.contains_key(&[pos[0], pos[1], pos[2] + 1]) &&
            chunk_blocks.contains_key(&[pos[0], pos[1], pos[2] - 1])
        ){
            // 由原来的每个方块都绘制6个面，现在只需要按是否遮挡来绘制未遮挡的面
            add_cube_to_mesh(&mut positions, &mut normals, &mut uvs, &mut indices, [pos[0] as f32, pos[1] as f32, pos[2] as f32]);
        }
    }

    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD)
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
    .with_inserted_indices(Indices::U32(indices))
}

fn add_cube_to_mesh(
    positions: &mut Vec<[f32; 3]>,
    normals: &mut Vec<[f32; 3]>,
    uvs: &mut Vec<[f32; 2]>,
    indices: &mut Vec<u32>,
    pos: [f32; 3],
) {
    let start_index = positions.len() as u32;
    // 顶点位置 决定位置
    positions.extend_from_slice(&[
        [pos[0], pos[1] + 1.0, pos[2]], // 0
        [pos[0] + 1.0, pos[1] + 1.0, pos[2]], // 1
        [pos[0] + 1.0, pos[1] + 1.0, pos[2] + 1.0], // 2
        [pos[0], pos[1] + 1.0, pos[2] + 1.0], // 3
        [pos[0], pos[1], pos[2]], // 4
        [pos[0] + 1.0, pos[1], pos[2]], // 5
        [pos[0] + 1.0, pos[1], pos[2] + 1.0], // 6
        [pos[0], pos[1], pos[2] + 1.0], // 7
    ]);

    // 法线 决定面朝向
    normals.extend_from_slice(&[
        [0.0, 1.0, 0.0], [0.0, 1.0, 0.0], [0.0, 1.0, 0.0], [0.0, 1.0, 0.0], // 顶面
        [0.0, -1.0, 0.0], [0.0, -1.0, 0.0], [0.0, -1.0, 0.0], [0.0, -1.0, 0.0], // 底面
        [1.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 0.0, 0.0], // 右侧面
        [-1.0, 0.0, 0.0], [-1.0, 0.0, 0.0], [-1.0, 0.0, 0.0], [-1.0, 0.0, 0.0], // 左侧面
        [0.0, 0.0, 1.0], [0.0, 0.0, 1.0], [0.0, 0.0, 1.0], [0.0, 0.0, 1.0], // 背面
        [0.0, 0.0, -1.0], [0.0, 0.0, -1.0], [0.0, 0.0, -1.0], [0.0, 0.0, -1.0], // 前面
    ]);

    // UV 坐标 决定纹理
    uvs.extend_from_slice(&[
        [0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0], // 顶面
        [0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0], // 底面
        [0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0], // 右侧面
        [0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0], // 左侧面
        [0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0], // 背面
        [0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0], // 前面
    ]);

    // 索引 决定绘制顺序
    indices.extend_from_slice(&[
        start_index + 0, start_index + 3, start_index + 1, start_index + 1, start_index + 3, start_index + 2, // 顶面       0,3,1,1,3,2
        start_index + 4, start_index + 5, start_index + 7, start_index + 5, start_index + 6, start_index + 7, // 底面       4,5,7,5,6,7
        start_index + 1, start_index + 2, start_index + 5, start_index + 5, start_index + 2, start_index + 6, // 右侧面     1,2,5,5,2,6
        start_index + 0, start_index + 4, start_index + 3, start_index + 3, start_index + 4, start_index + 7, // 左侧面     0,4,3,3,4,7
        start_index + 2, start_index + 3, start_index + 6, start_index + 6, start_index + 3, start_index + 7, // 背面       2,3,6,6,3,7
        start_index + 0, start_index + 1, start_index + 4, start_index + 4, start_index + 1, start_index + 5, // 前面       0,1,4,4,1,5
    ]);
}