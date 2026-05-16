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
use std::sync::Arc;

use crate::chunk_dirty::ChunkMeshHandle;
use crate::resource_pack::{ResourcePackManager, VoxelMaterial};

/// Size of one dimension of a chunk (32³ blocks per chunk).
pub const CHUNK_SIZE: usize = 32;
/// Total number of voxels in a chunk (32³ = 32768).
const CHUNK_VOLUME: usize = CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE;

/// A single block type identifier.
pub type BlockId = u8;

/// 判断方块 ID 是否为实体（不透明）方块，用于面剔除优化。
///
/// 实体方块（草地=1, 石头=2, 泥土=3, 沙=4）完全遮挡相邻方向的拼接面，
/// 即使方块 ID 不同，两个实体方块之间也不应渲染面。
/// 非实体方块（空气=0, 水=5）不遮挡面，其与实体方块之间的界面应当渲染。
///
/// 这是核心优化：旧代码使用 `neighbor_id != current_id` 检查，
/// 导致不同实体类型（如石头 vs 泥土）之间生成大量冗余三角面。
#[inline]
pub fn is_block_solid(block_id: BlockId) -> bool {
    match block_id {
        0 | 5 => false, // 空气、水 → 非实体，不遮挡
        _ => true,      // 草地(1)、石头(2)、泥土(3)、沙(4) → 实体，完全遮挡
    }
}

/// 调色板压缩的区块数据。
#[derive(Clone)]
pub struct PalettedChunkData {
    palette: Vec<BlockId>,
    reverse_palette: HashMap<BlockId, u8>,
    indices: Vec<u8>,
}

impl PalettedChunkData {
    pub fn new() -> Self {
        let mut palette = Vec::new();
        palette.push(0);
        let mut reverse_palette = HashMap::new();
        reverse_palette.insert(0, 0);

        Self {
            palette,
            reverse_palette,
            indices: vec![0; CHUNK_VOLUME],
        }
    }

    pub fn from_blocks(blocks: &[BlockId]) -> Self {
        let mut palette = Vec::new();
        let mut reverse_palette = HashMap::new();
        let mut indices = Vec::with_capacity(blocks.len());

        for &block_id in blocks {
            if !reverse_palette.contains_key(&block_id) {
                let index = palette.len() as u8;
                palette.push(block_id);
                reverse_palette.insert(block_id, index);
            }
        }

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

    pub fn get(&self, x: usize, y: usize, z: usize) -> BlockId {
        if x >= CHUNK_SIZE || y >= CHUNK_SIZE || z >= CHUNK_SIZE {
            return 0;
        }
        let idx = z * CHUNK_SIZE * CHUNK_SIZE + y * CHUNK_SIZE + x;
        let palette_index = self.indices[idx] as usize;
        self.palette[palette_index]
    }

    pub fn add_or_get_palette_index(&mut self, id: BlockId) -> u8 {
        if let Some(&index) = self.reverse_palette.get(&id) {
            index
        } else {
            let index = self.palette.len() as u8;
            self.palette.push(id);
            self.reverse_palette.insert(id, index);
            index
        }
    }

    pub fn set(&mut self, x: usize, y: usize, z: usize, id: BlockId) {
        if x >= CHUNK_SIZE || y >= CHUNK_SIZE || z >= CHUNK_SIZE {
            return;
        }
        let idx = z * CHUNK_SIZE * CHUNK_SIZE + y * CHUNK_SIZE + x;
        let palette_index = self.add_or_get_palette_index(id);
        self.indices[idx] = palette_index;
    }

    pub fn to_blocks(&self) -> Vec<BlockId> {
        self.indices
            .iter()
            .map(|&idx| self.palette[idx as usize])
            .collect()
    }

    pub fn palette_len(&self) -> usize {
        self.palette.len()
    }

    pub fn is_empty(&self) -> bool {
        if self.palette.len() == 1 && self.palette[0] == 0 {
            return true;
        }
        if let Some(&air_index) = self.reverse_palette.get(&0) {
            self.indices.iter().all(|&idx| idx == air_index)
        } else {
            false
        }
    }

    pub fn is_uniform(&self) -> bool {
        self.palette.len() == 1
    }

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
    pub fn to_face_name(&self) -> &'static str {
        match self {
            Face::Top => "top",
            Face::Bottom => "bottom",
            _ => "side",
        }
    }

