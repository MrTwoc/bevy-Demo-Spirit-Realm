//! Tree generation with cross-chunk boundary support.
//!
//! # 跨区块树木生成策略
//!
//! 当树木生成在区块边界时，树木的树干或树叶可能延伸到相邻区块中。
//! 如果相邻区块在生成时不知道这棵树的存在，树木就会被"截断"。
//!
//! ## 解决方案
//!
//! 每个区块独立生成**所有可能进入该区块的树木方块**。
//! 使用确定性噪声（与区块坐标无关），确保相邻区块对同一棵树生成一致的树叶。
//!
//! 处理流程：
//! 1. 扫描**当前区块 XZ 范围内**所有可能的树干位置，找到需要生长在本区块的树木
//! 2. 扫描**相邻区块 XZ 边界附近**（距边界 <= leaf_radius）的树干位置，
//!    这些树的树叶可能延伸到本区块
//! 3. 对每个候选树干位置，计算树木结构（树干 + 树冠）
//! 4. 只放置落在本区块 XZY 范围内的方块

use crate::chunk::{BlockId, ChunkCoord, ChunkData, get_surface_height, CHUNK_SIZE, WATER_LEVEL};
use bevy::prelude::Resource;
use noise::{NoiseFn, Perlin};

// 树木方块类型 ID
pub const TREE_TRUNK: BlockId = 6;  // oak_log
pub const TREE_LEAVES: BlockId = 7; // oak_leaves

/// 树木生成配置
#[derive(Resource, Clone)]
pub struct TreeConfig {
    /// 树干最小高度
    pub trunk_min_height: i32,
    /// 树干最大高度
    pub trunk_max_height: i32,
    /// 树叶从树干向外延伸的半径（曼哈顿距离）
    pub leaf_radius: i32,
    /// 树木生成概率（每个候选 XZ 位置）
    pub spawn_chance: f64,
    /// 检查树木候选位置时的步长（越大性能越好，但树木越稀疏）
    pub tree_step: i32,
}

// # Safety
// TreeConfig 只包含原始类型（i32, f64），可安全跨线程发送和共享。
unsafe impl Send for TreeConfig {}
unsafe impl Sync for TreeConfig {}

impl Default for TreeConfig {
    fn default() -> Self {
        Self {
            trunk_min_height: 4,
            trunk_max_height: 7,
            leaf_radius: 2,
            spawn_chance: 0.12,
            tree_step: 4,
        }
    }
}

/// 树木分布噪声管理器（确定性噪声缓存）
#[derive(Resource, Clone)]
pub struct TreeNoise {
    /// 树木分布噪声（决定哪些位置长树）
    pub distribution: Perlin,
    /// 树木高度变异噪声（决定树的高度）
    pub height_variation: Perlin,
}

// # Safety
// TreeNoise 包含两个 Perlin 噪声生成器。noise crate 的 Perlin 类型实现了 Send + Sync，
// 因此 TreeNoise 可安全跨线程发送和共享。
unsafe impl Send for TreeNoise {}
unsafe impl Sync for TreeNoise {}

impl TreeNoise {
    pub fn new(seed: u32) -> Self {
        Self {
            distribution: Perlin::new(seed),
            height_variation: Perlin::new(seed.wrapping_add(1)),
        }
    }
    pub fn default() -> Self {
        Self::new(54321)
    }
}

