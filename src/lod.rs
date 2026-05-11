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
//!
//! # 核心设计
//!
//! - **降采样网格生成**：对 32³ 体素空间每隔 N 个体素采样一次
//! - **滞后切换策略**：玩家靠近时立即降级 LOD，远离时需要超过阈值才升级
//! - **跨 LOD 接缝处理**：面可见性检查时考虑邻居区块的 LOD 级别

use std::collections::HashMap;

use crate::async_mesh::UvLookupTable;
use crate::chunk::{BlockId, CHUNK_SIZE, ChunkCoord, ChunkData, ChunkNeighbors};
use bevy::prelude::Resource;

// ============================================================================
// LOD 级别定义
// ============================================================================

/// LOD 级别定义
///
/// 每级降采样 2x，四级 LOD 覆盖 32 区块视距：
/// - LOD0 = 32³ 体素（全精度）
/// - LOD1 = 16³ 体素（降采样 2x）
/// - LOD2 = 8³ 体素（降采样 4x）
/// - LOD3 = 4³ 体素（降采样 8x）
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LodLevel {
    Lod0 = 0, // 1:1, 0-8 chunks
    Lod1 = 1, // 1:2, 9-16 chunks
    Lod2 = 2, // 1:4, 17-24 chunks
    Lod3 = 3, // 1:8, 25-32 chunks
}

impl LodLevel {
    /// 最大 LOD 级别（用于边界检查）
    pub const MAX: usize = 3;

    /// 降采样步长（体素数）
    ///
    /// 例如：LOD1 的 step=2 表示每 2x2x2 体素取 1 个采样点
    #[inline]
    pub const fn step(self) -> usize {
        match self {
            LodLevel::Lod0 => 1,
            LodLevel::Lod1 => 2,
            LodLevel::Lod2 => 4,
            LodLevel::Lod3 => 8,
        }
    }

    /// 降采样后的区块体素数（单轴）
    ///
    /// 例如：LOD1 的 sampling_size=16，表示降采样后区块是 16³
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
    ///
    /// 使用距离阈值 9/17/25 来划分四个 LOD 环
    pub fn from_chunk_distance(dist_chunks: f32) -> Self {
        match dist_chunks {
            d if d < 9.0 => LodLevel::Lod0,
            d if d < 17.0 => LodLevel::Lod1,
            d if d < 25.0 => LodLevel::Lod2,
            _ => LodLevel::Lod3,
        }
    }

    /// 获取该 LOD 级别的距离阈值上限
    #[inline]
    fn threshold(self) -> f32 {
        match self {
            LodLevel::Lod0 => 8.0,
            LodLevel::Lod1 => 16.0,
            LodLevel::Lod2 => 24.0,
            LodLevel::Lod3 => 32.0,
        }
    }

    /// 获取该 LOD 级别的距离阈值下限
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

/// LOD 管理器
///
/// 管理每个已加载区块的当前 LOD 级别，处理 LOD 切换决策。
/// 使用滞后策略避免玩家在小范围内移动时 LOD 频繁切换导致的视觉闪烁。
///
/// # 滞后策略
///
/// - **靠近（降级 LOD）**：立即从粗糙切换到精细，保证视觉质量
/// - **远离（升级 LOD）**：需要额外走过半个距离环宽度才切换，避免小幅后退导致频繁升级
#[derive(Resource)]
pub struct LodManager {
    /// 每个区块当前的 LOD 级别
    chunk_lods: HashMap<ChunkCoord, LodLevel>,
    /// 滞后距离（区块为单位）：切换 LOD 需要额外多走一半距离
    hysteresis: f32,
}

impl LodManager {
    /// 创建 LOD 管理器
    pub fn new() -> Self {
        Self {
            chunk_lods: HashMap::new(),
            hysteresis: 0.5, // 半区块的滞后
        }
    }

