//! Voxel chunk: block storage + face-culled mesh generation.
//!
//! 使用调色板压缩（PalettedChunkData）优化内存占用：
//! - Empty: 0 字节（全空气）
//! - Uniform: 2 字节（全同一种方块）
//! - Paletted: ~4KB（调色板压缩，32³=32768体素）

use bevy::{
    asset::RenderAssetUsages, mesh::Indices, prelude::*, render::render_resource::PrimitiveTopology,
};
use noise::{Fbm, MultiFractal, NoiseFn, Simplex};
use std::collections::HashMap;
use std::hash::Hash;

use crate::chunk_dirty::ChunkMeshHandle;
use crate::resource_pack::ResourcePackManager;

/// Size of one dimension of a chunk (32³ blocks per chunk).
pub const CHUNK_SIZE: usize = 32;
/// Total number of voxels in a chunk (32³ = 32768).
const CHUNK_VOLUME: usize = CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE;

/// A single block type identifier.
/// 0 = air (not rendered), 1 = grass, 2 = stone, 3 = dirt, 4 = sand.
pub type BlockId = u8;

/// 调色板压缩的区块数据。
///
/// 使用调色板（palette）存储不同的方块类型，每个体素只存储调色板索引。
/// 对于32³=32768个体素：
/// - 如果只有1种方块：调色板1项 + 索引0位 = ~2字节
/// - 如果有2-256种方块：调色板N项 + 索引8位 = ~32KB + N*2字节
/// - 使用4位索引（最多16种方块）：调色板16项 + 索引4位 = ~16KB + 32字节
///
/// 当前实现使用8位索引（最多256种方块），适合大多数场景。
#[derive(Clone)]
pub struct PalettedChunkData {
    /// 调色板：索引 -> BlockId
    palette: Vec<BlockId>,
    /// 反向调色板：BlockId -> 索引（用于快速查找）
    reverse_palette: HashMap<BlockId, u8>,
    /// 体素索引数组：每个字节是调色板索引
    indices: Vec<u8>,
}

impl PalettedChunkData {
    /// 创建全空气的调色板区块
    pub fn new() -> Self {
        let mut palette = Vec::new();
        palette.push(0); // 索引0 = 空气
        let mut reverse_palette = HashMap::new();
        reverse_palette.insert(0, 0);

        Self {
            palette,
            reverse_palette,
            indices: vec![0; CHUNK_VOLUME],
        }
    }

    /// 从BlockId数组创建调色板区块
    pub fn from_blocks(blocks: &[BlockId]) -> Self {
        let mut palette = Vec::new();
        let mut reverse_palette = HashMap::new();
        let mut indices = Vec::with_capacity(blocks.len());

        // 收集所有不同的方块类型
        for &block_id in blocks {
            if !reverse_palette.contains_key(&block_id) {
                let index = palette.len() as u8;
                palette.push(block_id);
                reverse_palette.insert(block_id, index);
            }
        }

        // 生成索引数组
        for &block_id in blocks {
            let index = reverse_palette[&block_id];
            indices.push(index);
        }

        Self {
            palette,
            reverse_palette,
            indices,
        }
    }

    /// 获取指定位置的方块ID
    pub fn get(&self, x: usize, y: usize, z: usize) -> BlockId {
        if x >= CHUNK_SIZE || y >= CHUNK_SIZE || z >= CHUNK_SIZE {
            return 0;
        }
        let idx = z * CHUNK_SIZE * CHUNK_SIZE + y * CHUNK_SIZE + x;
        let palette_index = self.indices[idx] as usize;
        self.palette[palette_index]
    }

    /// 设置指定位置的方块ID
    pub fn set(&mut self, x: usize, y: usize, z: usize, id: BlockId) {
        if x >= CHUNK_SIZE || y >= CHUNK_SIZE || z >= CHUNK_SIZE {
            return;
        }
        let idx = z * CHUNK_SIZE * CHUNK_SIZE + y * CHUNK_SIZE + x;

        // 获取或创建调色板索引
        let palette_index = if let Some(&index) = self.reverse_palette.get(&id) {
            index
        } else {
            // 新方块类型，添加到调色板
            let index = self.palette.len() as u8;
            self.palette.push(id);
            self.reverse_palette.insert(id, index);
            index
        };

        self.indices[idx] = palette_index;
    }

