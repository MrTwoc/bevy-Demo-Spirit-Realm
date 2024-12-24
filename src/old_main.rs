use std::collections::HashMap;
use bevy::{asset::RenderAssetUsages, color::palettes::css::WHITE, dev_tools::fps_overlay::{FpsOverlayConfig, FpsOverlayPlugin}, pbr::{self, wireframe::WireframeConfig}, prelude::*, render::mesh::{Indices, PrimitiveTopology}, text::FontSmoothing};
use bevy_flycam::prelude::*;
// 绘制线框的插件
use pbr::wireframe::WireframePlugin;
// 噪声生成
use fastnoise_lite::*;
// 显示世界内参数
use bevy_inspector_egui::quick::WorldInspectorPlugin;

// 单个区块的直径为32
const CHUNK_XYZ:i32 = 32;
const CHUNK_Y:i32 = 32;
// 玩家可视半径 2 = 前后左右上下各2个区块
const VIEW_DISTANCE:i32 = 2;
#[derive(Component)]
struct CameraPosInfo;
#[derive(Component)]
struct ChunkPosInfo;

// 方块ID：
// 1:实体方块 0:空气
// 2:Pos 区块内方块的坐标
type BlockId = u8;
type Pos = [i32;3];

type WorldPos = [i32;3];
type ChunkPos = [i32;3];
type ChunkStartPos = [i32;3];

// 显示实时FPS
struct OverlayColor;
impl OverlayColor {
    // const RED: Color = Color::srgb(1.0, 0.0, 0.0);
    const GREEN: Color = Color::srgb(0.0, 1.0, 0.0);
}

fn main() {
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
        .add_systems(Update, (get_camera_pos,show_pos_info,show_chunkpos_info))        //.run_if() ：当满足条件时运行系统

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
    mut commands : Commands,
    mut meshes : ResMut<Assets<Mesh>>,
    mut materials : ResMut<Assets<StandardMaterial>>,
){
    let block_mesh_handle = create_cube_mesh();
    let cube_mesh = meshes.add(block_mesh_handle);
    let cube_materials = materials.add(
        StandardMaterial{
            // base_color: Color::hsla(0.0, 0.1, 0.5, 1.0),        // 将立方体透明，只绘制线框，关掉会使透明失效
            // alpha_mode: AlphaMode::Blend,                      // 开启透明模式
            ..Default::default()
        }
    );
    
    /*
        区块加载距离   view_distance = 4 ：加载距离为4，即加载9x9x9 = 729个区块
        每个区块32x32x32
     */
    // let view_distance:i32 = 4;

    commands.spawn((
        Mesh3d(cube_mesh.clone()),
        MeshMaterial3d(cube_materials.clone()),
        // 这里需要一个函数，返回区块的坐标
        Transform::from_xyz(0.0, 0.0, 0.0),
    ));
    
    // 绘制屏幕text，显示各种坐标信息
    commands.spawn(
        (
            Text::new("GameInfo: \n"),
            Node{
                position_type: PositionType::Absolute,
                bottom: Val::Px(5.0),
                left: Val::Px(15.0),
                ..default()
            },
        )
    )
    .with_child((
        TextSpan::new("Unknown"),
        CameraPosInfo, 
    ))
    .with_child((
        TextSpan::new("Unknown"),
        ChunkPosInfo, 
    ));
}

fn show_pos_info(
    mut camera_pos: Query<&mut TextSpan, With<CameraPosInfo>>,
    query: Query<(&FlyCam, &mut Transform)>,
) {
    for (_camera, transform) in query.iter() {
        // 在UI中显示摄像机的世界坐标
        camera_pos.single_mut().0 = 
            format!("Camera Pos: {},{},{}\n", &transform.translation[0], &transform.translation[1], &transform.translation[2]);
    }
}
fn show_chunkpos_info(
    mut chunk_pos: Query<&mut TextSpan, With<ChunkPosInfo>>,
    query: Query<(&FlyCam, &mut Transform)>,
) {
    for (_camera, transform) in query.iter() {
        let pos = [transform.translation[0], transform.translation[1], transform.translation[2]];
        // 在UI中显示摄像机的世界坐标
        chunk_pos.single_mut().0 = 
            format!("Chunk Pos: {},{},{}\n", pos[0] as i32 / CHUNK_XYZ, pos[1] as i32 / CHUNK_Y, pos[2] as i32 / CHUNK_XYZ);
    }
}

