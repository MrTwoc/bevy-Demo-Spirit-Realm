//! This example demonstrates how to use the `Camera::viewport_to_world` method.

use std::collections::HashMap;

use bevy::{asset::RenderAssetUsages, color::palettes::css::WHITE, dev_tools::fps_overlay::{FpsOverlayConfig, FpsOverlayPlugin}, pbr::{self, wireframe::WireframeConfig}, prelude::*, render::mesh::{Indices, PrimitiveTopology}, text::FontSmoothing};
use bevy_flycam::prelude::*;
// 绘制线框的插件
use pbr::wireframe::WireframePlugin;
// 噪声生成
use fastnoise_lite::*;

const CHUNK_XZ:i32 = 512;
const CHUNK_Y:i32 = 32;

// 方块ID：
// 1:实体方块 0:空气
type BlockId = u8;
type Pos = [i32;3];

// 显示实时FPS
struct OverlayColor;
impl OverlayColor {
    // const RED: Color = Color::srgb(1.0, 0.0, 0.0);
    const GREEN: Color = Color::srgb(0.0, 1.0, 0.0);
}

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
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
        .run();
}

fn setup(
    mut commands : Commands,
    mut meshes : ResMut<Assets<Mesh>>,
    mut materials : ResMut<Assets<StandardMaterial>>,
){

    let block_mesh_handle = create_cube_mesh();
    let cube_mesh = meshes.add(block_mesh_handle);
    let cube_materials = materials.add(
        StandardMaterial{
            base_color: Color::hsla(0.0, 0.1, 0.5, 0.0),        // 将立方体透明，只绘制线框，关掉会使透明失效
            // alpha_mode: AlphaMode::Blend,                      // 开启透明模式
            ..Default::default()
        }
    );
    commands.spawn((
        Mesh3d(cube_mesh.clone()),
        MeshMaterial3d(cube_materials.clone()),
        Transform::from_xyz(0.0, 0., 0.0),
    ));
}

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
    noise.set_seed(Some(420));
    noise.set_fractal_type(Some(FractalType::FBm));

/*
    遮挡剔除逻辑：
    检测坐标的方块周围是否被其他方块遮挡，如果被遮挡从pos中删除顶点
    根据噪声值判断，若该点噪声值低于设定阈值，则为空气，高于阈值就是实体方块
    再根据方块块与空气接触，判断是否绘制该方块

    方法一：
    先添加方块坐标，再在添加mesh的时候，判断方块六个面是否有方块，如果被遮挡则不添加顶点
    添加一个is_block方法，再将左右坐标存储在一个数据结构中，is_block方法判断六个面是否贴着方块
    若是上面有方块：不渲染顶面的顶点
    下面有方块：不渲染底面的顶点
    左面有方块：不渲染左面的顶点

*/
    for x in 0..CHUNK_XZ {
        for z in 0..CHUNK_XZ {
            let negative_1_to_1 = noise.get_noise_2d(x as f32, z as f32);
            let noise_date = (negative_1_to_1 + 1.) / 2.;
            
            let block_y = (noise_date * CHUNK_Y as f32) as i32;
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
        // println!("方块坐标：{:?}, 方块id:{}", pos, block_id);
        // 判断每个方块的六个面旁边是否有实体方块
        /*
            后续阶段优化方案：
            将此判断拆开，单独判断每个面是否贴着方块，结果取反，再添加顶点，绘制mesh面
            例如：判断当前方块的 y+ 面，是否被方块遮挡，为真则取反(接触空气的面)，添加顶点，绘制该面
            pos[0]:X轴   pos[1]:Y轴   pos[2]:Z轴
            if !(chunk_blocks.contains_key(&[pos[0], pos[1] + 1, pos[2]])) {
                添加 Y+ 面顶点
                绘制 Y+ 面
            }
         */
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

    // Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD)
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

    /*
        TODO:
        各种优化剔除：
        遮挡剔除、视锥剔除、LOD技术、八叉树等
        一、遮挡剔除：
        判断坐标的方块周围是否被其他方块遮挡，如果被遮挡从pos中删除顶点
        fn is_cube_occluded(pos: [f32; 3]) -> bool {}
        二、视锥剔除：
        判断坐标的方块是否在视锥内，如果不在视锥内从pos中删除顶点(没有方块遮挡与空气接触，但不在视锥内的方块)
        fn is_cube_in_view(pos: [f32; 3]) -> bool {}
        三、LOD技术：
        借鉴我的世界中《遥远的地平线》模组
     */
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
    
    /*
        目前add_cube_mesh函数绘制了一整个的方块，下阶段的绘制中，
        可以将此方法，分成六个单独的方法(代表立方体的六个面)，分别绘制立方体的六个面，
        用以更好的搭配优化代码
        add_cube_mesh 可以改为 add_cube_mesh_Y+
     */
}