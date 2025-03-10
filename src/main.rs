mod posinfo_plugin;
use posinfo_plugin::PosInfoPlugin;

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
// 单个区块的直径为32: 添加了 CHUNK_SIZE 作为单个区块的长宽高。。CHUNK_XYZ暂时不做更改，用于显示负数的区块坐标
const CHUNK_XYZ:i32 = 32;
// 单个区块的直径限定为32
const CHUNK_SIZE: usize = 32;
// 玩家在Y轴的可视距离,超过这个距离被剔除
const CHUNK_HEIGHT:usize = 128;

type ChunkStartPos = [i32;3];
type ChunkPos = [i32;3];

// Add this component to mark spawned chunk entities
#[derive(Component)]
struct ChunkEntity(ChunkPos);

#[derive(Resource)]
struct CountManager {
    chunks: HashMap<ChunkPos, i32>,
    // 玩家可视半径 2 = 前后左右上下各2个区块
    render_distance: i32,
    new_chunks: HashSet<ChunkPos>,  // 新增字段,存储新增的区块坐标
    spawned_chunks: HashMap<ChunkPos, Entity>, // 追踪已加载的区块
}



fn main(){
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(WorldInspectorPlugin::new())   // 显示世界内参数 
        .add_plugins((
            PosInfoPlugin,
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
            load_chunks,
            cleanup_chunks,
        ))

        // 将需要加载的区块存入HashMap
        .insert_resource(CountManager {
            chunks: HashMap::new(),
            render_distance: 5,         // 玩家可视半径 1 = 前后左右上下各1个区块
            new_chunks: HashSet::new(),
            spawned_chunks: HashMap::new(),
        })

        // 绘制线框需要的资源
        .insert_resource(WireframeConfig {
            global: true,
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
    let block_mesh_handle = create_cube_mesh([0,0,0]);
    let cube_mesh = meshes.add(block_mesh_handle);
    let cube_materials = materials.add(
        StandardMaterial{
            ..Default::default()
        }
    );
    
    // 初始区块：玩家进入游戏时产生的第一个区块(不会被卸载)
    commands.spawn((
        Mesh3d(cube_mesh.clone()),
        MeshMaterial3d(cube_materials.clone()),
        Transform::from_xyz(0.0,0.0,0.0)
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
                        // println!("新增区块坐标：{:?}", &chunk_pos);
                        count_manager.chunks.insert(chunk_pos, 1);
                        count_manager.new_chunks.insert(chunk_pos);  // 记录新增的区块
                    }
                }
                // println!("当前区块数量：{}", count_manager.chunks.len());   // 可视半径=3 -> 7x7x7 = 输出343

                // TODO: 在这里添加区块检测逻辑
                *previous_position = Some(transform.translation);
            }
        } else {
            // 第一次运行时，记录初始位置
            *previous_position = Some(transform.translation);
        }
    }
}

fn load_chunks(
    mut count_manager: ResMut<CountManager>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let chunk_size = CHUNK_SIZE as i32;
    if count_manager.is_changed() {
        let new_chunks: Vec<_> = count_manager.new_chunks.iter().cloned().collect();
        for chunk_pos in new_chunks {
            // 将区块坐标转换为世界坐标
            let world_x = chunk_pos[0] * chunk_size;
            let world_y = chunk_pos[1] * chunk_size; 
            let world_z = chunk_pos[2] * chunk_size;
            
            // 生成区块实体
            let block_mesh_handle = create_cube_mesh([world_x,world_y,world_z]);
            let cube_mesh = meshes.add(block_mesh_handle);
            let cube_materials = materials.add(
                StandardMaterial{
                    ..Default::default()
                }
            );
            let entity = commands.spawn(
                (
                        Mesh3d(cube_mesh.clone()),
                        MeshMaterial3d(cube_materials.clone()),
                        Transform::from_xyz(world_x as f32, world_y as f32, world_z as f32),
                        ChunkEntity(chunk_pos),
                    ),
            ).id();
            count_manager.spawned_chunks.insert(chunk_pos, entity);
        }
        // 清空新增区块集合
        count_manager.new_chunks.clear();
    }
}