    /// 转换为BlockId数组（用于兼容旧代码）
    pub fn to_blocks(&self) -> Vec<BlockId> {
        self.indices
            .iter()
            .map(|&idx| self.palette[idx as usize])
            .collect()
    }

    /// 获取调色板中的方块类型数量
    pub fn palette_len(&self) -> usize {
        self.palette.len()
    }

    /// 检查是否为空气区块
    pub fn is_empty(&self) -> bool {
        self.palette.len() == 1 && self.palette[0] == 0
    }

    /// 检查是否为单一方块类型
    pub fn is_uniform(&self) -> bool {
        self.palette.len() == 1
    }

    /// 获取单一方块类型（如果是Uniform）
    pub fn uniform_block(&self) -> Option<BlockId> {
        if self.is_uniform() {
            Some(self.palette[0])
        } else {
            None
        }
    }
}

/// Face direction on a block.
#[derive(Clone, Copy)]
pub enum Face {
    Top,
    Bottom,
    Right,
    Left,
    Front,
    Back,
}

impl Face {
    /// 将 Face 转换为资源包映射表中的面名称
    pub fn to_face_name(&self) -> &'static str {
        match self {
            Face::Top => "top",
            Face::Bottom => "bottom",
            _ => "side", // Right, Left, Front, Back 都用 "side"
        }
    }
}

/// All 6 faces of a block in order: +X, -X, +Y, -Y, +Z, -Z
const FACES: [(Face, [i32; 3]); 6] = [
    (Face::Right, [1, 0, 0]),
    (Face::Left, [-1, 0, 0]),
    (Face::Top, [0, 1, 0]),
    (Face::Bottom, [0, -1, 0]),
    (Face::Front, [0, 0, 1]),
    (Face::Back, [0, 0, -1]),
];

/// 6 个方向的邻居区块数据，用于跨区块面剔除。
///
/// 索引顺序与 FACES 一致：[+X, -X, +Y, -Y, +Z, -Z]。
/// 如果某个方向没有邻居（未加载），对应位置为 `None`，
/// 面剔除时会将缺失的邻居视为空气（即保留该面）。
pub struct ChunkNeighbors {
    /// 6 个方向的邻居区块数据引用
    pub neighbors: [Option<BlockId>; 6],
    /// 6 个方向的邻居完整数据（用于跨边界查询）
    pub neighbor_data: [Option<Vec<BlockId>>; 6],
}

impl ChunkNeighbors {
    /// 创建空的邻居数据（所有方向都没有邻居）
    pub fn empty() -> Self {
        Self {
            neighbors: [None; 6],
            neighbor_data: std::array::from_fn(|_| None),
        }
    }

    /// 获取指定方向邻居在 (x, y, z) 位置的方块 ID。
    /// 如果邻居不存在，返回 0（空气）。
    pub fn get_neighbor_block(&self, face_index: usize, x: usize, y: usize, z: usize) -> BlockId {
        if let Some(ref data) = self.neighbor_data[face_index] {
            if x < CHUNK_SIZE && y < CHUNK_SIZE && z < CHUNK_SIZE {
                let idx = z * CHUNK_SIZE * CHUNK_SIZE + y * CHUNK_SIZE + x;
                data[idx]
            } else {
                0
            }
        } else {
            0 // 没有邻居数据，视为空气
        }
    }
}

/// Chunk data: three-state storage for a 32x32x32 voxel chunk.
///
/// 使用调色板压缩优化内存：
/// - Empty: 0 字节（全空气）
/// - Uniform: 2 字节（全同一种方块）
/// - Paletted: ~4KB（调色板压缩，32³=32768体素）
#[derive(Component, Clone)]
pub enum ChunkData {
    Empty,
    Uniform(BlockId),
    Paletted(PalettedChunkData),
}

impl ChunkData {
    pub fn new() -> Self {
        Self::Empty
    }

    pub fn filled(block_id: BlockId) -> Self {
        Self::Uniform(block_id)
    }

    fn flatten(x: usize, y: usize, z: usize) -> usize {
        z * CHUNK_SIZE * CHUNK_SIZE + y * CHUNK_SIZE + x
    }

