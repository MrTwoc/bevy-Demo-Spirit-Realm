//! Voxel chunk: block storage + face-culled mesh generation.
//!
//! 使用调色板压缩（PalettedChunkData）优化内存占用：
//! - Empty: 0 字节（全空气）
//! - Uniform: 2 字节（全同一种方块）
//! - Paletted: ~0.5-32KB（调色板压缩 + 位打包，32³=32768体素）

use bevy::prelude::*;
use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Arc;

// 从 terrain_noise re-export 地形常量和函数，保持外部模块兼容
pub use crate::terrain_noise::{
    TERRAIN_BASE_HEIGHT, DIRT_LAYER_DEPTH,
    TERRAIN_MIN_Y, TERRAIN_MAX_Y, SEA_LEVEL,
    compute_surface_height, get_terrain_noise,
};
// 向后兼容别名：tree_gen 等模块仍在使用 WATER_LEVEL
pub const WATER_LEVEL: i32 = SEA_LEVEL;

// 从 types.rs 和 face.rs re-export 核心类型，保持外部模块兼容
pub use crate::types::{CHUNK_SIZE, CHUNK_VOLUME, BlockId, ChunkCoord, BlockPos};
pub use crate::types::{AIR, GRASS, STONE, DIRT, SAND, WATER};
pub use crate::face::{Face, FACES};

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
/// 数据驱动：从全局 `BlockPropertiesTable` 读取（由 `assets/blockstates/*.json` 定义）。
/// 实体方块完全遮挡相邻方向的拼接面，即使方块 ID 不同。
/// 非实体方块（空气、水）不遮挡面，其与实体方块之间的界面应当渲染。
#[inline]
pub fn is_block_solid(block_id: BlockId) -> bool {
    crate::block_definition::is_block_solid_from_table(block_id)
}

/// 核心剔除规则：判断两个方块之间的面是否应该被剔除。
///
/// 所有网格生成模块（同步、异步、LOD）共享此规则，避免逻辑重复。
#[inline]
pub fn should_cull_face(current_id: BlockId, neighbor_id: BlockId) -> bool {
    // 相同类型的非空气方块：内部面完全剔除（水体相邻面不再渲染）
    if neighbor_id == current_id && neighbor_id != AIR {
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
        let palette = vec![AIR];
        let mut reverse_palette = HashMap::new();
        reverse_palette.insert(AIR, 0);

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
            return AIR;
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
        if self.palette.len() == 1 && self.palette[0] == AIR {
            return true;
        }
        if let Some(&air_index) = self.reverse_palette.get(&AIR) {
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

    pub fn get(&self, x: usize, y: usize, z: usize) -> BlockId {
        match self {
            ChunkData::Empty => AIR,
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
                    if *current_id != AIR {
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

// 地形生成函数已移至 terrain_gen.rs，通过 re-export 保持外部模块兼容
pub use crate::terrain_gen::{fill_terrain, fill_flat_terrain, fill_menger_sponge, get_surface_height};