/// 在世界坐标 (wx, wz) 处放置树木方块到目标区块。
/// 只放置落在 `target_coord` 范围内的方块。
///
/// `surface_y` 是地表 Y 坐标（树干底部 = surface_y + 1）。
/// `trunk_height` 是树干总高度。
fn place_tree_blocks(
    chunk: &mut ChunkData,
    target_coord: &ChunkCoord,
    trunk_wx: i32,
    trunk_wz: i32,
    surface_y: i32,
    trunk_height: i32,
    config: &TreeConfig,
) {
    let leaf_top_y = surface_y + trunk_height;
    let chunk_ox = target_coord.cx * CHUNK_SIZE as i32;
    let chunk_oy = target_coord.cy * CHUNK_SIZE as i32;
    let chunk_oz = target_coord.cz * CHUNK_SIZE as i32;

    // ── 树干 ──
    for dy in 1..=trunk_height {
        let by = surface_y + dy;
        if by >= chunk_oy && by < chunk_oy + CHUNK_SIZE as i32 {
            let lx = (trunk_wx - chunk_ox) as usize;
            let ly = (by - chunk_oy) as usize;
            let lz = (trunk_wz - chunk_oz) as usize;
            if lx < CHUNK_SIZE && ly < CHUNK_SIZE && lz < CHUNK_SIZE {
                chunk.set(lx, ly, lz, TREE_TRUNK);
            }
        }
    }

    // ── 树叶 ──
    // 树冠分层（y_offset 相对于树干顶部）：
    //   +2: 3x3 十字形
    //   +1: 5x5 挖角（曼哈顿距离 <= 2 且 非四角）
    //    0: 5x5 挖角（同上）
    //   -1: 3x3 十字形
    let leaf_layers: &[(i32, i32, i32)] = &[
        // (y_offset, max_dx, max_dz) 其中 max_dx/max_dz 是曼哈顿距离限制
        (2, 1, 1),  // +2: 3x3 十字
        (1, 2, 2),  // +1: 5x5 挖角
        (0, 2, 2),  //  0: 5x5 挖角
        (-1, 2, 2), // -1: 5x5 挖角（稀疏）
        (-2, 1, 1), // -2: 3x3 十字（稀疏）
    ];

    let r = config.leaf_radius;
    for &(y_offset, max_dx, max_dz) in leaf_layers {
        let ly = leaf_top_y + y_offset;
        if ly < chunk_oy || ly >= chunk_oy + CHUNK_SIZE as i32 {
            continue; // 这层树叶不在当前区块 Y 范围内
        }
        let local_y = (ly - chunk_oy) as usize;

        for dx in -max_dx..=max_dx {
            for dz in -max_dz..=max_dz {
                // 跳过中心柱（树干位置，已有树干方块）
                if dx == 0 && dz == 0 {
                    continue;
                }
                // 跳过超出树叶半径的方块
                if dx.abs() > r || dz.abs() > r {
                    continue;
                }

                // 层特有的过滤规则
                let place = match y_offset {
                    2 => {
                        // 顶层：曼哈顿距离 <= 2 且 dx/dz 至少有一个 <= 1
                        dx.abs() + dz.abs() <= r && (dx.abs() <= 1 || dz.abs() <= 1)
                    }
                    1 | 0 => {
                        // 中间层：曼哈顿距离 <= r，跳过四角
                        dx.abs() + dz.abs() <= r && !(dx.abs() == r && dz.abs() == r)
                    }
                    -1 => {
                        // 下层：曼哈顿距离 <= r，更稀疏（75% 填充）
                        dx.abs() + dz.abs() <= r
                            && !(dx.abs() == r && dz.abs() == r)
                            && (dx.abs() < r || dz.abs() < r || (dx.abs() + dz.abs()) % 2 == 0)
                    }
                    -2 => {
                        // 底层：仅树干对角四角
                        dx.abs() == 1 && dz.abs() == 1
                    }
                    _ => false,
                };

                if !place {
                    continue;
                }

                let wx = trunk_wx + dx;
                let wz = trunk_wz + dz;

                // 只放置当前区块 XZ 范围内的树叶
                if wx >= chunk_ox
                    && wx < chunk_ox + CHUNK_SIZE as i32
                    && wz >= chunk_oz
                    && wz < chunk_oz + CHUNK_SIZE as i32
                {
                    let lx = (wx - chunk_ox) as usize;
                    let lz = (wz - chunk_oz) as usize;
                    // 不要在树干位置上重复放置树叶
                    if chunk.get(lx, local_y, lz) == 0 {
                        chunk.set(lx, local_y, lz, TREE_LEAVES);
                    }
                }
            }
        }
    }
}