    pub fn get(&self, x: usize, y: usize, z: usize) -> BlockId {
        match self {
            ChunkData::Empty => 0,
            ChunkData::Uniform(id) => *id,
            ChunkData::Paletted(data) => data.get(x, y, z),
        }
    }

    pub fn set(&mut self, x: usize, y: usize, z: usize, id: BlockId) {
        if x >= CHUNK_SIZE || y >= CHUNK_SIZE || z >= CHUNK_SIZE {
            return;
        }

        match self {
            ChunkData::Empty => {
                *self = ChunkData::Uniform(id);
            }
            ChunkData::Uniform(current_id) => {
                if *current_id != id {
                    // 从Uniform转换为Paletted
                    let mut data = PalettedChunkData::new();
                    // 填充所有体素为当前方块
                    for oz in 0..CHUNK_SIZE {
                        for oy in 0..CHUNK_SIZE {
                            for ox in 0..CHUNK_SIZE {
                                data.set(ox, oy, oz, *current_id);
                            }
                        }
                    }
                    // 设置新方块
                    data.set(x, y, z, id);
                    *self = ChunkData::Paletted(data);
                }
            }
            ChunkData::Paletted(data) => {
                data.set(x, y, z, id);
            }
        }
    }

    /// 将 ChunkData 转换为 Vec<BlockId>（用于传递给邻居查询）
    pub fn to_vec(&self) -> Vec<BlockId> {
        match self {
            ChunkData::Empty => vec![0; CHUNK_VOLUME],
            ChunkData::Uniform(id) => vec![*id; CHUNK_VOLUME],
            ChunkData::Paletted(data) => data.to_blocks(),
        }
    }

    /// 判断指定面是否可见（需要渲染）。
    ///
    /// 当邻居在区块边界内时，直接查询本区块数据。
    /// 当邻居在区块边界外时，通过 `neighbors` 查询邻居区块数据。
    pub fn is_face_visible(
        &self,
        x: usize,
        y: usize,
        z: usize,
        face: &[i32; 3],
        face_index: usize,
        neighbors: &ChunkNeighbors,
    ) -> bool {
        let nx = x as i32 + face[0];
        let ny = y as i32 + face[1];
        let nz = z as i32 + face[2];

        let current_id = self.get(x, y, z);

        // 邻居在区块边界内，直接查询本区块
        if nx >= 0
            && ny >= 0
            && nz >= 0
            && nx < CHUNK_SIZE as i32
            && ny < CHUNK_SIZE as i32
            && nz < CHUNK_SIZE as i32
        {
            return self.get(nx as usize, ny as usize, nz as usize) != current_id;
        }

        // 邻居在区块边界外，查询邻居区块数据
        let neighbor_x = nx.rem_euclid(CHUNK_SIZE as i32) as usize;
        let neighbor_y = ny.rem_euclid(CHUNK_SIZE as i32) as usize;
        let neighbor_z = nz.rem_euclid(CHUNK_SIZE as i32) as usize;

        let neighbor_id =
            neighbors.get_neighbor_block(face_index, neighbor_x, neighbor_y, neighbor_z);
        neighbor_id != current_id
    }

    /// 获取内存占用估算（字节）
    pub fn memory_usage(&self) -> usize {
        match self {
            ChunkData::Empty => 0,
            ChunkData::Uniform(_) => 2,
            ChunkData::Paletted(data) => {
                // 调色板 + 索引数组
                data.palette_len() * 2 + CHUNK_VOLUME
            }
        }
    }
}

impl Default for ChunkData {
    fn default() -> Self {
        Self::Empty
    }
}

pub type Chunk = ChunkData;

