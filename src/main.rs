use bevy::{
    color::palettes::css::WHITE, dev_tools::fps_overlay::{FpsOverlayConfig, FpsOverlayPlugin}, pbr, prelude::*, render::{mesh::{Indices, PrimitiveTopology}, render_asset::RenderAssetUsages}, utils::HashMap
};
use bevy_flycam::prelude::*;
use bevy_inspector_egui::quick::WorldInspectorPlugin;

// 绘制线框的插件
use pbr::wireframe::{WireframeConfig, WireframePlugin};
/*
    参考：
    https://github.com/bevyengine/bevy/blob/main/examples/3d/wireframe.rs
    https://github.com/Adamkob12/Meshem/blob/main/examples/simple_example.rs
    关于开启透明立方体的模式  AlphaMode::Blend
    https://bevyengine.org/examples/3d-rendering/blend-modes/
    关于在屏幕中打印文字：
    https://bevyengine.org/examples/ui-user-interface/text/
*/

const CHUNK_WEIGHT: i32 = 32;
const CHUNK_HEIGHT: i32 = 64;

// 方块ID：
// 1:实体方块 0:空气
type BlockId = u8;
type Pos = [i32;3];


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
            WireframePlugin,
        ))
        .add_plugins(WorldInspectorPlugin::new())
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
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    asset_server: Res<AssetServer>,
) {
    let cube_mesh = create_cube_mesh();
    // 顶点总数量：Chunk_Weight * Chunk_Height * 8
    let vertices_count = format!("vertices_counts: {}", &cube_mesh.count_vertices());

    let cube_mesh = meshes.add(cube_mesh);
    let custom_texture_handle: Handle<Image> = asset_server.load("array_texture.png");

    // 绘制立方体 
    commands.spawn(PbrBundle {
        mesh: cube_mesh.clone(),
        material: materials.add(StandardMaterial {
            base_color_texture: Some(custom_texture_handle.clone()),
            base_color: Color::hsla(0.1, 0.1, 0.1, 0.1),        // 将立方体透明，只绘制线框，关掉会使透明失效
            alpha_mode: AlphaMode::Blend,                      // 开启透明模式
            ..default()
        }),
        transform: Transform::from_translation(Vec3::new(0.0, 0.0, 0.0)),
        ..Default::default()
    });

    //在窗口中打印当前顶点总数
    commands.spawn(
        TextBundle::from_section(
            vertices_count,                        // 显示顶点总数
            TextStyle::default(),
        )
        .with_style(Style {
            position_type: PositionType::Absolute,
            top: Val::Px(52.0),
            left: Val::Px(2.0),
            ..default()
        }),
    );

}

fn create_cube_mesh() -> Mesh {
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut uvs = Vec::new();
    let mut indices = Vec::new();

    // 将方块坐标存在hashmap中，k:pos, v:block_id
    let mut chunk_blocks:HashMap<Pos, BlockId> = HashMap::new();

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
    for x in 0..CHUNK_WEIGHT {
        for y in 0..CHUNK_HEIGHT {
            for z in 0..CHUNK_WEIGHT {
                // 可以从这里判断当前坐标的方块是否需要绘制
                // get 方块坐标，判断是否四周是空气还是实体方块，如果是实体方块则删掉该顶点：坐标的  噪声值 < 阈值 = 空气
                // let pos = [x as f32, y as f32, z as f32];
                if !chunk_blocks.contains_key(&[x, y, z]){
                    chunk_blocks.insert([x,y,z], 1);
                }
                // add_cube_to_mesh(&mut positions, &mut normals, &mut uvs, &mut indices, pos);
            }
        }
    }

    // 遍历chunk_blocks中所有方块，判断是否需要绘制
    // println!("方块数量：{}", chunk_blocks.len());
    for (pos, _block_id) in chunk_blocks.iter(){
        let pos = [pos[0], pos[1], pos[2]];
        // println!("方块坐标：{:?}, 方块id:{}", pos, block_id);
        // 判断每个方块的六个面旁边是否有实体方块
        if !(
            chunk_blocks.contains_key(&[pos[0], pos[1] + 1, pos[2]]) &&
            chunk_blocks.contains_key(&[pos[0], pos[1] - 1, pos[2]]) &&
            chunk_blocks.contains_key(&[pos[0] + 1, pos[1], pos[2]]) &&
            chunk_blocks.contains_key(&[pos[0] - 1, pos[1], pos[2]]) &&
            chunk_blocks.contains_key(&[pos[0], pos[1], pos[2] + 1]) &&
            chunk_blocks.contains_key(&[pos[0], pos[1], pos[2] - 1])
        ){
            add_cube_to_mesh(&mut positions, &mut normals, &mut uvs, &mut indices, [pos[0] as f32, pos[1] as f32, pos[2] as f32]);
        }
    }

    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD)
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
    .with_inserted_indices(Indices::U32(indices))
}