    pub const fn face_index(&self) -> usize {
        match self {
            Face::Top => 0,
            Face::Bottom => 1,
            _ => 2,
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

const FACE_UV_INDICES: [usize; 6] = [2, 2, 0, 1, 2, 2];

/// 6 个方向的邻居区块数据，用于跨区块面剔除。
pub struct ChunkNeighbors {
    pub neighbor_data: [Option<Arc<Vec<BlockId>>>; 6],
}

impl ChunkNeighbors {
    pub fn empty() -> Self {
        Self {
            neighbor_data: std::array::from_fn(|_| None),
        }
    }

    pub fn get_neighbor_block(&self, face_index: usize, x: usize, y: usize, z: usize) -> BlockId {
        if let Some(ref data) = self.neighbor_data[face_index] {
            if x < CHUNK_SIZE && y < CHUNK_SIZE && z < CHUNK_SIZE {
                let idx = z * CHUNK_SIZE * CHUNK_SIZE + y * CHUNK_SIZE + x;
                data[idx]
            } else {
                0
            }
        } else {
            0
        }
    }
}

/// Chunk data: three-state storage for a 32x32x32 voxel chunk.
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
                    let mut data = PalettedChunkData::new();
                    if *current_id != 0 {
                        let palette_index = data.add_or_get_palette_index(*current_id);
                        data.indices.fill(palette_index);
                    }
                    data.set(x, y, z, id);
                    *self = ChunkData::Paletted(data);
                }
            }
            ChunkData::Paletted(data) => {
                data.set(x, y, z, id);
            }
        }
    }

    pub fn to_shared_vec(&self) -> Arc<Vec<BlockId>> {
        match self {
            ChunkData::Empty => Arc::new(vec![0; CHUNK_VOLUME]),
            ChunkData::Uniform(id) => Arc::new(vec![*id; CHUNK_VOLUME]),
            ChunkData::Paletted(data) => Arc::new(data.to_blocks()),
        }
    }

    pub fn to_vec(&self) -> Vec<BlockId> {
        match self {
            ChunkData::Empty => vec![0; CHUNK_VOLUME],
            ChunkData::Uniform(id) => vec![*id; CHUNK_VOLUME],
            ChunkData::Paletted(data) => data.to_blocks(),
        }
    }

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

        let neighbor_id = if nx >= 0
            && ny >= 0
            && nz >= 0
            && nx < CHUNK_SIZE as i32
            && ny < CHUNK_SIZE as i32
            && nz < CHUNK_SIZE as i32
        {
            self.get(nx as usize, ny as usize, nz as usize)
        } else {
            let neighbor_x = nx.rem_euclid(CHUNK_SIZE as i32) as usize;
            let neighbor_y = ny.rem_euclid(CHUNK_SIZE as i32) as usize;
            let neighbor_z = nz.rem_euclid(CHUNK_SIZE as i32) as usize;
            neighbors.get_neighbor_block(face_index, neighbor_x, neighbor_y, neighbor_z)
        };

        let current_id = self.get(x, y, z);

        // 优化1：相同类型的相邻方块（包括水）之间的面完全剔除
        // 防止水体内部方块渲染冗余面
        if neighbor_id == current_id && neighbor_id != 0 {
            return false;
        }

        // 优化2：实体方块（草地、石头、泥土、沙）完全遮挡相邻面
        if is_block_solid(neighbor_id) {
            return false;
        }

        // 邻居是空气或不同类型的非实体方块时，渲染面
        true
    }

    pub fn memory_usage(&self) -> usize {
        match self {
            ChunkData::Empty => 0,
            ChunkData::Uniform(_) => 2,
            ChunkData::Paletted(data) => data.palette_len() * 2 + CHUNK_VOLUME,
        }
    }
}

