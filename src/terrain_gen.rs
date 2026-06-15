//! 地形生成系统。
//!
//! 提供多种世界类型的地形填充函数：
//! - `fill_terrain`：噪声世界（5 层噪声 + 生物群系 + 洞穴）
//! - `fill_flat_terrain`：平坦世界（草 + 泥土）
//! - `fill_menger_sponge`：门格海绵分形世界

use crate::biome;
use crate::chunk::{Chunk, WorldType};
use crate::terrain_noise::{
    TERRAIN_BASE_HEIGHT, TERRAIN_MAX_Y, TERRAIN_MIN_Y,
    SEA_LEVEL, DIRT_LAYER_DEPTH, compute_surface_height, get_terrain_noise,
};
use crate::types::{AIR, GRASS, DIRT, STONE, WATER, CHUNK_SIZE, ChunkCoord};

/// 获取世界坐标 (world_x, world_z) 处的**地表高度**。
///
/// 使用 5 层噪声系统（`terrain_noise::compute_surface_height`）计算，
/// 确保在任何位置（跨越区块边界）计算的地表高度一致。
///
/// 此函数是确定性的——相同的 (world_x, world_z) 总是返回相同的高度值。
/// 这使得树木生成可以在不依赖邻近区块数据的情况下正确计算树木位置。
pub fn get_surface_height(world_x: f64, world_z: f64, world_type: WorldType) -> i32 {
    match world_type {
        WorldType::Flat => TERRAIN_BASE_HEIGHT,
        WorldType::Void | WorldType::MengerSponge => i32::MIN,
        WorldType::Noise => compute_surface_height(world_x, world_z),
    }
}

/// 使用 5 层噪声系统 + 生物群系填充区块地形。
///
/// # 算法
///
/// 1. 采样 5 个噪声层（大陆性、侵蚀、山脊、温度、植被）
/// 2. 通过 `NoiseSample::compute_base_height()` 计算地表高度
/// 3. 通过 `get_biome()` 判定生物群系
/// 4. 根据群系选择地表/地下方块
/// 5. 海平面以下的地表凹陷处填充水(5)
pub fn fill_terrain(chunk: &mut Chunk, coord: &ChunkCoord) {
    let noise = get_terrain_noise();

    // ── 洞穴检测缓存（2×2×2 空间降采样）──
    // is_cave() 每次调用执行 2 次 3D Simplex 噪声采样，是本函数最大热点。
    // 将邻近体素（2×2×2 格内）的洞穴查询结果缓存，减少约 8 倍的噪声调用。
    // 复用单个 HashMap，每列 clear() 替代 1024 次 HashMap::new() 分配。
    const CAVE_CACHE_GRID: i32 = 2;
    let mut cave_cache: std::collections::HashMap<(i32, i32, i32), bool> =
        std::collections::HashMap::with_capacity(32);

    for z in 0..CHUNK_SIZE {
        for x in 0..CHUNK_SIZE {
            let world_x = coord.cx as f64 * CHUNK_SIZE as f64 + x as f64;
            let world_z = coord.cz as f64 * CHUNK_SIZE as f64 + z as f64;

            let sample = noise.sample_all(world_x, world_z);
            let surface_height = sample.compute_base_height() as i32;
            let biome = biome::get_biome(&sample, surface_height);

            // 每列清空缓存（保留容量，零分配）
            cave_cache.clear();

            for y in 0..CHUNK_SIZE {
                let world_y = coord.cy as i32 * CHUNK_SIZE as i32 + y as i32;

                if world_y > TERRAIN_MAX_Y || world_y < TERRAIN_MIN_Y {
                    continue;
                }

                // ── 地表以上 ──
                if world_y > surface_height {
                    if world_y <= SEA_LEVEL && surface_height < SEA_LEVEL {
                        chunk.set(x, y, z, WATER);
                    }
                    continue;
                }

                // ── 地表及以下 ──
                let depth = surface_height - world_y;

                // 洞穴检测（地表以下 5 格以下），使用 2×2×2 缓存降采样
                let sx = ((world_x as i32) / CAVE_CACHE_GRID) * CAVE_CACHE_GRID + CAVE_CACHE_GRID / 2;
                let sy = (world_y / CAVE_CACHE_GRID) * CAVE_CACHE_GRID + CAVE_CACHE_GRID / 2;
                let sz = ((world_z as i32) / CAVE_CACHE_GRID) * CAVE_CACHE_GRID + CAVE_CACHE_GRID / 2;
                let cache_key = (sx, sy, sz);

                let is_cave_result = *cave_cache.entry(cache_key).or_insert_with(|| {
                    noise.is_cave(world_x, world_y as f64, world_z, depth)
                });

                if depth > 5 && is_cave_result {
                    continue; // 挖空
                }

                let block_id = if depth == 0 {
                    biome.surface_block(world_y)
                } else {
                    biome.subsurface_block(depth)
                };
                chunk.set(x, y, z, block_id);
            }
        }
    }
}

/// 填充平台世界地形（超平坦，只有草方块和泥土）
///
/// 表面 Y=`TERRAIN_BASE_HEIGHT`(80) 为草方块(grass=1)，
/// 下方 `DIRT_LAYER_DEPTH`(4) 层为泥土(dirt=3)，
/// 不生成石头和水（与噪声世界截然不同）。
pub fn fill_flat_terrain(chunk: &mut Chunk, coord: &ChunkCoord) {
    for z in 0..CHUNK_SIZE {
        for x in 0..CHUNK_SIZE {
            for y in 0..CHUNK_SIZE {
                let world_y = coord.cy as i32 * CHUNK_SIZE as i32 + y as i32;

                if world_y == TERRAIN_BASE_HEIGHT {
                    chunk.set(x, y, z, GRASS);
                } else if world_y > TERRAIN_BASE_HEIGHT - DIRT_LAYER_DEPTH
                    && world_y < TERRAIN_BASE_HEIGHT
                {
                    chunk.set(x, y, z, DIRT);
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
