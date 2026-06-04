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

use crate::async_mesh::{SubMeshData, UvLookupTable, MeshVertex};
use crate::chunk::{BlockId, CHUNK_SIZE, ChunkCoord, ChunkData, ChunkNeighbors, should_cull_face};
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
            d if d < 81 => LodLevel::Lod0,  // < 9²
            d if d < 289 => LodLevel::Lod1, // < 17²
            d if d < 625 => LodLevel::Lod2, // < 25²
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
    /// 上次 LOD 全量检查时的玩家区块坐标
    last_player_chunk: Option<ChunkCoord>,
}

impl LodManager {
    pub fn new() -> Self {
        Self {
            chunk_lods: HashMap::new(),
            hysteresis: 0.5,
            last_player_chunk: None,
        }
    }

    pub fn update(
        &mut self,
        player_chunk: ChunkCoord,
        loaded: &super::chunk_manager::LoadedChunks,
    ) -> Vec<(ChunkCoord, LodLevel)> {
        let mut to_rebuild = Vec::new();

        for (coord, _) in &loaded.entries {
            let dist_sq = Self::chunk_distance_sq(*coord, player_chunk);
            let new_lod = LodLevel::from_chunk_distance_sq(dist_sq);

            let current_lod = self
                .chunk_lods
                .get(coord)
                .copied()
                .unwrap_or(LodLevel::Lod0);

            if new_lod != current_lod {
                if self.should_switch_sq(current_lod, new_lod, dist_sq) {
                    self.chunk_lods.insert(*coord, new_lod);
                    to_rebuild.push((*coord, new_lod));
                }
            }
        }

        to_rebuild
    }