    /// 更新所有已加载区块的 LOD 级别
    ///
    /// 返回需要重建网格的区块列表（LOD 级别发生变化的区块）。
    ///
    /// # 参数
    /// - `player_chunk`: 玩家所在的区块坐标
    /// - `loaded`: 已加载区块的引用
    ///
    /// # 返回
    /// 需要重建网格的 (坐标, 新LOD级别) 列表
    pub fn update(
        &mut self,
        player_chunk: ChunkCoord,
        loaded: &LoadedChunks,
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

            // 只有 LOD 级别发生变化时才考虑触发重建
            if new_lod != current_lod {
                // 滞后检查：距离变化超过阈值才切换
                if self.should_switch(current_lod, new_lod, dist) {
                    self.chunk_lods.insert(*coord, new_lod);
                    to_rebuild.push((*coord, new_lod));
                }
            }
        }

        to_rebuild
    }

    /// 获取区块的当前 LOD 级别
    ///
    /// 如果区块未记录，返回 LOD0（全精度）
    pub fn get_lod(&self, coord: &ChunkCoord) -> LodLevel {
        self.chunk_lods
            .get(coord)
            .copied()
            .unwrap_or(LodLevel::Lod0)
    }

    /// 设置区块的 LOD 级别（用于初始化）
    pub fn set_lod(&mut self, coord: ChunkCoord, lod: LodLevel) {
        self.chunk_lods.insert(coord, lod);
    }

    /// 移除区块的 LOD 记录（用于卸载）
    pub fn remove(&mut self, coord: &ChunkCoord) {
        self.chunk_lods.remove(coord);
    }

    /// 计算两个 ChunkCoord 之间的区块距离（欧几里得距离）
    fn chunk_distance(&self, a: ChunkCoord, b: ChunkCoord) -> f32 {
        let dx = (a.cx - b.cx) as f32;
        let dy = (a.cy - b.cy) as f32;
        let dz = (a.cz - b.cz) as f32;
        (dx * dx + dy * dy + dz * dz).sqrt()
    }

