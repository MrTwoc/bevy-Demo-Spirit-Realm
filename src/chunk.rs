//! Voxel chunk: block storage + face-culled mesh generation.
//!
//! 使用调色板压缩（PalettedChunkData）优化内存占用：
//! - Empty: 0 字节（全空气）
//! - Uniform: 2 字节（全同一种方块）
//! - Paletted: ~0.5-32KB（调色板压缩 + 位打包，32³=32768体素）

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

/// 世界类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum WorldType {
    /// 噪声世界：使用 Simplex 噪声生成起伏地形，含草/泥土/石头/水
    #[default]
    Noise,
    /// 平坦世界：只有草方块(1)和泥土(3)，地表 Y=96
    Flat,
    /// 虚空世界：完全没有地面，全部为虚空（仅空气）。
    /// 用于自由演示各种内容（如实体、粒子、建筑等），不受地形干扰。
    Void,
    /// 门格海绵世界：由 Menger Sponge 分形构成的石头结构，中心位于世界原点。
    /// 使用 4 次递归迭代，实体方块为石头(2)。
    MengerSponge,
}

/// 世界类型资源（ECS Resource），用于在系统中查询当前世界类型
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct WorldTypeResource(pub WorldType);

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

/// 核心剔除规则：判断两个方块之间的面是否应该被剔除。
///
/// 所有网格生成模块（同步、异步、LOD）共享此规则，避免逻辑重复。
#[inline]
pub fn should_cull_face(current_id: BlockId, neighbor_id: BlockId) -> bool {
    // 相同类型的非空气方块：内部面完全剔除（水体相邻面不再渲染）
    if neighbor_id == current_id && neighbor_id != 0 {
        return true;
    }
    // 实体方块完全遮挡相邻面
    if is_block_solid(neighbor_id) {
        return true;
    }
    false
}

// ── P0 位打包辅助函数 ────────────────────────────────────────────

/// 计算调色板大小所需的最低位宽（1/2/4/8 bit）。
#[inline]
const fn bits_for_palette(len: usize) -> u8 {
    if len <= 2 {
        1
    } else if len <= 4 {
        2
    } else if len <= 16 {
        4
    } else {
        8
    }
}

/// 一个 u64 能存多少个指定位宽的索引。
#[inline]
const fn indices_per_u64(bits: u8) -> usize {
    64 / (bits as usize)
}

/// 存储 CHUNK_VOLUME 个指定位宽索引需要的 u64 数量。
#[inline]
const fn total_packed_u64s(bits: u8) -> usize {
    let per = indices_per_u64(bits);
    (CHUNK_VOLUME + per - 1) / per
}

/// 从 packed 存储中读取第 `idx` 个调色板索引。
#[inline]
fn read_packed(packed: &[u64], idx: usize, bits: u8) -> u8 {
    let b = bits as usize;
    let per = 64 / b;
    let word = idx / per;
    let off = (idx % per) * b;
    ((packed[word] >> off) & ((1u64 << b) - 1)) as u8
}

/// 将 `value` 写入 packed 存储的第 `idx` 个位置。
#[inline]
fn write_packed(packed: &mut [u64], idx: usize, bits: u8, value: u8) {
    let b = bits as usize;
    let per = 64 / b;
    let word = idx / per;
    let off = (idx % per) * b;
    let mask = (1u64 << b) - 1;
    let val = (value as u64) & mask;
    packed[word] = (packed[word] & !(mask << off)) | (val << off);
}

/// 将所有索引从 `old_bits` 重新打包为 `new_bits`（位宽扩容时用）。
fn re_pack(packed: &[u64], old_bits: u8, new_bits: u8) -> Vec<u64> {
    let mut new_packed = vec![0u64; total_packed_u64s(new_bits)];
    for i in 0..CHUNK_VOLUME {
        write_packed(
            &mut new_packed,
            i,
            new_bits,
            read_packed(packed, i, old_bits),
        );
    }
    new_packed
}

// ── 调色板压缩区块数据 ──────────────────────────────────────────

/// 调色板压缩 + P0 位打包的区块数据。
#[derive(Clone)]
pub struct PalettedChunkData {
    palette: Vec<BlockId>,
    reverse_palette: HashMap<BlockId, u8>,
    /// 当前每个索引占用的比特数（1/2/4/8）。
    bits_per_index: u8,
    /// 位打包后的索引存储，每个 u64 存放多个索引。
    packed: Vec<u64>,
}