/// Generates a face-culled mesh for the chunk.
/// Returns (positions, uvs, normals, indices).
/// UV 坐标从 ResourcePackManager 的动态 Atlas 中查找。
///
/// `neighbors` 提供 6 个方向的邻居区块数据，用于跨区块面剔除。
/// 当邻居不存在时，边界面上的方块面会被保留（视为空气）。
pub fn generate_chunk_mesh(
    chunk: &Chunk,
    resource_pack: &ResourcePackManager,
    neighbors: &ChunkNeighbors,
) -> (Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<[f32; 3]>, Vec<u32>) {
    let mut positions = Vec::new();
    let mut uvs = Vec::new();
    let mut normals = Vec::new();
    let mut indices = Vec::new();

    for z in 0..CHUNK_SIZE {
        for y in 0..CHUNK_SIZE {
            for x in 0..CHUNK_SIZE {
                let block_id = chunk.get(x, y, z);
                if block_id == 0 {
                    continue;
                }

                for (face_index, (face, offset)) in FACES.iter().cloned().enumerate() {
                    if !chunk.is_face_visible(x, y, z, &offset, face_index, neighbors) {
                        continue;
                    }

                    let base_index = positions.len() as u32;
                    let face_name = face.to_face_name();

                    // 从资源包查找 UV 坐标
                    let uv = resource_pack
                        .get_block_uv(block_id, face_name)
                        .unwrap_or((0.0, 1.0, 0.0, 1.0));

                    let (face_verts, face_uvs, face_normal) = face_quad(x, y, z, face, uv);
                    positions.extend(face_verts);
                    uvs.extend(face_uvs);
                    normals.extend([face_normal; 4]);
                    indices.extend([
                        base_index,
                        base_index + 2,
                        base_index + 1,
                        base_index,
                        base_index + 3,
                        base_index + 2,
                    ]);
                }
            }
        }
    }

    (positions, uvs, normals, indices)
}

/// Returns the 4 vertices, UVs, and normal for a single face.
fn face_quad(
    x: usize,
    y: usize,
    z: usize,
    face: Face,
    uv: (f32, f32, f32, f32),
) -> ([[f32; 3]; 4], [[f32; 2]; 4], [f32; 3]) {
    let (verts, normal) = match face {
        Face::Top => (
            [
                [x as f32, y as f32 + 1.0, z as f32],
                [x as f32 + 1.0, y as f32 + 1.0, z as f32],
                [x as f32 + 1.0, y as f32 + 1.0, z as f32 + 1.0],
                [x as f32, y as f32 + 1.0, z as f32 + 1.0],
            ],
            [0.0, 1.0, 0.0],
        ),
        Face::Bottom => (
            [
                [x as f32, y as f32, z as f32 + 1.0],
                [x as f32 + 1.0, y as f32, z as f32 + 1.0],
                [x as f32 + 1.0, y as f32, z as f32],
                [x as f32, y as f32, z as f32],
            ],
            [0.0, -1.0, 0.0],
        ),
        Face::Right => (
            [
                [x as f32 + 1.0, y as f32, z as f32],
                [x as f32 + 1.0, y as f32, z as f32 + 1.0],
                [x as f32 + 1.0, y as f32 + 1.0, z as f32 + 1.0],
                [x as f32 + 1.0, y as f32 + 1.0, z as f32],
            ],
            [1.0, 0.0, 0.0],
        ),
        Face::Left => (
            [
                [x as f32, y as f32, z as f32 + 1.0],
                [x as f32, y as f32, z as f32],
                [x as f32, y as f32 + 1.0, z as f32],
                [x as f32, y as f32 + 1.0, z as f32 + 1.0],
            ],
            [-1.0, 0.0, 0.0],
        ),
        Face::Front => (
            [
                [x as f32 + 1.0, y as f32, z as f32 + 1.0],
                [x as f32, y as f32, z as f32 + 1.0],
                [x as f32, y as f32 + 1.0, z as f32 + 1.0],
                [x as f32 + 1.0, y as f32 + 1.0, z as f32 + 1.0],
            ],
            [0.0, 0.0, 1.0],
        ),
        Face::Back => (
            [
                [x as f32, y as f32, z as f32],
                [x as f32 + 1.0, y as f32, z as f32],
                [x as f32 + 1.0, y as f32 + 1.0, z as f32],
                [x as f32, y as f32 + 1.0, z as f32],
            ],
            [0.0, 0.0, -1.0],
        ),
    };

    let (u_min, u_max, v_min, v_max) = uv;
    let face_uvs = [
        [u_min, v_max],
        [u_max, v_max],
        [u_max, v_min],
        [u_min, v_min],
    ];

    (verts, face_uvs, normal)
}

// --------------------------------------------------------------------------
// Terrain helpers
// --------------------------------------------------------------------------