#[rustfmt::skip]
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
    // 顶点位置
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

    // 法线
    normals.extend_from_slice(&[
        [0.0, 1.0, 0.0], [0.0, 1.0, 0.0], [0.0, 1.0, 0.0], [0.0, 1.0, 0.0], // 顶面
        [0.0, -1.0, 0.0], [0.0, -1.0, 0.0], [0.0, -1.0, 0.0], [0.0, -1.0, 0.0], // 底面
        [1.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 0.0, 0.0], // 右侧面
        [-1.0, 0.0, 0.0], [-1.0, 0.0, 0.0], [-1.0, 0.0, 0.0], [-1.0, 0.0, 0.0], // 左侧面
        [0.0, 0.0, 1.0], [0.0, 0.0, 1.0], [0.0, 0.0, 1.0], [0.0, 0.0, 1.0], // 背面
        [0.0, 0.0, -1.0], [0.0, 0.0, -1.0], [0.0, 0.0, -1.0], [0.0, 0.0, -1.0], // 前面
    ]);

    // UV 坐标
    uvs.extend_from_slice(&[
        [0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0], // 顶面
        [0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0], // 底面
        [0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0], // 右侧面
        [0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0], // 左侧面
        [0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0], // 背面
        [0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0], // 前面
    ]);

    // 索引
    /*
        这个条件判断检查当前方块是否位于立方体的边缘或角落。
        pos[0] == 0.0 或 pos[0] == CHUNK_WEIGHT as f32：检查方块是否位于 x 轴的最小或最大位置。
        pos[1] == 0.0 或 pos[1] == CHUNK_HEIGHT as f32：检查方块是否位于 y 轴的最小或最大位置。
        pos[2] == 0.0 或 pos[2] == CHUNK_WEIGHT as f32：检查方块是否位于 z 轴的最小或最大位置。
     */
    if pos[0] >= 0.0 || pos[0] <= CHUNK_WEIGHT as f32 || pos[1] >= 0.0 || pos[1] <= CHUNK_HEIGHT as f32 || pos[2] >= 0.0 || pos[2] <= CHUNK_WEIGHT as f32 {
        indices.extend_from_slice(&[
            start_index + 0, start_index + 3, start_index + 1, start_index + 1, start_index + 3, start_index + 2, // 顶面
            start_index + 4, start_index + 5, start_index + 7, start_index + 5, start_index + 6, start_index + 7, // 底面
            start_index + 1, start_index + 2, start_index + 5, start_index + 5, start_index + 2, start_index + 6, // 右侧面
            start_index + 0, start_index + 4, start_index + 3, start_index + 3, start_index + 4, start_index + 7, // 左侧面
            start_index + 2, start_index + 3, start_index + 6, start_index + 6, start_index + 3, start_index + 7, // 背面
            start_index + 0, start_index + 1, start_index + 4, start_index + 4, start_index + 1, start_index + 5, // 前面
        ]);
    }
    
}