    /// 仅在玩家跨越区块边界时触发全量 LOD 检查。
    ///
    /// 旧版 `update_incremental` 每帧只检查 200 个区块，对 3000+ 区块需要 15 帧
    /// 才能完整遍历一次，移动速度快时 LOD 切换明显滞后。
    ///
    /// 新版仅在 `player_chunk` 变化时执行一次全量遍历（`update`）。
    /// LOD 检查本身只做整数距离平方比较（O(1)/区块），3000 区块全量遍历 < 0.5ms。
    pub fn update_incremental(
        &mut self,
        player_chunk: ChunkCoord,
        loaded: &super::chunk_manager::LoadedChunks,
    ) -> Vec<(ChunkCoord, LodLevel)> {
        // 仅在玩家跨越区块边界时触发全量检查
        if self.last_player_chunk == Some(player_chunk) {
            return Vec::new();
        }
        self.last_player_chunk = Some(player_chunk);

        // 全量遍历：距离平方比较是纯整数运算，3000 区块 < 0.5ms
        self.update(player_chunk, loaded)
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

    /// 区块间距离的平方（整数运算，避免 sqrt）。
    #[inline]
    fn chunk_distance_sq(a: ChunkCoord, b: ChunkCoord) -> i32 {
        let dx = a.cx - b.cx;
        let dy = a.cy - b.cy;
        let dz = a.cz - b.cz;
        dx * dx + dy * dy + dz * dz
    }

    /// 基于平方距离的迟滞判断。
    ///
    /// 阈值预计算为平方值，避免运行时 sqrt。
    #[inline]
    fn should_switch_sq(&self, current: LodLevel, new: LodLevel, dist_sq: i32) -> bool {
        if (new as i32) < (current as i32) {
            true
        } else {
            let threshold = current.threshold() + self.hysteresis * 8.0;
            let threshold_sq = (threshold * threshold) as i32;
            dist_sq > threshold_sq
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

/// 预计算的降采样主导方块网格。
///
/// 将 `sample_dominant_block` 的 O(step³) 开销集中到一次遍历中完成，
/// 后续面可见性检查改为 O(1) 数组索引。
struct LodDominantGrid {
    /// sample_size³ 大小的降采样体素数组，存储每个采样位置的主导方块 ID。
    grid: Vec<BlockId>,
    /// 降采样后的网格尺寸（sample_size）。
    size: usize,
    /// 降采样步长（step）。
    step: usize,
}

impl LodDominantGrid {
    /// 从原始区块数据预计算所有降采样采样点的 BlockId。
    fn from_chunk(chunk: &ChunkData, step: usize, sample_size: usize) -> Self {
        let volume = sample_size * sample_size * sample_size;
        let mut grid = vec![0u8; volume];

        for sz in 0..sample_size {
            for sy in 0..sample_size {
                for sx in 0..sample_size {
                    let idx = sz * sample_size * sample_size + sy * sample_size + sx;
                    grid[idx] = sample_dominant_block(
                        chunk,
                        sx * step,
                        sy * step,
                        sz * step,
                        step,
                    );
                }
            }
        }

        Self {
            grid,
            size: sample_size,
            step,
        }
    }

    /// O(1) 查询采样坐标 (sx, sy, sz) 处的主导方块。
    #[inline]
    fn get(&self, sx: usize, sy: usize, sz: usize) -> BlockId {
        self.grid[sz * self.size * self.size + sy * self.size + sx]
    }

    /// 从体素坐标 (x, y, z) 查询对应采样位置的主导方块（坐标必须是 step 的整数倍）。
    #[inline]
    fn get_from_voxel(&self, x: usize, y: usize, z: usize) -> BlockId {
        let sx = x / self.step;
        let sy = y / self.step;
        let sz = z / self.step;
        self.get(sx, sy, sz)
    }
}

/// LOD 级别的分离 Mesh 生成。
///
/// 对于 LOD1+，降采样后水的 Greedy Mesh 优化效果不明显，
/// 因此统一使用标准算法生成固体 Mesh，水方块同样参与降采样。
///
/// # 性能优化
///
/// 预计算 `LodDominantGrid` 降采样体素数组，将面剔除中的
/// O(step³) `sample_dominant_block` 调用替换为 O(1) 数组索引。
pub fn generate_lod_mesh_separated(
    chunk: &ChunkData,
    uv_table: &UvLookupTable,
    neighbors: &ChunkNeighbors,
    lod: LodLevel,
) -> (SubMeshData, Option<SubMeshData>) {
    if matches!(lod, LodLevel::Lod0) {
        return (SubMeshData::new(), None);
    }

    let step = lod.step();
    let step_f = step as f32;
    let sample_size = lod.sampling_size();

    if matches!(chunk, ChunkData::Empty | ChunkData::Uniform(0)) {
        return (SubMeshData::new(), None);
    }

    // ── Phase 1: 预计算降采样体素网格（O(sample_size³ × step³) 只执行一次） ─
    let dominant_grid = LodDominantGrid::from_chunk(chunk, step, sample_size);

    let capacity = match lod {
        LodLevel::Lod1 => 1200,
        LodLevel::Lod2 => 150,
        LodLevel::Lod3 => 20,
        LodLevel::Lod0 => 48000,
    };

    let mut vertices = Vec::with_capacity(capacity * 4); // 4 vertices per face
    let mut indices = Vec::with_capacity(capacity * 6);   // 6 indices per face

    // ── Phase 2: 面剔除与网格生成 ──────────────────────────────────
    for sz in 0..sample_size {
        for sy in 0..sample_size {
            for sx in 0..sample_size {
                let block_id = dominant_grid.get(sx, sy, sz);

                // LOD 级别只跳过空气方块，水方块同样参与降采样
                if block_id == 0 {
                    continue;
                }

                let x = sx * step;
                let y = sy * step;
                let z = sz * step;

                for (face_index, (face, offset, uv_idx)) in FACES_LOD.iter().cloned().enumerate() {
                    if !is_face_visible_lod_fast(
                        x, y, z,
                        block_id,
                        offset,
                        face_index,
                        neighbors,
                        step,
                        &dominant_grid,
                    ) {
                        continue;
                    }

                    let base_index = vertices.len() as u32;

                    let uv = uv_table.get_uv(block_id, uv_idx);

                    let (face_verts, face_uvs, face_normal) =
                        face_quad_lod(x, y, z, face, uv, step_f);

                    // AoS 布局：4 个顶点连续 push，单次写入 32 字节
                    vertices.push(MeshVertex { position: face_verts[0], normal: face_normal, uv: face_uvs[0] });
                    vertices.push(MeshVertex { position: face_verts[1], normal: face_normal, uv: face_uvs[1] });
                    vertices.push(MeshVertex { position: face_verts[2], normal: face_normal, uv: face_uvs[2] });
                    vertices.push(MeshVertex { position: face_verts[3], normal: face_normal, uv: face_uvs[3] });
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

    let solid = SubMeshData {
        triangle_count: indices.len() as u32 / 3,
        vertices,
        indices,
    };

    // LOD 级别水方块被合并到固体 Mesh 中（简化处理）
    (solid, None)
}

/// 从原始体素区域中找到第一个非空气方块（优先从顶部向下搜索）。
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

/// 面可见性检查（优化版）：使用预计算的 `LodDominantGrid` 进行 O(1) 查询。
///
/// 对比旧版 `is_face_visible_lod`：
/// - 不再对当前区块内的邻居调用 `sample_dominant_block`（O(step³)）
/// - 不再重复计算 `sample_dominant_block(chunk, x, y, z, step)`（与调用方 `block_id` 相同）
/// - 仅当邻居跨区块边界时才回退到邻居数据查询
fn is_face_visible_lod_fast(
    x: usize,
    y: usize,
    z: usize,
    current_id: BlockId,
    face_offset: [i32; 3],
    face_index: usize,
    neighbors: &ChunkNeighbors,
    step: usize,
    grid: &LodDominantGrid,
) -> bool {
    let nx = x as i32 + face_offset[0] * step as i32;
    let ny = y as i32 + face_offset[1] * step as i32;
    let nz = z as i32 + face_offset[2] * step as i32;

    let neighbor_id = if nx >= 0
        && ny >= 0
        && nz >= 0
        && (nx as usize) + step <= CHUNK_SIZE
        && (ny as usize) + step <= CHUNK_SIZE
        && (nz as usize) + step <= CHUNK_SIZE
    {
        // 邻居在同一区块内 → O(1) 网格查询
        grid.get_from_voxel(nx as usize, ny as usize, nz as usize)
    } else {
        // 邻居跨区块边界 → 回退到邻居数据查询
        let neighbor_x = nx.rem_euclid(CHUNK_SIZE as i32) as usize;
        let neighbor_y = ny.rem_euclid(CHUNK_SIZE as i32) as usize;
        let neighbor_z = nz.rem_euclid(CHUNK_SIZE as i32) as usize;

        if neighbor_x + step <= CHUNK_SIZE
            && neighbor_y + step <= CHUNK_SIZE
            && neighbor_z + step <= CHUNK_SIZE
        {
            if let Some(sampled) = sample_dominant_block_from_neighbors(
                neighbors, face_index, neighbor_x, neighbor_y, neighbor_z, step,
            ) {
                sampled
            } else {
                neighbors.get_neighbor_block(face_index, neighbor_x, neighbor_y, neighbor_z)
            }
        } else {
            neighbors.get_neighbor_block(face_index, neighbor_x, neighbor_y, neighbor_z)
        }
    };

    !should_cull_face(current_id, neighbor_id)
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
                        let id = data.get(x, y, z);
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

/// LOD 面四边形生成（顶点归一化版）。
///
/// 顶点坐标除以 `step_f`，使模型空间尺寸与 LOD0 一致（均为 1x1）。
/// 世界空间放大由 `Transform::scale` 通过 GPU 矩阵完成。
fn face_quad_lod(
    x: usize,
    y: usize,
    z: usize,
    face: FaceLod,
    uv: (f32, f32, f32, f32),
    step_f: f32,
) -> ([[f32; 3]; 4], [[f32; 2]; 4], [f32; 3]) {
    let x_f = x as f32 / step_f;
    let y_f = y as f32 / step_f;
    let z_f = z as f32 / step_f;

    let (verts, normal) = match face {
        FaceLod::Top => (
            [
                [x_f, y_f + 1.0, z_f],
                [x_f + 1.0, y_f + 1.0, z_f],
                [x_f + 1.0, y_f + 1.0, z_f + 1.0],
                [x_f, y_f + 1.0, z_f + 1.0],
            ],
            [0.0, 1.0, 0.0],
        ),
        FaceLod::Bottom => (
            [
                [x_f, y_f, z_f + 1.0],
                [x_f + 1.0, y_f, z_f + 1.0],
                [x_f + 1.0, y_f, z_f],
                [x_f, y_f, z_f],
            ],
            [0.0, -1.0, 0.0],
        ),
        FaceLod::Right => (
            [
                [x_f + 1.0, y_f, z_f],
                [x_f + 1.0, y_f, z_f + 1.0],
                [x_f + 1.0, y_f + 1.0, z_f + 1.0],
                [x_f + 1.0, y_f + 1.0, z_f],
            ],
            [1.0, 0.0, 0.0],
        ),
        FaceLod::Left => (
            [
                [x_f, y_f, z_f + 1.0],
                [x_f, y_f, z_f],
                [x_f, y_f + 1.0, z_f],
                [x_f, y_f + 1.0, z_f + 1.0],
            ],
            [-1.0, 0.0, 0.0],
        ),
        FaceLod::Front => (
            [
                [x_f + 1.0, y_f, z_f + 1.0],
                [x_f, y_f, z_f + 1.0],
                [x_f, y_f + 1.0, z_f + 1.0],
                [x_f + 1.0, y_f + 1.0, z_f + 1.0],
            ],
            [0.0, 0.0, 1.0],
        ),
        FaceLod::Back => (
            [
                [x_f, y_f, z_f],
                [x_f + 1.0, y_f, z_f],
                [x_f + 1.0, y_f + 1.0, z_f],
                [x_f, y_f + 1.0, z_f],
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