/// 检测树干位置是否位于水中。
///
/// 自然生成的树木不应生长在水中。检测逻辑：
/// 1. **全局检测**：地表高度低于水平面（`surface_y < WATER_LEVEL`），
///    说明该 XZ 位置被海洋/水域覆盖，树干会被水淹没。
/// 2. **局部检测**：如果树干基部（`surface_y + 1`）或地表方块（`surface_y`）
///    落在当前区块内且是水方块，说明该位置确实有水。
///
/// 这两个检测相互补充：全局检测能跨区块工作（不依赖本地块数据），
/// 局部检测能捕获本地有水方块的特殊情况。
///
/// # 不影响玩家手动放置
///
/// 此函数仅在 `generate_trees_in_chunk` 中被调用，属于自然生成路径。
/// 未来玩家通过树苗手动种植的树木会使用独立的代码路径（如 `block_interaction`），
/// 不会经过此检测，因此不受影响。
fn is_water_environment(
    chunk: &ChunkData,
    coord: &ChunkCoord,
    trunk_wx: i32,
    trunk_wz: i32,
    surface_y: i32,
) -> bool {
    const WATER_ID: BlockId = 5;

    // ── 检测 1：地表低于水平面 → 该位置被海洋/水域覆盖 ──
    // 这是最可靠的跨区块检测。水平面以下的地形会被水填充，
    // 树木如果在这里生长，树干基部会直接泡在水中。
    if surface_y < WATER_LEVEL {
        return true;
    }

    let chunk_ox = coord.cx * CHUNK_SIZE as i32;
    let chunk_oy = coord.cy * CHUNK_SIZE as i32;
    let chunk_oz = coord.cz * CHUNK_SIZE as i32;

    // ── 检测 2：检查树干基部及地表方块是否为水 ──
    // 如果树干基部或地表正好落在当前区块的 Y 范围内，
    // 可以读取实际方块数据验证。
    for check_y_offset in [0, 1] {
        let check_wy = surface_y + check_y_offset;
        if check_wy >= chunk_oy && check_wy < chunk_oy + CHUNK_SIZE as i32
            && trunk_wx >= chunk_ox && trunk_wx < chunk_ox + CHUNK_SIZE as i32
            && trunk_wz >= chunk_oz && trunk_wz < chunk_oz + CHUNK_SIZE as i32
        {
            let lx = (trunk_wx - chunk_ox) as usize;
            let ly = (check_wy - chunk_oy) as usize;
            let lz = (trunk_wz - chunk_oz) as usize;
            if chunk.get(lx, ly, lz) == WATER_ID {
                return true;
            }
        }
    }

    // ── 检测 3：树干基部周围 4 邻域是否有水 ──
    // 检查树干基部在 y=surface_y 平面的 4 个水平邻居，
    // 防止树木生成在水边时根系陷入水中。
    const NEIGHBOR_OFFSETS: &[(i32, i32)] = &[(1, 0), (-1, 0), (0, 1), (0, -1)];
    let trunk_base_y = surface_y + 1;
    for &(dx, dz) in NEIGHBOR_OFFSETS {
        let wx = trunk_wx + dx;
        let wz = trunk_wz + dz;
        if trunk_base_y >= chunk_oy && trunk_base_y < chunk_oy + CHUNK_SIZE as i32
            && wx >= chunk_ox && wx < chunk_ox + CHUNK_SIZE as i32
            && wz >= chunk_oz && wz < chunk_oz + CHUNK_SIZE as i32
        {
            let lx = (wx - chunk_ox) as usize;
            let ly = (trunk_base_y - chunk_oy) as usize;
            let lz = (wz - chunk_oz) as usize;
            if chunk.get(lx, ly, lz) == WATER_ID {
                return true;
            }
        }
    }

    false
}

