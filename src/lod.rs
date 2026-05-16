//! LOD (Level of Detail) 系统 - Phase 1 核心模块
//!
//! 实现四级 LOD 降采样，将渲染距离从 8 区块扩展到 32 区块。
//!
//! # LOD 级别定义
//!
//! | LOD 级别 | 降采样率 | 采样步长 | 体素数/区块 | 渲染距离 |
//! |----------|---------|---------|------------|---------|
//! | LOD0     | 1:1     | 1 体素  | 32³ = 32,768 | 0-8 区块 |
//! | LOD1     | 1:2     | 2 体素  | 16³ = 4,096  | 9-16 区块 |
//! | LOD2     | 1:4     | 4 体素  | 8³ = 512     | 17-24 区块 |
//! | LOD3     | 1:8     | 8 体素  | 4³ = 64      | 25-32 区块 |

use std::collections::HashMap;

use crate::async_mesh::UvLookupTable;
use crate::chunk::{BlockId, ChunkCoord, ChunkData, ChunkNeighbors, is_block_solid, CHUNK_SIZE};
use bevy::prelude::Resource;

// ============================================================================
// LOD 级别定义
// ============================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LodLevel {
    Lod0 = 0,
    Lod1 = 1,
    Lod2 = 2,
    Lod3 = 3,
}

impl LodLevel {
    pub const MAX: usize = 3;

    #[inline]
    pub const fn step(self) -> usize {
        match self {
            LodLevel::Lod0 => 1,
            LodLevel::Lod1 => 2,
            LodLevel::Lod2 => 4,
            LodLevel::Lod3 => 8,
        }
    }

    #[inline]
    pub const fn sampling_size(self) -> usize {
        match self {
            LodLevel::Lod0 => CHUNK_SIZE,
            LodLevel::Lod1 => 16,
            LodLevel::Lod2 => 8,
            LodLevel::Lod3 => 4,
        }
    }

    /// 根据与玩家的距离（区块为单位）计算 LOD 级别
    pub fn from_chunk_distance(dist_chunks: f32) -> Self {
        match dist_chunks {
            d if d < 9.0 => LodLevel::Lod0,
            d if d < 17.0 => LodLevel::Lod1,
            d if d < 25.0 => LodLevel::Lod2,
            _ => LodLevel::Lod3,
        }
    }

    /// 根据距离平方计算 LOD 级别（避免 sqrt）
    ///
    /// 阈值平方：9²=81, 17²=289, 25²=625
    #[inline]
    pub fn from_chunk_distance_sq(dist_sq: i32) -> Self {
        match dist_sq {
            d if d < 81 => LodLevel::Lod0,    // < 9²
            d if d < 289 => LodLevel::Lod1,   // < 17²
            d if d < 625 => LodLevel::Lod2,   // < 25²
            _ => LodLevel::Lod3,
        }
    }

    #[inline]
    fn threshold(self) -> f32 {
        match self {
            LodLevel::Lod0 => 8.0,
            LodLevel::Lod1 => 16.0,
            LodLevel::Lod2 => 24.0,
            LodLevel::Lod3 => 32.0,
        }
    }

    #[inline]
    #[allow(dead_code)]
    fn min_threshold(self) -> f32 {
        match self {
            LodLevel::Lod0 => 0.0,
            LodLevel::Lod1 => 9.0,
            LodLevel::Lod2 => 17.0,
            LodLevel::Lod3 => 25.0,
        }
    }
}

// ============================================================================
// LOD 管理器
// ============================================================================

#[derive(Resource)]
pub struct LodManager {
    chunk_lods: HashMap<ChunkCoord, LodLevel>,
    hysteresis: f32,
}

impl LodManager {
    pub fn new() -> Self {
        Self {
            chunk_lods: HashMap::new(),
            hysteresis: 0.5,
        }
    }

    pub fn update(
        &mut self,
        player_chunk: ChunkCoord,
        loaded: &super::chunk_manager::LoadedChunks,
    ) -> Vec<(ChunkCoord, LodLevel)> {
        let mut to_rebuild = Vec::new();

        for (coord, _) in &loaded.entries {
            let dist = self.chunk_distance(*coord, player_chunk);
            let new_lod = LodLevel::from_chunk_distance(dist);

            let current_lod = self
                .chunk_lods
                .get(coord)
                .copied()
                .unwrap_or(LodLevel::Lod0);

            if new_lod != current_lod {
                if self.should_switch(current_lod, new_lod, dist) {
                    self.chunk_lods.insert(*coord, new_lod);
                    to_rebuild.push((*coord, new_lod));
                }
            }
        }

        to_rebuild
    }