// 这里应该实现一个方法检测摄像机移动，移动才会触发区块检测，移动才会触发区块加载
// 而不是每帧都检测，这样极大浪费性能
fn get_camera_pos(
    query: Query<(&FlyCam, &mut Transform)>,
){
    for (_camera, transform) in query.iter() {
        // 待优化：
        // 当摄像机坐标改变时，才触发区块检测
        manage_chunks(transform.translation);
    }
}

fn manage_chunks(
    camera_pos: Vec3,
){
    // 将玩家坐标由f32转为u32，方便后续计算区块坐标
    let [x,y,z] = [camera_pos[0] as i32, camera_pos[1] as i32, camera_pos[2] as i32]; 

    // 将世界坐标转为区块坐标，既是中心区块的坐标
    let [chunk_x,chunk_y,chunk_z] = worldpos_to_chunkpos([x,y,z]);

    // let [chunk_start_x,chunk_start_y,chunk_start_z] = chunkpos_to_worldpos([chunk_x,chunk_y,chunk_z]);

    // 将需要加载的区块坐标存储在hash中
    let mut chunk_count:HashMap<ChunkPos, u32> = HashMap::new();
    let mut count = 0;
    
    /*
        循环前if一下，避免重复加载
        判断原理：维护一个已加载区块的数组或hash存储区块坐标，当加载新的区块时，判断是否包含此坐标，未加载则加载，已加载则跳过
        for循环需要加载、卸载的区块坐标
     */
    for x in (chunk_x - VIEW_DISTANCE)..=(chunk_x + VIEW_DISTANCE){
        for y in (chunk_y - VIEW_DISTANCE)..=(chunk_y + VIEW_DISTANCE){
            for z in (chunk_z - VIEW_DISTANCE)..=(chunk_z + VIEW_DISTANCE){
                // 这里需要if判断是否已经加载过该区块, 将区块坐标存储在hash中
                if !chunk_count.contains_key(&[x, y, z]){
                    chunk_count.insert([x, y, z], count+1);
                }
            }
        }
    }

    for (pos,_num) in chunk_count.iter(){
        // 加载区块
        // spawn_chunk([pos[0], pos[1], pos[2]]);
        // 还可以在这里，检查是否需要卸载区块
        // unload_chunk(chunk_x, chunk_y, chunk_z);
    }
}


/*
    区块起始坐标：0,0,0
    单纯向上移动，变为：
    区块起始坐标：0,32,0
*/
/// 这个方法返回个区块 mesh给 manage_chunks 方法
fn load_chunk(chunk_pos: ChunkPos){
    // 将区块坐标转为世界坐标，再转为区块起始坐标
    let [chunk_start_x,chunk_start_y,chunk_start_z] = chunkpos_to_worldpos([chunk_pos[0] * CHUNK_XYZ,chunk_pos[1] * CHUNK_XYZ,chunk_pos[2] * CHUNK_XYZ]);
    // 加载区块
    // create_cube_mesh([chunk_start_x,chunk_start_y,chunk_start_z])
}
fn unload_chunk(chunk_pos: ChunkPos){
    // 卸载区块
}

fn worldpos_to_chunkpos(world_pos: WorldPos) -> ChunkPos{
    // 世界坐标转区块坐标   区块坐标 = 世界坐标 / 单区块大小
    [
        world_pos[0] / CHUNK_XYZ,
        world_pos[1] / CHUNK_XYZ,
        world_pos[2] / CHUNK_XYZ
    ]
}
fn chunkpos_to_worldpos(chunk_pos: ChunkPos) -> WorldPos{
    // 区块坐标转世界坐标
    // 区块坐标 * 单区块大小 = 区块起始坐标，再加偏移量，就是玩家的世界坐标
    [
        chunk_pos[0] * CHUNK_XYZ,
        chunk_pos[1] * CHUNK_XYZ,
        chunk_pos[2] * CHUNK_XYZ
    ]
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
    for x in 0..CHUNK_XYZ {
        for z in 0..CHUNK_XYZ {
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