impl PalettedChunkData {
    pub fn new() -> Self {
        let palette = vec![0];
        let mut reverse_palette = HashMap::new();
        reverse_palette.insert(0, 0);

        let bits_per_index = bits_for_palette(palette.len());

        Self {
            palette,
            reverse_palette,
            bits_per_index,
            packed: vec![0u64; total_packed_u64s(bits_per_index)],
        }
    }

    pub fn from_blocks(blocks: &[BlockId]) -> Self {
        let mut palette = Vec::new();
        let mut reverse_palette = HashMap::new();

        for &block_id in blocks {
            if !reverse_palette.contains_key(&block_id) {
                let index = palette.len() as u8;
                palette.push(block_id);
                reverse_palette.insert(block_id, index);
            }
        }

        let bits_per_index = bits_for_palette(palette.len());
        let mut packed = vec![0u64; total_packed_u64s(bits_per_index)];

        for (i, &block_id) in blocks.iter().enumerate() {
            let index = reverse_palette[&block_id];
            write_packed(&mut packed, i, bits_per_index, index);
        }

        Self {
            palette,
            reverse_palette,
            bits_per_index,
            packed,
        }
    }

    pub fn get(&self, x: usize, y: usize, z: usize) -> BlockId {
        if x >= CHUNK_SIZE || y >= CHUNK_SIZE || z >= CHUNK_SIZE {
            return 0;
        }
        let idx = z * CHUNK_SIZE * CHUNK_SIZE + y * CHUNK_SIZE + x;
        let palette_index = read_packed(&self.packed, idx, self.bits_per_index) as usize;
        self.palette[palette_index]
    }

    pub fn add_or_get_palette_index(&mut self, id: BlockId) -> u8 {
        if let Some(&index) = self.reverse_palette.get(&id) {
            return index;
        }
        let new_size = self.palette.len() + 1;
        let new_bits = bits_for_palette(new_size);
        if new_bits > self.bits_per_index {
            self.packed = re_pack(&self.packed, self.bits_per_index, new_bits);
            self.bits_per_index = new_bits;
        }
        let index = self.palette.len() as u8;
        self.palette.push(id);
        self.reverse_palette.insert(id, index);
        index
    }

    pub fn set(&mut self, x: usize, y: usize, z: usize, id: BlockId) {
        if x >= CHUNK_SIZE || y >= CHUNK_SIZE || z >= CHUNK_SIZE {
            return;
        }
        let idx = z * CHUNK_SIZE * CHUNK_SIZE + y * CHUNK_SIZE + x;
        let palette_index = self.add_or_get_palette_index(id);
        write_packed(&mut self.packed, idx, self.bits_per_index, palette_index);
    }

    /// 用指定的调色板索引填充所有位置（从 Uniform 升级时使用）。
    pub fn fill_all(&mut self, palette_index: u8) {
        let bits = self.bits_per_index as usize;
        let per = 64 / bits;
        let mut pattern = 0u64;
        for s in 0..per {
            let shifted = (palette_index as u64) << (s * bits);
            pattern |= shifted;
        }
        self.packed.fill(pattern);
    }

    pub fn to_blocks(&self) -> Vec<BlockId> {
        let mut blocks = Vec::with_capacity(CHUNK_VOLUME);
        for i in 0..CHUNK_VOLUME {
            let idx = read_packed(&self.packed, i, self.bits_per_index) as usize;
            blocks.push(self.palette[idx]);
        }
        blocks
    }

    pub fn palette_len(&self) -> usize {
        self.palette.len()
    }