    pub fn get_lod(&self, coord: &ChunkCoord) -> LodLevel {
        self.chunk_lods
            .get(coord)
            .copied()
            .unwrap_or(LodLevel::Lod0)
    }

    pub fn set_lod(&mut self, coord: ChunkCoord, lod: LodLevel) {
        self.chunk_lods.insert(coord, lod);
    }

    pub fn remove(&mut self, coord: &ChunkCoord) {
        self.chunk_lods.remove(coord);
    }

    fn chunk_distance(&self, a: ChunkCoord, b: ChunkCoord) -> f32 {
        let dx = (a.cx - b.cx) as f32;
        let dy = (a.cy - b.cy) as f32;
        let dz = (a.cz - b.cz) as f32;
        (dx * dx + dy * dy + dz * dz).sqrt()
    }

    fn should_switch(&self, current: LodLevel, new: LodLevel, dist: f32) -> bool {
        if (new as i32) < (current as i32) {
            true
        } else {
            let ring_width = 8.0;
            dist > current.threshold() + self.hysteresis * ring_width
        }
    }
}

impl Default for LodManager {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// LOD 降采样网格生成
// ============================================================================

const FACES_LOD: [(FaceLod, [i32; 3], usize); 6] = [
    (FaceLod::Right, [1, 0, 0], 2),
    (FaceLod::Left, [-1, 0, 0], 2),
    (FaceLod::Top, [0, 1, 0], 0),
    (FaceLod::Bottom, [0, -1, 0], 1),
    (FaceLod::Front, [0, 0, 1], 2),
    (FaceLod::Back, [0, 0, -1], 2),
];

#[derive(Clone, Copy)]
enum FaceLod {
    Top,
    Bottom,
    Right,
    Left,
    Front,
    Back,
}

pub fn generate_lod_mesh(
    chunk: &ChunkData,
    uv_table: &UvLookupTable,
    neighbors: &ChunkNeighbors,
    lod: LodLevel,
) -> (Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<[f32; 3]>, Vec<u32>) {
    if matches!(lod, LodLevel::Lod0) {
        return (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    }

    let step = lod.step();
    let step_f = step as f32;
    let sample_size = lod.sampling_size();

    if matches!(chunk, ChunkData::Empty | ChunkData::Uniform(0)) {
        return (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    }

    let capacity = match lod {
        LodLevel::Lod1 => 1200,
        LodLevel::Lod2 => 150,
        LodLevel::Lod3 => 20,
        LodLevel::Lod0 => 48000,
    };

    let mut positions = Vec::with_capacity(capacity);
    let mut uvs = Vec::with_capacity(capacity);
    let mut normals = Vec::with_capacity(capacity);
    let mut indices = Vec::with_capacity(capacity * 2);

    for sz in 0..sample_size {
        for sy in 0..sample_size {
            for sx in 0..sample_size {
                let x = sx * step;
                let y = sy * step;
                let z = sz * step;

                let block_id = sample_dominant_block(chunk, x, y, z, step);
                if block_id == 0 {
                    continue;
                }

                for (face_index, (face, offset, uv_idx)) in FACES_LOD.iter().cloned().enumerate() {
                    let lod_offset = [
                        offset[0] * step as i32,
                        offset[1] * step as i32,
                        offset[2] * step as i32,
                    ];

                    if !is_face_visible_lod(
                        chunk,
                        x,
                        y,
                        z,
                        block_id,
                        &lod_offset,
                        face_index,
                        neighbors,
                        step,
                    ) {
                        continue;
                    }

                    let base_index = positions.len() as u32;

                    let uv = uv_table.get_uv(block_id, uv_idx);

                    let (face_verts, face_uvs, face_normal) =
                        face_quad_lod(x, y, z, face, uv, step_f);

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

fn sample_dominant_block(
    chunk: &ChunkData,
    base_x: usize,
    base_y: usize,
    base_z: usize,
    step: usize,
) -> BlockId {
    for dy in (0..step).rev() {
        for dz in 0..step {
            for dx in 0..step {
                let id = chunk.get(base_x + dx, base_y + dy, base_z + dz);
                if id != 0 {
                    return id;
                }
            }
        }
    }
    0
}

fn is_face_visible_lod(
    chunk: &ChunkData,
    x: usize,
    y: usize,
    z: usize,
    _current_id: BlockId,
    lod_offset: &[i32; 3],
    face_index: usize,
    neighbors: &ChunkNeighbors,
    step: usize,
) -> bool {
    let nx = x as i32 + lod_offset[0];
    let ny = y as i32 + lod_offset[1];
    let nz = z as i32 + lod_offset[2];

    let neighbor_id = if nx >= 0
        && ny >= 0
        && nz >= 0
        && nx + step as i32 <= CHUNK_SIZE as i32
        && ny + step as i32 <= CHUNK_SIZE as i32
        && nz + step as i32 <= CHUNK_SIZE as i32
    {
        sample_dominant_block(chunk, nx as usize, ny as usize, nz as usize, step)
    } else {
        let neighbor_x = nx.rem_euclid(CHUNK_SIZE as i32) as usize;
        let neighbor_y = ny.rem_euclid(CHUNK_SIZE as i32) as usize;
        let neighbor_z = nz.rem_euclid(CHUNK_SIZE as i32) as usize;

        if neighbor_x + step <= CHUNK_SIZE
            && neighbor_y + step <= CHUNK_SIZE
            && neighbor_z + step <= CHUNK_SIZE
        {
            if let Some(sampled) = sample_dominant_block_from_neighbors(
                neighbors,
                face_index,
                neighbor_x,
                neighbor_y,
                neighbor_z,
                step,
            ) {
                sampled
            } else {
                neighbors.get_neighbor_block(face_index, neighbor_x, neighbor_y, neighbor_z)
            }
        } else {
            neighbors.get_neighbor_block(face_index, neighbor_x, neighbor_y, neighbor_z)
        }
    };

    // 核心优化：仅当相邻降采样区域为非实体时才渲染面。
    !is_block_solid(neighbor_id)
}

fn sample_dominant_block_from_neighbors(
    neighbors: &ChunkNeighbors,
    face_index: usize,
    base_x: usize,
    base_y: usize,
    base_z: usize,
    step: usize,
) -> Option<BlockId> {
    if let Some(ref data) = neighbors.neighbor_data[face_index] {
        for dy in (0..step).rev() {
            for dz in 0..step {
                for dx in 0..step {
                    let x = base_x + dx;
                    let y = base_y + dy;
                    let z = base_z + dz;
                    if x < CHUNK_SIZE && y < CHUNK_SIZE && z < CHUNK_SIZE {
                        let idx = z * CHUNK_SIZE * CHUNK_SIZE + y * CHUNK_SIZE + x;
                        let id = data[idx];
                        if id != 0 {
                            return Some(id);
                        }
                    }
                }
            }
        }
    }
    None
}

fn face_quad_lod(
    x: usize,
    y: usize,
    z: usize,
    face: FaceLod,
    uv: (f32, f32, f32, f32),
    step_f: f32,
) -> ([[f32; 3]; 4], [[f32; 2]; 4], [f32; 3]) {
    let x_f = x as f32;
    let y_f = y as f32;
    let z_f = z as f32;

    let (verts, normal) = match face {
        FaceLod::Top => (
            [
                [x_f, y_f + step_f, z_f],
                [x_f + step_f, y_f + step_f, z_f],
                [x_f + step_f, y_f + step_f, z_f + step_f],
                [x_f, y_f + step_f, z_f + step_f],
            ],
            [0.0, 1.0, 0.0],
        ),
        FaceLod::Bottom => (
            [
                [x_f, y_f, z_f + step_f],
                [x_f + step_f, y_f, z_f + step_f],
                [x_f + step_f, y_f, z_f],
                [x_f, y_f, z_f],
            ],
            [0.0, -1.0, 0.0],
        ),
        FaceLod::Right => (
            [
                [x_f + step_f, y_f, z_f],
                [x_f + step_f, y_f, z_f + step_f],
                [x_f + step_f, y_f + step_f, z_f + step_f],
                [x_f + step_f, y_f + step_f, z_f],
            ],
            [1.0, 0.0, 0.0],
        ),
        FaceLod::Left => (
            [
                [x_f, y_f, z_f + step_f],
                [x_f, y_f, z_f],
                [x_f, y_f + step_f, z_f],
                [x_f, y_f + step_f, z_f + step_f],
            ],
            [-1.0, 0.0, 0.0],
        ),
        FaceLod::Front => (
            [
                [x_f + step_f, y_f, z_f + step_f],
                [x_f, y_f, z_f + step_f],
                [x_f, y_f + step_f, z_f + step_f],
                [x_f + step_f, y_f + step_f, z_f + step_f],
            ],
            [0.0, 0.0, 1.0],
        ),
        FaceLod::Back => (
            [
                [x_f, y_f, z_f],
                [x_f + step_f, y_f, z_f],
                [x_f + step_f, y_f + step_f, z_f],
                [x_f, y_f + step_f, z_f],
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
