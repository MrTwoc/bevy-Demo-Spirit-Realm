//! Tree generator: procedural tree generation with cross-chunk support.
//!
//! 使用两阶段生成方案解决跨区块树木生成问题：
//! - 阶段1：所有区块地形生成完毕
//! - 阶段2：全局树木生成遍历，收集跨区块写入请求
//! - 统一处理 pending_tree_writes
//!
//! # 方块类型约定
//! - block_id = 0: 空气
//! - block_id = 1: 草方块
//! - block_id = 2: 石头
//! - block_id = 3: 泥土
//! - block_id = 4: 沙子
//! - block_id = 5: 水
//! - block_id = 6: 橡木原木 (oak_log)
//! - block_id = 7: 橡木树叶 (oak_leaves)

use crate::chunk::{BlockPos, CHUNK_SIZE, Chunk, ChunkCoord};
use noise::{Fbm, MultiFractal, NoiseFn, Simplex};
use std::collections::HashMap;

/// 树木生成配置
#[derive(Clone, Debug)]
pub struct TreeConfig {
    /// 最小树干高度
    pub min_trunk_height: i32,
    /// 最大树干高度
    pub max_trunk_height: i32,
    /// 树冠半径
    pub crown_radius: i32,
    /// 生成概率（0.0 - 1.0）
    pub spawn_probability: f64,
    /// 树冠最小高度（从地表算起）
    pub crown_min_height: i32,
    /// 树冠最大高度
    pub crown_max_height: i32,
}

impl Default for TreeConfig {
    fn default() -> Self {
        Self {
            min_trunk_height: 4,
            max_trunk_height: 6,
            crown_radius: 2,
            spawn_probability: 0.05, // 5% 概率生成树木（调试用，可降低）
            crown_min_height: 3,
            crown_max_height: 4,
        }
    }
}

/// 跨区块写入请求
/// 当树木跨越区块边界时，生成此请求而非直接写入
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct TreeWriteRequest {
    /// 目标区块坐标
    pub chunk: ChunkCoord,
    /// 方块在世界中的绝对坐标
    pub world_pos: BlockPos,
    /// 要设置的方块 ID
    pub block_id: u8,
}

/// 树木生成结果
#[derive(Clone, Debug, Default)]
pub struct TreeGenerationResult {
    /// 本地写入：直接写入当前区块的方块
    pub local_writes: Vec<(BlockPos, u8)>,
    /// 跨区块写入请求
    pub cross_chunk_writes: Vec<TreeWriteRequest>,
}

/// 树木生成器
pub struct TreeGenerator {
    /// 树生成噪声（用于随机位置）
    tree_noise: Fbm<Simplex>,
    /// 树变种噪声（用于形状变化）
    variant_noise: Fbm<Simplex>,
    /// 配置
    config: TreeConfig,
}

impl TreeGenerator {
    /// 创建新的树木生成器
    pub fn new(seed: u32, config: TreeConfig) -> Self {
        Self {
            tree_noise: Fbm::<Simplex>::new(seed)
                .set_octaves(4)
                .set_frequency(0.05)
                .set_lacunarity(2.0)
                .set_persistence(0.5),
            variant_noise: Fbm::<Simplex>::new(seed.wrapping_add(1000))
                .set_octaves(3)
                .set_frequency(0.1)
                .set_lacunarity(2.0)
                .set_persistence(0.5),
            config,
        }
    }

    /// 检查给定位置是否适合生成树木
    ///
    /// 要求：
    /// - 地表是草方块或泥土
    /// - 树干位置上方不能有水方块（树木不能在水中生成）
    pub fn can_spawn_tree(
        &self,
        chunk: &Chunk,
        chunk_coord: ChunkCoord,
        local_x: usize,
        local_y: usize,
        local_z: usize,
        surface_world_y: i32,
    ) -> bool {
        // 检查地表方块是否是草或泥土
        let surface_block = chunk.get(local_x, local_y, local_z);
        if surface_block != 1 && surface_block != 3 {
            return false; // 不是草方块(1)也不是泥土(3)
        }

        // 检查树干位置上方是否有水方块（block_id = 5）
        // 树木必须在水面上方生成
        for check_y in local_y..CHUNK_SIZE {
            let block_above = chunk.get(local_x, check_y, local_z);
            if block_above == 5 {
                // 水方块，树木不能在此生成
                return false;
            }
        }

        true
    }