    pub fn is_empty(&self) -> bool {
        if self.palette.len() == 1 && self.palette[0] == 0 {
            return true;
        }
        if let Some(&air_index) = self.reverse_palette.get(&0) {
            for i in 0..CHUNK_VOLUME {
                if read_packed(&self.packed, i, self.bits_per_index) != air_index {
                    return false;
                }
            }
            true
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
///
/// 存储 `Arc<ChunkData>` 引用而非展开的 `Arc<Vec<BlockId>>`，
/// 避免主线程分配 32KB 容器。工作线程通过 `ChunkData::get()` 按需查询。
pub struct ChunkNeighbors {
    pub neighbor_data: [Option<Arc<ChunkData>>; 6],
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
                data.get(x, y, z)
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
                        data.fill_all(palette_index);
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

    #[deprecated = "展开 32KB ChunkData 为 Vec<BlockId>，纯性能损耗。仅在废弃的 GPU meshing 路径中使用。"]
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

        !should_cull_face(current_id, neighbor_id)
    }

    pub fn memory_usage(&self) -> usize {
        match self {
            ChunkData::Empty => 0,
            ChunkData::Uniform(_) => 2,
            ChunkData::Paletted(data) => data.palette_len() * 2 + data.packed.len() * 8,
        }
    }

    /// 检查区块是否包含指定的方块类型
    pub fn contains_block(&self, block_id: BlockId) -> bool {
        match self {
            ChunkData::Empty => false,
            ChunkData::Uniform(id) => *id == block_id,
            ChunkData::Paletted(data) => {
                // 检查调色板中是否包含该方块 ID
                data.palette.contains(&block_id)
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
pub const TERRAIN_SEED: u32 = 12345;

/// Base terrain height (world Y coordinate)
pub const TERRAIN_BASE_HEIGHT: i32 = 96;

/// Terrain height amplitude (max deviation from base)
pub const TERRAIN_AMPLITUDE: f64 = 180.0;

/// Depth of dirt layer below surface
pub const DIRT_LAYER_DEPTH: i32 = 4;

/// Water level (base height for water to appear)
pub const WATER_LEVEL: i32 = 80;

/// Minimum terrain generation height
pub const TERRAIN_MIN_Y: i32 = -256;
/// Maximum terrain generation height
pub const TERRAIN_MAX_Y: i32 = 256;

/// 细节层噪声振幅（小丘陵/凹陷的高度变化，±20 blocks）
pub const TERRAIN_DETAIL_AMP: f64 = 20.0;

/// 全局噪声缓存（线程安全）
///
/// 使用 std::sync::OnceLock 缓存噪声函数，避免每次调用 fill_terrain 都重新创建。
static TERRAIN_NOISE: std::sync::OnceLock<Fbm<Simplex>> = std::sync::OnceLock::new();

// ── powf 查表优化 ─────────────────────────────────────────────────
//
// compute_surface_height 中的 2.0_f64.powf(x) 是昂贵的 f64 指数运算（~20-30 周期）。
// 由于输入 x ∈ [-1, 1]（FBM 噪声输出范围），可预计算 2^x 的查找表，
// 使用线性插值替代运行时 powf 调用。
// 257 项精度误差 < 0.0004，对地形高度（整数）完全无影响。

/// 2^x 查找表，x ∈ [-1, 1]，257 项（含两端），线性插值精度 < 0.0004
static POWF_TABLE: [f64; 257] = {
    let mut table = [0.0f64; 257];
    let mut i = 0;
    while i < 257 {
        let x = -1.0 + (i as f64) * (2.0 / 256.0);
        // 手动泰勒展开计算 2^x = e^(x*ln2)
        // 使用 Horner 形式的 12 阶泰勒多项式，精度 ~1e-15
        let xln2 = x * 0.6931471805599453;
        // e^t 的 12 阶泰勒展开：1 + t + t²/2! + ... + t¹²/12!
        let t = xln2;
        let t2 = t * t;
        let t3 = t2 * t;
        let t4 = t3 * t;
        let t5 = t4 * t;
        let t6 = t5 * t;
        let t7 = t6 * t;
        let t8 = t7 * t;
        let t9 = t8 * t;
        let t10 = t9 * t;
        let t11 = t10 * t;
        let t12 = t11 * t;
        table[i] = 1.0
            + t
            + t2 * 0.5
            + t3 * (1.0 / 6.0)
            + t4 * (1.0 / 24.0)
            + t5 * (1.0 / 120.0)
            + t6 * (1.0 / 720.0)
            + t7 * (1.0 / 5040.0)
            + t8 * (1.0 / 40320.0)
            + t9 * (1.0 / 362880.0)
            + t10 * (1.0 / 3628800.0)
            + t11 * (1.0 / 39916800.0)
            + t12 * (1.0 / 479001600.0);
        i += 1;
    }
    table
};

/// 快速 2^x 计算（查表 + 线性插值），替代 f64::powf。
///
/// 输入范围：x ∈ [-1, 1]（FBM 噪声输出范围）
/// 输出范围：[0.5, 2.0]
/// 精度误差：< 0.0004（对整数地形高度完全无影响）
/// 性能：~2-3 周期 vs powf 的 ~20-30 周期
#[inline]
fn fast_pow2(x: f64) -> f64 {
    // 将 [-1, 1] 映射到 [0, 256]
    let t = (x + 1.0) * 128.0;
    let i = t as usize;
    let f = t - i as f64;
    // 边界保护（x 可能因浮点精度略超出 [-1, 1]）
    if i >= 256 {
        return POWF_TABLE[256];
    }
    // 线性插值
    POWF_TABLE[i] * (1.0 - f) + POWF_TABLE[i + 1] * f
}

/// 获取缓存的粗轮廓噪声函数（低频 FBM，大山脉/谷地）
pub fn get_terrain_noise() -> &'static Fbm<Simplex> {
    TERRAIN_NOISE.get_or_init(|| {
        Fbm::<Simplex>::new(TERRAIN_SEED)
            .set_octaves(5)
            .set_frequency(0.003)
            .set_lacunarity(2.0)
            .set_persistence(0.5)
    })
}

/// 细节层噪声缓存（高频，小丘陵/凹陷）
static TERRAIN_DETAIL_NOISE: std::sync::OnceLock<Fbm<Simplex>> = std::sync::OnceLock::new();

/// 获取缓存的细节噪声函数（高频 3 octaves，频率 ~7x 于粗轮廓）
pub fn get_terrain_detail_noise() -> &'static Fbm<Simplex> {
    TERRAIN_DETAIL_NOISE.get_or_init(|| {
        Fbm::<Simplex>::new(TERRAIN_SEED.wrapping_add(1))
            .set_octaves(3)
            .set_frequency(0.02)
            .set_lacunarity(2.0)
            .set_persistence(0.5)
    })
}

/// 计算噪声地形的地表高度（核心公式，被 fill_terrain、get_surface_height、terrain_bridge 共享）。
///
/// 公式：`base + (2^coarse - 1) * amplitude + detail * detail_amp`
///
/// - **粗轮廓** (`2^coarse - 1`)：将 [-1,1] 噪声映射为 [-0.5, 1.0] 的非线性乘数，
///   产生大面积平坦低地 + 少量高耸山峰的效果。
/// - **细节层**：叠加高频小振幅噪声，为平原和山坡增添微起伏。
///
/// # 性能优化
///
/// 使用 `fast_pow2` 查表 + 线性插值替代 `f64::powf`（~2-3 周期 vs ~20-30 周期）。
#[inline]
pub fn compute_surface_height(world_x: f64, world_z: f64) -> i32 {
    let coarse_val = get_terrain_noise().get([world_x, world_z]);       // [-1, 1]
    let coarse_mult = fast_pow2(coarse_val) - 1.0;                      // [-0.5, 1.0]
    let detail_val = get_terrain_detail_noise().get([world_x, world_z]); // [-1, 1]
    TERRAIN_BASE_HEIGHT
        + (coarse_mult * TERRAIN_AMPLITUDE + detail_val * TERRAIN_DETAIL_AMP) as i32
}

/// 获取世界坐标 (world_x, world_z) 处的**地表高度**。
///
/// 使用与 `fill_terrain` 完全相同的噪声配置和计算公式，
/// 确保在任何位置（跨越区块边界）计算的地表高度一致。
///
/// 此函数是确定性的——相同的 (world_x, world_z) 总是返回相同的高度值。
/// 这使得树木生成可以在不依赖邻近区块数据的情况下正确计算树木位置。
pub fn get_surface_height(world_x: f64, world_z: f64, world_type: WorldType) -> i32 {
    match world_type {
        WorldType::Flat => TERRAIN_BASE_HEIGHT,
        WorldType::Void => i32::MIN,
        WorldType::Noise => compute_surface_height(world_x, world_z),
        WorldType::MengerSponge => i32::MIN,
    }
}

/// Fills a chunk with noise-generated terrain.
///
/// 使用指数高度映射（`2^coarse - 1`）+ 高频细节层生成地形。
/// 公式：`base + (2^coarse - 1) * amplitude + detail * detail_amp`
/// 地形分层：地表=草地(1)，浅层=泥土(3)，深层=石头(2)，土壤厚度=4。
pub fn fill_terrain(chunk: &mut Chunk, coord: &ChunkCoord) {
    for z in 0..CHUNK_SIZE {
        for x in 0..CHUNK_SIZE {
            let world_x = coord.cx as f64 * CHUNK_SIZE as f64 + x as f64;
            let world_z = coord.cz as f64 * CHUNK_SIZE as f64 + z as f64;

            let surface_height = compute_surface_height(world_x, world_z);

            let surface_block: BlockId = 1; // grass
            let under_surface_block: BlockId = 3; // dirt
            let soil_thickness = 4;

            for y in 0..CHUNK_SIZE {
                let world_y = coord.cy as i32 * CHUNK_SIZE as i32 + y as i32;

                if world_y > TERRAIN_MAX_Y {
                    continue;
                }
                if world_y < TERRAIN_MIN_Y {
                    continue;
                }

                if world_y > surface_height {
                    if world_y < WATER_LEVEL && surface_height < WATER_LEVEL {
                        chunk.set(x, y, z, 5); // water
                    }
                    continue;
                }

                let block_id = if world_y == surface_height {
                    surface_block
                } else if world_y > surface_height - soil_thickness {
                    under_surface_block
                } else {
                    2 // stone
                };

                chunk.set(x, y, z, block_id);
            }
        }
    }
}

/// 填充平台世界地形（超平坦，只有草方块和泥土）
///
/// 表面 Y=96（`TERRAIN_BASE_HEIGHT`）为草方块(grass=1)，
/// 下方 `DIRT_LAYER_DEPTH` 层为泥土(dirt=3)，
/// 不生成石头和水（与噪声世界截然不同）。
pub fn fill_flat_terrain(chunk: &mut Chunk, coord: &ChunkCoord) {
    for z in 0..CHUNK_SIZE {
        for x in 0..CHUNK_SIZE {
            for y in 0..CHUNK_SIZE {
                let world_y = coord.cy as i32 * CHUNK_SIZE as i32 + y as i32;

                if world_y == TERRAIN_BASE_HEIGHT {
                    chunk.set(x, y, z, 1); // grass
                } else if world_y > TERRAIN_BASE_HEIGHT - DIRT_LAYER_DEPTH
                    && world_y < TERRAIN_BASE_HEIGHT
                {
                    chunk.set(x, y, z, 3); // dirt
                }
                // else: air（默认就是 0）
            }
        }
    }
}

/// 使用 Menger Sponge（门格海绵）分形算法填充区块。
///
/// 门格海绵规则：对于体素的世界坐标 (wx, wy, wz)，在每个递归层级上，
/// 将坐标除以 3 并检查余数：如果至少有 2 个坐标的余数为 1，则该体素为空洞；
/// 否则保持为实体。
///
/// 使用 4 次递归迭代，实体方块为石头(2)。海绵中心位于世界原点。
pub fn fill_menger_sponge(chunk: &mut Chunk, coord: &ChunkCoord) {
    const SPONGE_ITERATIONS: u32 = 4;
    const AIR: u8 = 0;
    const STONE: u8 = 2;

    for z in 0..CHUNK_SIZE {
        for y in 0..CHUNK_SIZE {
            for x in 0..CHUNK_SIZE {
                let wx = coord.cx as i32 * CHUNK_SIZE as i32 + x as i32;
                let wy = coord.cy as i32 * CHUNK_SIZE as i32 + y as i32;
                let wz = coord.cz as i32 * CHUNK_SIZE as i32 + z as i32;

                // 使用正坐标计算分形（取绝对值，保持对称性）
                let mut cx = wx.unsigned_abs();
                let mut cy = wy.unsigned_abs();
                let mut cz = wz.unsigned_abs();

                let mut solid = true;
                for _ in 0..SPONGE_ITERATIONS {
                    let rx = cx % 3;
                    let ry = cy % 3;
                    let rz = cz % 3;

                    // 如果至少有 2 个坐标在当前层级的数字为 1，则此处为空洞
                    let ones = (rx == 1) as u8 + (ry == 1) as u8 + (rz == 1) as u8;
                    if ones >= 2 {
                        solid = false;
                        break;
                    }

                    cx /= 3;
                    cy /= 3;
                    cz /= 3;
                }

                chunk.set(x, y, z, if solid { STONE } else { AIR });
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
        alpha_mode: if chunk.contains_block(5) {
            AlphaMode::Blend
        } else {
            AlphaMode::Opaque
        },
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