fn cleanup_chunks(
    mut count_manager: ResMut<CountManager>,
    mut commands: Commands,
    camera_query: Query<&Transform, With<FlyCam>>,
) {
    if let Ok(camera_transform) = camera_query.get_single() {
        let current_chunk = world_pos_2_block_pos(&camera_transform.translation);
        
        // Collect chunks to remove
        let chunks_to_remove: Vec<ChunkPos> = count_manager.spawned_chunks
            .keys()
            .filter(|&&chunk_pos| {
                let dx = (chunk_pos[0] - current_chunk[0]).abs();
                let dy = (chunk_pos[1] - current_chunk[1]).abs();
                let dz = (chunk_pos[2] - current_chunk[2]).abs();
                
                dx > count_manager.render_distance || 
                dy > count_manager.render_distance || 
                dz > count_manager.render_distance
            })
            .copied()
            .collect();

        // Remove chunks outside render distance
        for chunk_pos in chunks_to_remove {
            if let Some(entity) = count_manager.spawned_chunks.remove(&chunk_pos) {
                commands.entity(entity).despawn();
                count_manager.chunks.remove(&chunk_pos);
            }
        }
    }
}

fn world_pos_2_chunk_start_pos(pos: &Vec3) -> ChunkStartPos {
    let chunk_size = CHUNK_SIZE as i32;

    let chunk_x = (pos.x as i32) / chunk_size;
    let chunk_y = (pos.y as i32) / chunk_size;
    let chunk_z = (pos.z as i32) / chunk_size;

    let chunk_start_x = chunk_x * chunk_size;
    let chunk_start_y = chunk_y * chunk_size;
    let chunk_start_z = chunk_z * chunk_size;

    [chunk_start_x, chunk_start_y, chunk_start_z]
}

fn world_pos_2_block_pos(pos: &Vec3) -> ChunkPos {
    let chunk_size = CHUNK_SIZE as i32;
    let chunk_x = (pos.x as i32) / chunk_size;
    let chunk_y = (pos.y as i32) / chunk_size;
    let chunk_z = (pos.z as i32) / chunk_size;
    [chunk_x, chunk_y, chunk_z]
}

// 实现区块管理的化，这里应该需要传递区块坐标，生成该区块的方块，再生成、返回mesh
fn create_cube_mesh(world_pos:[i32;3]) -> Mesh {
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut uvs = Vec::new();
    let mut indices = Vec::new();

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

    // 将方块坐标存在hashmap中，k:pos, v:block_id
    let mut chunk_blocks:HashMap<Pos, BlockId> = HashMap::new();

    let chunk_start_x = world_pos[0];
    let chunk_start_y = world_pos[1];
    let chunk_start_z = world_pos[2];

    /*TODO:
        优化：使用3D数组存储方块数据
        注：这里数组的长度数值类型好像必须是usize，否则会报错
     */
    // let mut chunk_blocks_array = [[[0u8; CHUNK_SIZE]; CHUNK_SIZE]; CHUNK_SIZE];

    for x in 0..CHUNK_XYZ {
        for z in 0..CHUNK_XYZ {
            let world_x = chunk_start_x + x;
            let world_z = chunk_start_z + z;

            let negative_1_to_1 = noise.get_noise_2d(world_x as f32, world_z as f32);
            let noise_value = (negative_1_to_1 + 1.) / 2.;

            /*
                TODO:
                2025年1月1
                这里的高度值太高会导致生成错误，应该是目前基于块剔除的方法导致的，需要重新实现剔除算法
                BUG——2025.1.6
                每隔一段高度，会有一层高度完全没有方块，可能是拼接出的问题。
             */
            // 将噪声值从 [-1,1] 映射到 [± 20480]
            // let height = (noise_value * 20480.) as i32;
            
            let height = (noise_value * 32.) as i32;
            
            // let block_y = (noise_value * CHUNK_XYZ as f32) as i32;
            // 使用 world_y 来确定在当前区块高度范围内的方块
            let start_y = chunk_start_y;
            let end_y = chunk_start_y + CHUNK_XYZ;

            // // 只在当前区块高度范围内生成方块
            // if block_y > start_y && block_y < end_y {
            //     for y in start_y..block_y {
            //         let local_y = y - chunk_start_y;
            //         chunk_blocks.insert([x, local_y, z], 1);
            //     }
            // }
            
            if height >= start_y && height < end_y {
                for y in start_y..height {
                    let local_y = y - chunk_start_y;
                    if local_y >= 0 && local_y < CHUNK_XYZ {
                        chunk_blocks.insert([x, local_y, z], 1);
                    }
                }
            }

        }
    }

    // 遍历chunk_blocks中所有方块，判断是否需要绘制
    for (pos, _block_id) in chunk_blocks.iter(){
        let pos = [pos[0], pos[1], pos[2]];
        // 检查顶面
        if !chunk_blocks.contains_key(&[pos[0], pos[1] + 1, pos[2]]) {
            add_top_face(&mut positions, &mut normals, &mut uvs, &mut indices, [pos[0] as f32, pos[1] as f32, pos[2] as f32]);
        }
        // 检查底面 -- 暂时跳过不处理底面
        // if !chunk_blocks.contains_key(&[pos[0], pos[1] - 1, pos[2]]) {
        //     add_bottom_face(&mut positions, &mut normals, &mut uvs, &mut indices, [pos[0] as f32, pos[1] as f32, pos[2] as f32]);
        // }
        // 检查右面
        if !chunk_blocks.contains_key(&[pos[0] + 1, pos[1], pos[2]]) {
            add_right_face(&mut positions, &mut normals, &mut uvs, &mut indices, [pos[0] as f32, pos[1] as f32, pos[2] as f32]);
        }
        // 检查左面
        if !chunk_blocks.contains_key(&[pos[0] - 1, pos[1], pos[2]]) {
            add_left_face(&mut positions, &mut normals, &mut uvs, &mut indices, [pos[0] as f32, pos[1] as f32, pos[2] as f32]);
        }
        // 检查前面
        if !chunk_blocks.contains_key(&[pos[0], pos[1], pos[2] + 1]) {
            add_front_face(&mut positions, &mut normals, &mut uvs, &mut indices, [pos[0] as f32, pos[1] as f32, pos[2] as f32]);
            // 这里将前后的检测面写反了:
            // add_back_face(&mut positions, &mut normals, &mut uvs, &mut indices, [pos[0] as f32, pos[1] as f32, pos[2] as f32]);
        }
        // 检查背面
        if !chunk_blocks.contains_key(&[pos[0], pos[1], pos[2] - 1]) {
            add_back_face(&mut positions, &mut normals, &mut uvs, &mut indices, [pos[0] as f32, pos[1] as f32, pos[2] as f32]);
            // 以下是错误:
            // add_front_face(&mut positions, &mut normals, &mut uvs, &mut indices, [pos[0] as f32, pos[1] as f32, pos[2] as f32]);
        }
    }

    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD)
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
    .with_inserted_indices(Indices::U32(indices))
}