    /// 生成一棵树，返回需要写入的方块列表
    ///
    /// `origin_world_y` 是树干底部（地表）的世界 Y 坐标
    pub fn generate_tree(
        &self,
        origin_x: i32,
        origin_z: i32,
        origin_world_y: i32,
    ) -> TreeGenerationResult {
        let mut result = TreeGenerationResult::default();

        // 使用噪声确定树干高度
        let height_noise = self
            .variant_noise
            .get([origin_x as f64 * 0.7, origin_z as f64 * 0.7]);
        let normalized_height = (height_noise + 1.0) * 0.5; // 0 ~ 1
        let trunk_height = self.config.min_trunk_height
            + (normalized_height
                * (self.config.max_trunk_height - self.config.min_trunk_height) as f64)
                as i32;

        // 生成树干
        for y in 0..trunk_height {
            let world_pos = BlockPos {
                x: origin_x,
                y: origin_world_y + y,
                z: origin_z,
            };
            let coord = world_pos.to_chunk_coord();

            if coord == ChunkCoord::from_world_coords(origin_x, origin_world_y, origin_z) {
                // 本地写入
                result.local_writes.push((world_pos, 6)); // 6 = oak_log
            } else {
                // 跨区块写入
                result.cross_chunk_writes.push(TreeWriteRequest {
                    chunk: coord,
                    world_pos,
                    block_id: 6, // oak_log
                });
            }
        }

        // 生成树冠
        let crown_base_y = origin_world_y + trunk_height;
        let crown_variance = self
            .variant_noise
            .get([origin_x as f64 * 1.3, origin_z as f64 * 1.3]);
        let crown_height = self.config.crown_min_height
            + ((crown_variance + 1.0)
                * 0.5
                * (self.config.crown_max_height - self.config.crown_min_height) as f64)
                as i32;

        for dy in 0..crown_height {
            // 椭圆形树冠：底部宽，顶部窄
            let normalized_dy = dy as f64 / crown_height as f64;
            let radius_factor = 1.0 - normalized_dy * 0.5; // 从 1.0 递减到 0.5
            let effective_radius = (self.config.crown_radius as f64 * radius_factor).ceil() as i32;

            for dx in -effective_radius..=effective_radius {
                for dz in -effective_radius..=effective_radius {
                    // 跳过角落，形成更自然的形状
                    if dx * dx + dz * dz > (effective_radius + 1) * (effective_radius + 1) {
                        continue;
                    }

                    let world_pos = BlockPos {
                        x: origin_x + dx,
                        y: crown_base_y + dy,
                        z: origin_z + dz,
                    };
                    let coord = world_pos.to_chunk_coord();

                    if coord == ChunkCoord::from_world_coords(origin_x, origin_world_y, origin_z) {
                        result.local_writes.push((world_pos, 7)); // 7 = oak_leaves
                    } else {
                        result.cross_chunk_writes.push(TreeWriteRequest {
                            chunk: coord,
                            world_pos,
                            block_id: 7, // oak_leaves
                        });
                    }
                }
            }
        }

        result
    }

    /// 决定是否在给定位置生成树木
    pub fn should_spawn_tree(&self, world_x: i32, world_z: i32) -> bool {
        // 约 0.2% 的概率生成树木（约每 500 个地表方块 1 棵树）
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        world_x.hash(&mut hasher);
        world_z.hash(&mut hasher);
        // 使用低 10 位作为随机数，阈值 2 表示约 0.2%
        (hasher.finish() & 0x3FF) < 2
    }
}

/// 扩展 ChunkCoord 以支持从世界坐标创建
impl ChunkCoord {
    /// 从世界坐标创建区块坐标
    pub fn from_world_coords(x: i32, y: i32, z: i32) -> Self {
        Self {
            cx: x.div_euclid(CHUNK_SIZE as i32),
            cy: y.div_euclid(CHUNK_SIZE as i32),
            cz: z.div_euclid(CHUNK_SIZE as i32),
        }
    }
}

