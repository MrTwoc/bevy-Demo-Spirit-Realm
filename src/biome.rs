//! 生物群系判定系统
//!
//! 三轴映射：温度 × 植被 × 海拔 → BiomeType
//!
//! # 群系判定流程
//!
//! 1. 海洋/深海 → 直接返回（海拔覆盖）
//! 2. 山脉（海拔 > 180）→ 直接返回（海拔覆盖）
//! 3. 温度 × 植被矩阵 → 平原/沙漠/森林/雪原
//!
//! # 群系地形特征
//!
//! 地形形状由温度/植被噪声直接调制（在 `compute_base_height` 中）：
//! - **沙漠**（热+干）：沙丘起伏（±8 格正弦波）
//! - **雪原/山脉**（冷）：山脊增强 1.4 倍
//! - **森林**（湿）：地形更平缓（×0.75）
//! - **平原**（温和）：标准地形
//!
//! # 自然过渡
//!
//! 群系之间无硬边界。温度和植被噪声频率极低（0.0006），
//! 相邻位置的温度/植被值差异极小，群系自然渐变。

use crate::terrain_noise::{NoiseSample, SEA_LEVEL, TerrainType};

// ══════════════════════════════════════════════════════════════════════════════
// 方块 ID
// ══════════════════════════════════════════════════════════════════════════════

// 现有方块
pub const AIR: u8 = 0;
pub const GRASS: u8 = 1;
pub const STONE: u8 = 2;
pub const DIRT: u8 = 3;
pub const SAND: u8 = 4;
pub const WATER: u8 = 5;
pub const TREE_TRUNK: u8 = 6;
pub const TREE_LEAVES: u8 = 7;

// S2 新增方块
pub const SANDSTONE: u8 = 8;   // 沙石（沙漠地下层）
pub const SNOW_GRASS: u8 = 9;  // 雪地草（寒冷群系地表）
pub const GRAVEL: u8 = 10;     // 砂砾（河床/山脚）
pub const ROCK: u8 = 11;       // 岩石变体（山脉裸露）
pub const MUD: u8 = 12;        // 泥土变体（河岸/湿地）

// ══════════════════════════════════════════════════════════════════════════════
// BiomeType
// ══════════════════════════════════════════════════════════════════════════════

/// 生物群系类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BiomeType {
    DeepOcean,  // 深海
    Ocean,      // 海洋
    Plains,     // 平原
    Desert,     // 沙漠
    Forest,     // 森林
    Mountains,  // 山脉
    SnowPlains, // 雪原
}

// ══════════════════════════════════════════════════════════════════════════════
// 噪声分级
// ══════════════════════════════════════════════════════════════════════════════

/// 温度分级（5 级）— 参考 Tectonic temperature_index
#[inline]
fn temperature_index(t: f64) -> u8 {
    if t < -0.48 {
        1 // 极寒
    } else if t < -0.18 {
        2 // 寒冷
    } else if t < 0.17 {
        3 // 温和
    } else if t < 0.52 {
        4 // 温暖
    } else {
        5 // 炎热
    }
}

/// 植被分级（5 级）— 参考 Tectonic vegetation_index
#[inline]
fn vegetation_index(v: f64) -> u8 {
    if v < -0.38 {
        1 // 干旱
    } else if v < -0.13 {
        2 // 干燥
    } else if v < 0.07 {
        3 // 中等
    } else if v < 0.27 {
        4 // 湿润
    } else {
        5 // 茂盛
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// 群系判定
// ══════════════════════════════════════════════════════════════════════════════

/// 根据噪声采样和地表高度判定生物群系。
///
/// # 判定优先级
///
/// 1. 海洋/深海（continentalness 覆盖）
/// 2. 山脉（海拔 > 180 覆盖）
/// 3. 温度 × 植被矩阵
#[inline]
pub fn get_biome(sample: &NoiseSample, surface_height: i32) -> BiomeType {
    // 海洋覆盖（由 continentalness 决定）
    let tt = sample.terrain_type();
    if tt == TerrainType::DeepOcean {
        return BiomeType::DeepOcean;
    }
    if tt == TerrainType::Ocean {
        return BiomeType::Ocean;
    }

    // 山脉覆盖（海拔 > 180）
    if surface_height > 180 {
        return BiomeType::Mountains;
    }

    // 温度 × 植被矩阵
    let ti = temperature_index(sample.temperature);
    let vi = vegetation_index(sample.vegetation);

    match (ti, vi) {
        // 极寒 → 雪原
        (1, _) => BiomeType::SnowPlains,
        // 寒冷
        (2, 1..=2) => BiomeType::SnowPlains,
        (2, _) => BiomeType::Forest,
        // 温和
        (3, 1) => BiomeType::Plains,
        (3, 2) => BiomeType::Plains,
        (3, _) => BiomeType::Forest,
        // 温暖
        (4, 1) => BiomeType::Desert,
        (4, 2) => BiomeType::Desert,
        (4, 3) => BiomeType::Plains,
        (4, _) => BiomeType::Forest,
        // 炎热
        (5, 1..=3) => BiomeType::Desert,
        (5, 4) => BiomeType::Plains,
        (5, _) => BiomeType::Forest,
        _ => BiomeType::Plains,
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// 群系 → 方块映射
// ══════════════════════════════════════════════════════════════════════════════

impl BiomeType {
    /// 地表方块（最顶层）
    #[inline]
    pub fn surface_block(self, world_y: i32) -> u8 {
        match self {
            BiomeType::DeepOcean | BiomeType::Ocean => {
                // 海底：浅层沙，深层石头
                if world_y > SEA_LEVEL - 8 {
                    SAND
                } else {
                    STONE
                }
            }
            BiomeType::Plains | BiomeType::Forest => GRASS,
            BiomeType::Desert => SAND,
            BiomeType::Mountains => {
                // 高海拔裸露岩石，低海拔草地
                if world_y > 160 {
                    ROCK
                } else if world_y > 130 {
                    STONE
                } else {
                    GRASS
                }
            }
            BiomeType::SnowPlains => SNOW_GRASS,
        }
    }

    /// 地下层方块（地表以下 depth 格）
    #[inline]
    pub fn subsurface_block(self, depth: i32) -> u8 {
        match self {
            BiomeType::Desert => {
                if depth < 4 {
                    SAND
                } else {
                    SANDSTONE
                }
            }
            BiomeType::Mountains => {
                if depth < 2 {
                    GRAVEL
                } else {
                    STONE
                }
            }
            _ => {
                if depth <= 4 {
                    DIRT
                } else {
                    STONE
                }
            }
        }
    }

    /// 树木密度乘数（相对于默认密度）
    #[inline]
    pub fn tree_density_multiplier(self) -> f64 {
        match self {
            BiomeType::Forest => 2.5,     // 密林
            BiomeType::Plains => 0.4,     // 稀疏
            BiomeType::SnowPlains => 0.2, // 极少
            BiomeType::Desert => 0.0,     // 无树
            BiomeType::Mountains => 0.1,  // 极少
            BiomeType::Ocean | BiomeType::DeepOcean => 0.0,
        }
    }
}