fn add_top_face(
    positions: &mut Vec<[f32; 3]>,
    normals: &mut Vec<[f32; 3]>,
    uvs: &mut Vec<[f32; 2]>,
    indices: &mut Vec<u32>,
    pos: [f32; 3],
) {
    let start_index = positions.len() as u32;
    positions.extend_from_slice(&[
        [pos[0], pos[1] + 1.0, pos[2]],
        [pos[0] + 1.0, pos[1] + 1.0, pos[2]],
        [pos[0] + 1.0, pos[1] + 1.0, pos[2] + 1.0],
        [pos[0], pos[1] + 1.0, pos[2] + 1.0],
    ]);
    normals.extend_from_slice(&[[0.0, 1.0, 0.0]; 4]);
    uvs.extend_from_slice(&[
        [0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0],
    ]);
    indices.extend_from_slice(&[
        start_index, start_index + 3, start_index + 1,
        start_index + 1, start_index + 3, start_index + 2,
    ]);
}

fn add_bottom_face(
    positions: &mut Vec<[f32; 3]>,
    normals: &mut Vec<[f32; 3]>,
    uvs: &mut Vec<[f32; 2]>,
    indices: &mut Vec<u32>,
    pos: [f32; 3],
) {
    let start_index = positions.len() as u32;
    positions.extend_from_slice(&[
        [pos[0], pos[1], pos[2]],
        [pos[0] + 1.0, pos[1], pos[2]],
        [pos[0] + 1.0, pos[1], pos[2] + 1.0],
        [pos[0], pos[1], pos[2] + 1.0],
    ]);
    normals.extend_from_slice(&[[0.0, -1.0, 0.0]; 4]);
    uvs.extend_from_slice(&[
        [0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0],
    ]);
    indices.extend_from_slice(&[
        start_index, start_index + 1, start_index + 3,
        start_index + 1, start_index + 2, start_index + 3,
    ]);
}