/// 在一个区块内生成所有树木
///
/// `chunk_coord` - 区块坐标（因为 Chunk 类型是 ChunkData 别名，不存储坐标）
///
/// 返回：
/// - `local_writes`: 直接应用到当前区块的写入
/// - `cross_chunk_requests`: 需要写入到邻居区块的请求
pub fn generate_trees_in_chunk(
    chunk: &Chunk,
    chunk_coord: ChunkCoord,
    generator: &TreeGenerator,
) -> (Vec<(BlockPos, u8)>, Vec<TreeWriteRequest>) {
    let mut local_writes = Vec::new();
    let mut cross_chunk_requests = Vec::new();

    let mut surfaces_checked = 0;
    let mut can_spawn_count = 0;
    let mut should_spawn_count = 0;

    // 遍历区块内地表
    for z in 0..CHUNK_SIZE {
        for x in 0..CHUNK_SIZE {
            let world_x = chunk_coord.cx as i32 * CHUNK_SIZE as i32 + x as i32;
            let world_z = chunk_coord.cz as i32 * CHUNK_SIZE as i32 + z as i32;

            // 找到这个位置的地表高度
            let surface_world_y = find_surface_height(chunk, x, z, chunk_coord);

            if let Some(surface_y) = surface_world_y {
                surfaces_checked += 1;
                let local_y = surface_y - chunk_coord.cy as i32 * CHUNK_SIZE as i32;

                // 检查是否可以生成树木
                if !generator.can_spawn_tree(chunk, chunk_coord, x, local_y as usize, z, surface_y)
                {
                    continue;
                }
                can_spawn_count += 1;

                // 决定是否生成
                if generator.should_spawn_tree(world_x, world_z) {
                    should_spawn_count += 1;
                    let result = generator.generate_tree(world_x, world_z, surface_y);

                    for (pos, block_id) in result.local_writes {
                        local_writes.push((pos, block_id));
                    }

                    cross_chunk_requests.extend(result.cross_chunk_writes);
                }
            }
        }
    }

    // 调试日志 (已禁用)
    // if surfaces_checked > 0 {
    //     println!(
    //         "[TreeGen] chunk ({},{},{}) surfaces={}, can_spawn={}, should_spawn={}, local_writes={}, cross_chunk={}",
    //         chunk_coord.cx,
    //         chunk_coord.cy,
    //         chunk_coord.cz,
    //         surfaces_checked,
    //         can_spawn_count,
    //         should_spawn_count,
    //         local_writes.len(),
    //         cross_chunk_requests.len()
    //     );
    // }

    (local_writes, cross_chunk_requests)
}

/// 找到区块内指定 XZ 位置的地表高度
fn find_surface_height(
    chunk: &Chunk,
    local_x: usize,
    local_z: usize,
    coord: ChunkCoord,
) -> Option<i32> {
    // 从区块顶部向下扫描
    for y in (0..CHUNK_SIZE).rev() {
        let block_id = chunk.get(local_x, y, local_z);
        if block_id == 1 || block_id == 3 || block_id == 4 {
            // 找到地表（草、泥土或沙子）
            let world_y = coord.cy as i32 * CHUNK_SIZE as i32 + y as i32;
            return Some(world_y);
        }
    }
    None
}

/// 批量应用树木写入请求到多个区块
///
/// # 参数
/// - `chunks`: 可变的区块 map（key 为区块坐标，value 为可变引用）
/// - `requests`: 跨区块写入请求
///
/// # 返回
/// - 被写入的区块坐标列表（用于后续标记 dirty）
pub fn apply_tree_writes(
    chunks: &mut HashMap<ChunkCoord, &mut Chunk>,
    requests: &[TreeWriteRequest],
) -> Vec<ChunkCoord> {
    let mut modified_chunks = Vec::new();

    for request in requests {
        if let Some(chunk) = chunks.get_mut(&request.chunk) {
            // 计算本地坐标
            let local_x =
                (request.world_pos.x - request.chunk.cx as i32 * CHUNK_SIZE as i32) as usize;
            let local_y =
                (request.world_pos.y - request.chunk.cy as i32 * CHUNK_SIZE as i32) as usize;
            let local_z =
                (request.world_pos.z - request.chunk.cz as i32 * CHUNK_SIZE as i32) as usize;

            chunk.set(local_x, local_y, local_z, request.block_id);

            if !modified_chunks.contains(&request.chunk) {
                modified_chunks.push(request.chunk);
            }
        }
    }

    modified_chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tree_generation() {
        let config = TreeConfig::default();
        let generator = TreeGenerator::new(12345, config);

        let result = generator.generate_tree(100, 200, 80);

        // 检查树干是否生成
        let trunk_count = result
            .local_writes
            .iter()
            .filter(|(pos, block_id)| *block_id == 6 && pos.y >= 80 && pos.y < 86)
            .count();
        assert!(trunk_count > 0, "Should generate trunk blocks");

        // 检查树冠是否生成
        let leaf_count = result
            .local_writes
            .iter()
            .filter(|(_, block_id)| *block_id == 7)
            .count();
        assert!(leaf_count > 0, "Should generate leaf blocks");
    }

    #[test]
    fn test_cross_chunk_detection() {
        let config = TreeConfig::default();
        let generator = TreeGenerator::new(12345, config);

        // 在区块边界生成树木
        let boundary_x = 31; // CHUNK_SIZE - 1
        let result = generator.generate_tree(boundary_x, 0, 80);

        // 应该有跨区块写入
        assert!(
            !result.cross_chunk_writes.is_empty(),
            "Tree at boundary should have cross-chunk writes"
        );
    }
}