impl Default for ChunkData {
    fn default() -> Self {
        Self::Empty
    }
}

pub type Chunk = ChunkData;

/// `Arc<ChunkData>` 的组件包装器。
///
/// 实体组件和 `ChunkEntry.data` 共享同一份 `Arc<ChunkData>`，
/// 避免在创建实体和提交异步任务时发生 ~64KB 的深拷贝。
/// 写入操通过 `Arc::make_mut` 在必要时按需克隆（仅限方块交互路径）。
#[derive(Component, Clone)]
pub struct ChunkComponent(pub Arc<ChunkData>);

impl std::ops::Deref for ChunkComponent {
    type Target = ChunkData;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Generates a face-culled mesh for the chunk.
pub fn generate_chunk_mesh(
    chunk: &Chunk,
    resource_pack: &ResourcePackManager,
    neighbors: &ChunkNeighbors,
) -> (Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<[f32; 3]>, Vec<u32>) {
    if matches!(chunk, ChunkData::Empty | ChunkData::Uniform(0)) {
        return (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    }

    let mut positions = Vec::with_capacity(48000);
    let mut uvs = Vec::with_capacity(48000);
    let mut normals = Vec::with_capacity(48000);
    let mut indices = Vec::with_capacity(72000);

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
                    let uv_idx = FACE_UV_INDICES[face_index];

                    let uv = resource_pack.get_block_uv_by_index(block_id, uv_idx);

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

    let u_min = uv.0;
    let u_max = uv.1;
    let v_min = uv.2;
    let v_max = uv.3;

    let face_uvs = [
        [u_min, v_max],
        [u_max, v_max],
        [u_max, v_min],
        [u_min, v_min],
    ];

    (verts, face_uvs, normal)
}

/// Chunk coordinates in chunk space.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
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

/// Block position in world space.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BlockPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl BlockPos {
    pub fn from_world(world_pos: Vec3) -> Self {
        Self {
            x: world_pos.x.floor() as i32,
            y: world_pos.y.floor() as i32,
            z: world_pos.z.floor() as i32,
        }
    }

    pub fn to_chunk_coord(self) -> ChunkCoord {
        ChunkCoord {
            cx: self.x.div_euclid(CHUNK_SIZE as i32),
            cy: self.y.div_euclid(CHUNK_SIZE as i32),
            cz: self.z.div_euclid(CHUNK_SIZE as i32),
        }
    }

    pub fn to_local(self) -> (usize, usize, usize) {
        (
            self.x.rem_euclid(CHUNK_SIZE as i32) as usize,
            self.y.rem_euclid(CHUNK_SIZE as i32) as usize,
            self.z.rem_euclid(CHUNK_SIZE as i32) as usize,
        )
    }
}

// ============================================================================
// Terrain generation
// ============================================================================

/// Terrain noise seed
const TERRAIN_SEED: u32 = 12345;

/// Base terrain height (world Y coordinate)
const TERRAIN_BASE_HEIGHT: i32 = 96;

/// Terrain height amplitude (max deviation from base)
const TERRAIN_AMPLITUDE: f64 = 80.0;

/// Height threshold for sand vs grass
const SAND_HEIGHT_THRESHOLD: i32 = 22;

/// Depth of dirt layer below surface
const DIRT_LAYER_DEPTH: i32 = 4;

/// Water level (base height for water to appear)
pub const WATER_LEVEL: i32 = 80;

/// Minimum terrain generation height
const TERRAIN_MIN_Y: i32 = -64;
/// Maximum terrain generation height
const TERRAIN_MAX_Y: i32 = 256;

/// 全局噪声缓存（线程安全）
///
/// 使用 std::sync::OnceLock 缓存噪声函数，避免每次调用 fill_terrain 都重新创建。
static TERRAIN_NOISE: std::sync::OnceLock<Fbm<Simplex>> = std::sync::OnceLock::new();

/// 获取缓存的噪声函数
fn get_terrain_noise() -> &'static Fbm<Simplex> {
    TERRAIN_NOISE.get_or_init(|| {
        Fbm::<Simplex>::new(TERRAIN_SEED)
            .set_octaves(5)
            .set_frequency(0.003)
            .set_lacunarity(2.0)
            .set_persistence(0.5)
    })
}