fn add_right_face(
    positions: &mut Vec<[f32; 3]>,
    normals: &mut Vec<[f32; 3]>,
    uvs: &mut Vec<[f32; 2]>,
    indices: &mut Vec<u32>,
    pos: [f32; 3],
) {
    let start_index = positions.len() as u32;
    positions.extend_from_slice(&[
        [pos[0] + 1.0, pos[1] + 1.0, pos[2]],
        [pos[0] + 1.0, pos[1] + 1.0, pos[2] + 1.0],
        [pos[0] + 1.0, pos[1], pos[2] + 1.0],
        [pos[0] + 1.0, pos[1], pos[2]],
    ]);
    normals.extend_from_slice(&[[1.0, 0.0, 0.0]; 4]);
    uvs.extend_from_slice(&[
        [0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0],
    ]);
    indices.extend_from_slice(&[
        start_index, start_index + 1, start_index + 3,
        start_index + 3, start_index + 1, start_index + 2,
    ]);
}

fn add_left_face(
    positions: &mut Vec<[f32; 3]>,
    normals: &mut Vec<[f32; 3]>,
    uvs: &mut Vec<[f32; 2]>,
    indices: &mut Vec<u32>,
    pos: [f32; 3],
) {
    let start_index = positions.len() as u32;
    positions.extend_from_slice(&[
        [pos[0], pos[1] + 1.0, pos[2]],
        [pos[0], pos[1] + 1.0, pos[2] + 1.0],
        [pos[0], pos[1], pos[2] + 1.0],
        [pos[0], pos[1], pos[2]],
    ]);
    normals.extend_from_slice(&[[-1.0, 0.0, 0.0]; 4]);
    uvs.extend_from_slice(&[
        [0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0],
    ]);
    indices.extend_from_slice(&[
        start_index, start_index + 3, start_index + 1,
        start_index + 1, start_index + 3, start_index + 2,
    ]);
}

fn add_back_face(
    positions: &mut Vec<[f32; 3]>,
    normals: &mut Vec<[f32; 3]>,
    uvs: &mut Vec<[f32; 2]>,
    indices: &mut Vec<u32>,
    pos: [f32; 3],
) {
    let start_index = positions.len() as u32;
    // 调整顶点顺序为右下→右上→左上→左下（负Z方向逆时针）
    positions.extend_from_slice(&[
        [pos[0] + 1.0, pos[1], pos[2]],        // 右下 (0)
        [pos[0] + 1.0, pos[1] + 1.0, pos[2]],  // 右上 (1)
        [pos[0], pos[1] + 1.0, pos[2]],        // 左上 (2)
        [pos[0], pos[1], pos[2]],              // 左下 (3)
    ]);
    normals.extend_from_slice(&[[0.0, 0.0, 1.0]; 4]);
    // 调整UV坐标匹配顶点顺序
    uvs.extend_from_slice(&[
        [1.0, 0.0], // 右下
        [1.0, 1.0], // 右上
        [0.0, 1.0], // 左上
        [0.0, 0.0], // 左下
    ]);
    // 修正索引顺序
    indices.extend_from_slice(&[
        start_index, start_index + 3, start_index + 1, // 三角形1: 左下→左上→右上
        start_index + 1, start_index + 3, start_index + 2, // 三角形2: 左下→右上→右下
    ]);
}

fn add_front_face(
    positions: &mut Vec<[f32; 3]>,
    normals: &mut Vec<[f32; 3]>,
    uvs: &mut Vec<[f32; 2]>,
    indices: &mut Vec<u32>,
    pos: [f32; 3],
) {
    let start_index = positions.len() as u32;
    positions.extend_from_slice(&[
        [pos[0], pos[1], pos[2] + 1.0],         // 左下 (0)
        [pos[0], pos[1] + 1.0, pos[2] + 1.0],   // 左上 (1)
        [pos[0] + 1.0, pos[1] + 1.0, pos[2] + 1.0], // 右上 (2)
        [pos[0] + 1.0, pos[1], pos[2] + 1.0],   // 右下 (3)
    ]);
    normals.extend_from_slice(&[[0.0, 0.0, -1.0]; 4]);
    // 修正UV坐标
    uvs.extend_from_slice(&[
        [0.0, 0.0], // 左下
        [0.0, 1.0], // 左上
        [1.0, 1.0], // 右上
        [1.0, 0.0], // 右下
    ]);
    indices.extend_from_slice(&[
        start_index, start_index + 3, start_index + 1, // 三角形1: 左下→左上→右上
        start_index + 1, start_index + 3, start_index + 2, // 三角形2: 左下→右上→右下
    ]);
}