/// 地形基准高度（世界 Y 坐标）。噪声在此基础上起伏。
const TERRAIN_BASE_HEIGHT: i32 = 16;
/// 地形起伏幅度。噪声值 ±1.0 映射为 ±此值。
const TERRAIN_AMPLITUDE: f64 = 32.0;
/// 噪声种子，保证所有区块生成一致的地形。
const TERRAIN_SEED: u32 = 42;
/// 泥土层厚度（地表以下多少格是泥土，再往下是石头）。
const DIRT_LAYER_DEPTH: i32 = 4;
/// 沙滩高度阈值：地表高度低于此值时使用沙子而非草地。
const SAND_HEIGHT_THRESHOLD: i32 = TERRAIN_BASE_HEIGHT - 8;

/// 创建全局 FBM 噪声生成器（Simplex + 分形布朗运动）。
fn create_terrain_noise() -> Fbm<Simplex> {
    Fbm::<Simplex>::new(TERRAIN_SEED)
        .set_octaves(4)
        .set_frequency(0.005)
        .set_lacunarity(2.0)
        .set_persistence(0.5)
}

/// Fills a chunk with noise-generated terrain.
///
/// 使用 Simplex FBM 噪声在 XZ 平面采样，生成有起伏的自然地形。
/// 地形分层：地表=草地/沙子，浅层=泥土，深层=石头。
///
/// `coord` 是区块的世界坐标，用于将局部 (x, z) 转换为世界坐标进行噪声采样，
/// 保证区块边界处地形连续。
pub fn fill_terrain(chunk: &mut Chunk, coord: &ChunkCoord) {
    let noise = create_terrain_noise();

    for z in 0..CHUNK_SIZE {
        for x in 0..CHUNK_SIZE {
            // 将局部坐标转换为世界坐标
            let world_x = coord.cx as f64 * CHUNK_SIZE as f64 + x as f64;
            let world_z = coord.cz as f64 * CHUNK_SIZE as f64 + z as f64;

            // 采样 FBM 噪声，输出范围约 -1.0 ~ +1.0
            let noise_val = noise.get([world_x, world_z]);
            // 映射为地表高度
            let surface_height = TERRAIN_BASE_HEIGHT + (noise_val * TERRAIN_AMPLITUDE) as i32;

            for y in 0..CHUNK_SIZE {
                // 将局部 Y 转换为世界 Y
                let world_y = coord.cy as i32 * CHUNK_SIZE as i32 + y as i32;

                if world_y > surface_height {
                    continue; // 空气，不需要设置（默认就是 0）
                }

                let block_id = if world_y == surface_height {
                    // 地表层：根据高度选择草地或沙子
                    if surface_height < SAND_HEIGHT_THRESHOLD {
                        4 // sand
                    } else {
                        1 // grass
                    }
                } else if world_y > surface_height - DIRT_LAYER_DEPTH {
                    3 // dirt
                } else {
                    2 // stone
                };

                chunk.set(x, y, z, block_id);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Bevy spawn systems
// ---------------------------------------------------------------------------

/// Spawns a single chunk entity with texture-mapped mesh.
///
/// 返回 `(Entity, Handle<Mesh>, Handle<StandardMaterial>)`。
/// - `Entity` 用于 ECS 组件插入
/// - `Handle<Mesh>` 和 `Handle<StandardMaterial>` 用于在卸载/淘汰时从 `Assets` 中移除，
///   避免 GPU 内存泄漏（P0 #1 修复）
///
/// mesh 和 material handle 同时通过 `ChunkMeshHandle` 组件存储在实体上，
/// 用于脏块重建时移除旧资源。
pub fn spawn_chunk_entity(
    commands: &mut Commands,
    materials: &mut Assets<StandardMaterial>,
    meshes: &mut Assets<Mesh>,
    chunk: Chunk,
    position: Vec3,
    resource_pack: &ResourcePackManager,
    atlas_texture: &Handle<Image>,
    neighbors: &ChunkNeighbors,
) -> (Entity, Handle<Mesh>, Handle<StandardMaterial>) {
    let (positions, uvs, normals, indices) = generate_chunk_mesh(&chunk, resource_pack, neighbors);

    let mesh_handle = meshes.add(
        Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
        .with_inserted_indices(Indices::U32(indices)),
    );

    let mat_handle = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        base_color_texture: Some(atlas_texture.clone()),
        ..default()
    });

    let entity = commands
        .spawn((
            chunk,
            Mesh3d(mesh_handle.clone()),
            MeshMaterial3d(mat_handle.clone()),
            ChunkMeshHandle {
                mesh: mesh_handle.clone(),
                material: mat_handle.clone(),
            },
            Transform::from_translation(position),
            // Visibility::default() 会启用 Bevy 的视锥剔除（Frustum Culling）。
            // Bevy 的 calculate_bounds 系统会自动为 Mesh3d 实体计算 Aabb，
            // 从而启用视锥剔除和遮挡剔除。
            Visibility::default(),
        ))
        .id();

    (entity, mesh_handle, mat_handle)
}

// ---------------------------------------------------------------------------
// Chunk coordinate system
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Hash, Eq, PartialEq)]
pub struct BlockPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl BlockPos {
    pub fn from_world(world: Vec3) -> Self {
        Self {
            x: world.x.floor() as i32,
            y: world.y.floor() as i32,
            z: world.z.floor() as i32,
        }
    }
}