    /// 滞后切换决策
    ///
    /// # 规则
    /// - 玩家靠近（降级 LOD）：立即切换，无滞后
    /// - 玩家远离（升级 LOD）：需要超过滞后阈值
    fn should_switch(&self, current: LodLevel, new: LodLevel, dist: f32) -> bool {
        if (new as i32) < (current as i32) {
            // 玩家靠近：降级 LOD（从粗糙到精细），立即切换
            true
        } else {
            // 玩家远离：升级 LOD（从精细到粗糙），使用滞后
            // 需要额外走 hysteresis * (距离环宽度) 才切换
            let ring_width = 8.0; // 所有级别使用相同的环宽度
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

/// 面方向定义，与 async_mesh.rs 中 FACES_ASYNC 一致。
/// 元组格式：(面方向, 法线偏移, UV 数组索引)
/// UV 数组索引：0=top, 1=bottom, 2=side
const FACES_LOD: [(FaceLod, [i32; 3], usize); 6] = [
    (FaceLod::Right, [1, 0, 0], 2),   // side
    (FaceLod::Left, [-1, 0, 0], 2),   // side
    (FaceLod::Top, [0, 1, 0], 0),     // top
    (FaceLod::Bottom, [0, -1, 0], 1), // bottom
    (FaceLod::Front, [0, 0, 1], 2),   // side
    (FaceLod::Back, [0, 0, -1], 2),   // side
];

/// 面方向枚举（工作线程本地副本）
#[derive(Clone, Copy)]
enum FaceLod {
    Top,
    Bottom,
    Right,
    Left,
    Front,
    Back,
}

/// LOD 降采样网格生成函数
///
/// 根据指定的 LOD 级别对区块体素进行降采样，生成精简网格。
///
/// # 算法
///
/// 1. 根据 LOD 级别计算降采样步长（step）和采样范围（sampling_size）
/// 2. 每隔 `step` 个体素采样一次，映射到原区块坐标
/// 3. 对采样点执行标准的逐面可见性检查（使用放大的法线偏移）
/// 4. 生成大小为 `step × step` 的四边形（每个采样点代表 step³ 体素空间）
///
/// # 参数
/// - `chunk`: 原始 32³ 区块数据
/// - `uv_table`: UV 查找表
/// - `neighbors`: 6 方向邻居数据（原始 32³ 分辨率）
/// - `lod`: LOD 级别（决定降采样步长）
///
/// # 返回
/// (positions, uvs, normals, indices) - 标准顶点格式
pub fn generate_lod_mesh(
    chunk: &ChunkData,
    uv_table: &UvLookupTable,
    neighbors: &ChunkNeighbors,
    lod: LodLevel,
) -> (Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<[f32; 3]>, Vec<u32>) {
    // LOD0 使用标准生成逻辑（不应该调用这个函数）
    if matches!(lod, LodLevel::Lod0) {
        return (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    }

    let step = lod.step();
    let step_f = step as f32;
    let sample_size = lod.sampling_size();

    // 全空气区块提前返回
    if matches!(chunk, ChunkData::Empty | ChunkData::Uniform(0)) {
        return (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    }

    // 预分配容量（按面数估算）
    // LOD1: ~500-1000 面，LOD2: ~60-125 面，LOD3: ~8-16 面
    let capacity = match lod {
        LodLevel::Lod1 => 1200,
        LodLevel::Lod2 => 150,
        LodLevel::Lod3 => 20,
        LodLevel::Lod0 => 48000, // 不会用到
    };

    let mut positions = Vec::with_capacity(capacity);
    let mut uvs = Vec::with_capacity(capacity);
    let mut normals = Vec::with_capacity(capacity);
    let mut indices = Vec::with_capacity(capacity * 2);

    for sz in 0..sample_size {
        for sy in 0..sample_size {
            for sx in 0..sample_size {
                // 映射到原区块坐标
                let x = sx * step;
                let y = sy * step;
                let z = sz * step;

                // 在 step³ 子体素块中扫描，取最高非空气方块作为采样结果。
                // 这样可以保留地表层（草方块/沙子），避免降采样时跳过只有
                // 1 层厚的草方块而直接采样到下方的泥土层。
                let block_id = sample_dominant_block(chunk, x, y, z, step);
                if block_id == 0 {
                    continue;
                }

                // 检查 6 个面（使用 LOD 版本的面可见性检查）
                for (face_index, (face, offset, uv_idx)) in FACES_LOD.iter().cloned().enumerate() {
                    // 法线偏移量乘以 step（降采样后需要更大的偏移）
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
                    ) {
                        continue;
                    }

                    let base_index = positions.len() as u32;

                    // 从 UV 查找表获取坐标
                    let uv = uv_table.get_uv(block_id, uv_idx);

                    // LOD 降采样：四边形大小为 step × step（代表降采样后每个体素覆盖的原始空间）
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

/// 在 step³ 子体素块中采样"最高非空气方块"。
///
/// 从子体素块的顶部向下扫描，返回第一个遇到的非空气方块 ID。
/// 这确保地表层（草方块/沙子等 1 层厚的方块）不会被降采样跳过。
///
/// # 参数
/// - `chunk`: 区块数据
/// - `base_x, base_y, base_z`: 子体素块在区块中的起始坐标
/// - `step`: 子体素块边长
fn sample_dominant_block(
    chunk: &ChunkData,
    base_x: usize,
    base_y: usize,
    base_z: usize,
    step: usize,
) -> BlockId {
    // 从顶部向下扫描，找到最高层的非空气方块
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
    0 // 全部是空气
}

/// LOD 版本的面可见性检查
///
/// 与标准版本的区别：
/// 1. 法线偏移已乘以 step（用于降采样后的体素）
/// 2. 邻居查询时使用 rem_euclid 处理区块边界
/// 3. `current_id` 由外部传入（来自 `sample_dominant_block`），
///    而非直接 `chunk.get(x, y, z)`，确保面剔除使用正确的采样方块 ID
///
/// # 参数
/// - `chunk`: 区块数据
/// - `x, y, z`: 采样点坐标（降采样后的坐标）
/// - `current_id`: 当前采样点的方块 ID（由 `sample_dominant_block` 返回）
/// - `lod_offset`: 法线偏移量（已乘以 step）
/// - `face_index`: 面索引（0-5）
/// - `neighbors`: 邻居数据
fn is_face_visible_lod(
    chunk: &ChunkData,
    x: usize,
    y: usize,
    z: usize,
    current_id: BlockId,
    lod_offset: &[i32; 3],
    face_index: usize,
    neighbors: &ChunkNeighbors,
) -> bool {
    let nx = x as i32 + lod_offset[0];
    let ny = y as i32 + lod_offset[1];
    let nz = z as i32 + lod_offset[2];

    // 邻居在区块边界内
    if nx >= 0
        && ny >= 0
        && nz >= 0
        && nx < CHUNK_SIZE as i32
        && ny < CHUNK_SIZE as i32
        && nz < CHUNK_SIZE as i32
    {
        return chunk.get(nx as usize, ny as usize, nz as usize) != current_id;
    }

    // 邻居在区块边界外：使用 rem_euclid 处理环绕
    // 这确保了跨区块的面可见性检查正确工作
    let neighbor_x = nx.rem_euclid(CHUNK_SIZE as i32) as usize;
    let neighbor_y = ny.rem_euclid(CHUNK_SIZE as i32) as usize;
    let neighbor_z = nz.rem_euclid(CHUNK_SIZE as i32) as usize;

    let neighbor_id = neighbors.get_neighbor_block(face_index, neighbor_x, neighbor_y, neighbor_z);
    neighbor_id != current_id
}

/// LOD 版本的面四边形生成
///
/// 与标准版本的区别：四边形大小为 `step × step` 而非 `1 × 1`，
/// 因为每个 LOD 采样点代表 `step × step × step` 的原始体素空间。
///
/// **重要**：顶点顺序必须与 [`crate::async_mesh::face_quad_async`] 完全一致，
/// 只是将 `1.0` 替换为 `step`。否则会导致三角形绕序错误，被背面剔除隐藏一个三角形。
fn face_quad_lod(
    x: usize,
    y: usize,
    z: usize,
    face: FaceLod,
    uv: (f32, f32, f32, f32),
    step: f32,
) -> ([[f32; 3]; 4], [[f32; 2]; 4], [f32; 3]) {
    let (verts, normal) = match face {
        FaceLod::Top => (
            [
                [x as f32, y as f32 + step, z as f32],
                [x as f32 + step, y as f32 + step, z as f32],
                [x as f32 + step, y as f32 + step, z as f32 + step],
                [x as f32, y as f32 + step, z as f32 + step],
            ],
            [0.0, 1.0, 0.0],
        ),
        FaceLod::Bottom => (
            [
                [x as f32, y as f32, z as f32 + step],
                [x as f32 + step, y as f32, z as f32 + step],
                [x as f32 + step, y as f32, z as f32],
                [x as f32, y as f32, z as f32],
            ],
            [0.0, -1.0, 0.0],
        ),
        FaceLod::Right => (
            [
                [x as f32 + step, y as f32, z as f32],
                [x as f32 + step, y as f32, z as f32 + step],
                [x as f32 + step, y as f32 + step, z as f32 + step],
                [x as f32 + step, y as f32 + step, z as f32],
            ],
            [1.0, 0.0, 0.0],
        ),
        FaceLod::Left => (
            [
                [x as f32, y as f32, z as f32 + step],
                [x as f32, y as f32, z as f32],
                [x as f32, y as f32 + step, z as f32],
                [x as f32, y as f32 + step, z as f32 + step],
            ],
            [-1.0, 0.0, 0.0],
        ),
        FaceLod::Front => (
            [
                [x as f32 + step, y as f32, z as f32 + step],
                [x as f32, y as f32, z as f32 + step],
                [x as f32, y as f32 + step, z as f32 + step],
                [x as f32 + step, y as f32 + step, z as f32 + step],
            ],
            [0.0, 0.0, 1.0],
        ),
        FaceLod::Back => (
            [
                [x as f32, y as f32, z as f32],
                [x as f32 + step, y as f32, z as f32],
                [x as f32 + step, y as f32 + step, z as f32],
                [x as f32, y as f32 + step, z as f32],
            ],
            [0.0, 0.0, -1.0],
        ),
    };

    let (u_min, u_max, v_min, v_max) = uv;
    let eps = 0.016;
    let face_uvs = [
        [u_min + eps, v_max - eps],
        [u_max - eps, v_max - eps],
        [u_max - eps, v_min + eps],
        [u_min + eps, v_min + eps],
    ];

    (verts, face_uvs, normal)
}

// ============================================================================
// Re-exports for convenience
// ============================================================================

/// 已加载区块的类型别名（避免循环依赖）
pub type LoadedChunks = crate::chunk_manager::LoadedChunks;