/// 获取世界坐标 (world_x, world_z) 处的**地表高度**。
///
/// 使用与 `fill_terrain` 完全相同的噪声配置和计算公式，
/// 确保在任何位置（跨越区块边界）计算的地表高度一致。
///
/// 此函数是确定性的——相同的 (world_x, world_z) 总是返回相同的高度值。
/// 这使得树木生成可以在不依赖邻近区块数据的情况下正确计算树木位置。
pub fn get_surface_height(world_x: f64, world_z: f64) -> i32 {
    let noise = get_terrain_noise();
    let noise_val = noise.get([world_x, world_z]);
    TERRAIN_BASE_HEIGHT + (noise_val * TERRAIN_AMPLITUDE) as i32
}

/// Fills a chunk with noise-generated terrain.
///
/// 使用 Simplex FBM 噪声在 XZ 平面采样，生成有起伏的自然地形。
/// 地形分层：地表=草地/沙子，浅层=泥土，深层=石头。
///
/// 优化：使用缓存的噪声函数，避免每次创建。
pub fn fill_terrain(chunk: &mut Chunk, coord: &ChunkCoord) {
    let noise = get_terrain_noise();

    for z in 0..CHUNK_SIZE {
        for x in 0..CHUNK_SIZE {
            let world_x = coord.cx as f64 * CHUNK_SIZE as f64 + x as f64;
            let world_z = coord.cz as f64 * CHUNK_SIZE as f64 + z as f64;

            let noise_val = noise.get([world_x, world_z]);
            let surface_height = TERRAIN_BASE_HEIGHT + (noise_val * TERRAIN_AMPLITUDE) as i32;

            for y in 0..CHUNK_SIZE {
                let world_y = coord.cy as i32 * CHUNK_SIZE as i32 + y as i32;

                // Skip if outside terrain generation bounds
                if world_y > TERRAIN_MAX_Y {
                    continue; // above max height = air
                }
                if world_y < TERRAIN_MIN_Y {
                    continue; // below min height = air
                }

                if world_y > surface_height {
                    // 检查是否应该填充水方块
                    if world_y < WATER_LEVEL && surface_height < WATER_LEVEL {
                        chunk.set(x, y, z, 5); // water
                    }
                    continue; // 空气，不需要设置（默认就是 0）
                }

                let block_id = if world_y == surface_height {
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
pub fn spawn_chunk_entity(
    commands: &mut Commands,
    materials: &mut Assets<VoxelMaterial>,
    meshes: &mut Assets<Mesh>,
    chunk: Chunk,
    position: Vec3,
    resource_pack: &ResourcePackManager,
    array_texture: &Handle<Image>,
    neighbors: &ChunkNeighbors,
) -> (Entity, Handle<Mesh>, Handle<VoxelMaterial>) {
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

    let mat_handle = materials.add(VoxelMaterial {
        array_texture: array_texture.clone(),
    });

    let entity = commands
        .spawn((
            chunk.clone(),
            Transform::from_translation(position),
            Visibility::default(),
            Mesh3d(mesh_handle.clone()),
            MeshMaterial3d(mat_handle.clone()),
            ChunkMeshHandle {
                mesh: mesh_handle.clone(),
                material: mat_handle.clone(),
            },
        ))
        .id();

    (entity, mesh_handle, mat_handle)
}