#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq)]
pub struct ChunkCoord {
    pub cx: i32,
    pub cy: i32,
    pub cz: i32,
}

impl ChunkCoord {
    pub fn from_world(world_pos: Vec3) -> Self {
        Self {
            cx: (world_pos.x / CHUNK_SIZE as f32).floor() as i32,
            cy: (world_pos.y / CHUNK_SIZE as f32).floor() as i32,
            cz: (world_pos.z / CHUNK_SIZE as f32).floor() as i32,
        }
    }

    pub fn to_world_origin(self) -> Vec3 {
        Vec3::new(
            self.cx as f32 * CHUNK_SIZE as f32,
            self.cy as f32 * CHUNK_SIZE as f32,
            self.cz as f32 * CHUNK_SIZE as f32,
        )
    }
}

pub fn world_to_chunk(local_pos: BlockPos) -> Option<(ChunkCoord, usize)> {
    let cx = local_pos.x.div_euclid(CHUNK_SIZE as i32);
    let cy = local_pos.y.div_euclid(CHUNK_SIZE as i32);
    let cz = local_pos.z.div_euclid(CHUNK_SIZE as i32);

    let lx = local_pos.x.rem_euclid(CHUNK_SIZE as i32) as usize;
    let ly = local_pos.y.rem_euclid(CHUNK_SIZE as i32) as usize;
    let lz = local_pos.z.rem_euclid(CHUNK_SIZE as i32) as usize;

    Some((
        ChunkCoord { cx, cy, cz },
        lz * CHUNK_SIZE * CHUNK_SIZE + ly * CHUNK_SIZE + lx,
    ))
}

pub fn mark_block_dirty(
    coord: ChunkCoord,
    local_pos: (usize, usize, usize),
    dirty_chunks: &mut Vec<ChunkCoord>,
) {
    dirty_chunks.push(coord);

    let (x, y, z) = local_pos;
    if x == 0 {
        dirty_chunks.push(ChunkCoord {
            cx: coord.cx - 1,
            cy: coord.cy,
            cz: coord.cz,
        });
    }
    if x == CHUNK_SIZE - 1 {
        dirty_chunks.push(ChunkCoord {
            cx: coord.cx + 1,
            cy: coord.cy,
            cz: coord.cz,
        });
    }
    if y == 0 {
        dirty_chunks.push(ChunkCoord {
            cx: coord.cx,
            cy: coord.cy - 1,
            cz: coord.cz,
        });
    }
    if y == CHUNK_SIZE - 1 {
        dirty_chunks.push(ChunkCoord {
            cx: coord.cx,
            cy: coord.cy + 1,
            cz: coord.cz,
        });
    }
    if z == 0 {
        dirty_chunks.push(ChunkCoord {
            cx: coord.cx,
            cy: coord.cy,
            cz: coord.cz - 1,
        });
    }
    if z == CHUNK_SIZE - 1 {
        dirty_chunks.push(ChunkCoord {
            cx: coord.cx,
            cy: coord.cy,
            cz: coord.cz + 1,
        });
    }
}