/// 在指定区块中生成树木。
///
/// # 跨区块处理
///
/// 为了处理树木跨越区块边界的情况，此函数会扫描一个扩大的 XZ 范围：
/// - 当前区块内的树干位置 → 放置树干和树叶
/// - 相邻区块边界附近的树干位置（距边界 <= leaf_radius）→ 仅放置落在本区块内的树叶
///
/// 由于树木分布使用确定性噪声，相邻区块会独立生成相同树木的匹配树叶部分。
pub fn generate_trees_in_chunk(
    chunk: &mut ChunkData,
    coord: &ChunkCoord,
    config: &TreeConfig,
    noise: &TreeNoise,
) {
    let chunk_ox = coord.cx * CHUNK_SIZE as i32;
    let chunk_oz = coord.cz * CHUNK_SIZE as i32;
    let chunk_oy = coord.cy * CHUNK_SIZE as i32;

    // 搜索范围 = 当前区块 XZ 范围 + 树叶半径扩展
    // 这样能捕获到树干在相邻区块但树叶伸入本区块的树木
    let search_extend = config.leaf_radius + config.trunk_max_height + 2;
    let search_min_x = chunk_ox - search_extend;
    let search_max_x = chunk_ox + CHUNK_SIZE as i32 + search_extend;
    let search_min_z = chunk_oz - search_extend;
    let search_max_z = chunk_oz + CHUNK_SIZE as i32 + search_extend;

    // 跳过高度超出当前区块 Y 范围太多的树干
    // 树木需要的地表 Y 范围
    // 假设 trunk_bottom_y = surface_y + 1
    // 所以 surface_y 需要在 [chunk_oy - 1 - trunk_max_height, chunk_oy + CHUNK_SIZE - 1]
    let min_trunk_y = chunk_oy - 1 - config.trunk_max_height;
    let max_trunk_y = chunk_oy + CHUNK_SIZE as i32 + config.trunk_max_height + 5;

    let step = config.tree_step;

    // 遍历扩展搜索范围内的所有树干候选位置
    let mut trunk_x = search_min_x;
    while trunk_x <= search_max_x {
        let mut trunk_z = search_min_z;
        while trunk_z <= search_max_z {
            // 使用 Perlin 噪声判断是否在该位置生成树木
            // 频率较低使树木呈聚落状分布
            let noise_val = noise.distribution.get([
                trunk_x as f64 * 0.035,
                trunk_z as f64 * 0.035,
            ]);

            // noise_val 范围 [-1, 1]，映射到 [0, 1]
            let spawn_prob = noise_val * 0.5 + 0.5;
            if spawn_prob > config.spawn_chance {
                trunk_z += step;
                continue;
            }

            // 计算地表高度（使用与地形生成相同的确定性噪声）
            let surface_y = get_surface_height(trunk_x as f64, trunk_z as f64);

            // 跳过地表高度超出树木能触及范围的候选项
            if surface_y < min_trunk_y || surface_y > max_trunk_y {
                trunk_z += step;
                continue;
            }

            // ── 水方块检测 ──
            // 自然生成的树木不应生长在以下情况：
            // - 地表低于水平面（海洋/水下区域）
            // - 树干基部或地表方块本身就是水
            // - 树干基部周围 4 个水平邻居中有水
            //
            // 此检测只影响自然生成路径。
            // 未来玩家通过树苗手动种植的树木使用独立的代码路径，不受此限制。
            if is_water_environment(chunk, coord, trunk_x, trunk_z, surface_y) {
                trunk_z += step;
                continue;
            }

            // 树干高度：使用第二个噪声维度获得变化
            let height_scale = noise.height_variation.get([
                trunk_x as f64 * 0.2,
                trunk_z as f64 * 0.2,
            ]);
            // height_scale 范围 [-1, 1]，映射到 [trunk_min, trunk_max]
            let height_range = (config.trunk_max_height - config.trunk_min_height + 1) as f64;
            let trunk_height = config.trunk_min_height
                + ((height_scale * 0.5 + 0.5) * height_range) as i32;
            let trunk_height = trunk_height.clamp(config.trunk_min_height, config.trunk_max_height);

            place_tree_blocks(chunk, coord, trunk_x, trunk_z, surface_y, trunk_height, config);

            trunk_z += step;
        }
        trunk_x += step;
    }
}
