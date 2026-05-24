//! 生物群系系统 Biome System
//!
//! 定义了 9 种基础群系，用于驱动地形、植被和方块类型选择。

use crate::chunk::BlockId;

/// 群系ID枚举
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BiomeId {
    Desert = 0,    // 沙漠
    Savanna = 1,   // 热带草原
    Jungle = 2,    // 丛林
    Swamp = 3,     // 沼泽
    Plains = 4,    // 平原
    Forest = 5,    // 森林
    Taiga = 6,     // 针叶林
    Tundra = 7,    // 冰原
    Mountains = 8, // 高山
}

impl BiomeId {
    /// 从 u8 值转换为 BiomeId
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(BiomeId::Desert),
            1 => Some(BiomeId::Savanna),
            2 => Some(BiomeId::Jungle),
            3 => Some(BiomeId::Swamp),
            4 => Some(BiomeId::Plains),
            5 => Some(BiomeId::Forest),
            6 => Some(BiomeId::Taiga),
            7 => Some(BiomeId::Tundra),
            8 => Some(BiomeId::Mountains),
            _ => None,
        }
    }
}

/// 树木类型枚举
#[derive(Debug, Clone, Copy)]
pub enum TreeType {
    None,       // 无树木
    Oak,        // 橡树（平原、森林）
    Pine,       // 松树（针叶林）
    JungleTree, // 丛林树（密林）
    SwampTree,  // 沼泽树（含藤蔓）
}

/// 群系定义
#[derive(Debug, Clone)]
pub struct Biome {
    /// 群系ID
    pub id: BiomeId,
    /// 群系名称
    pub name: &'static str,

    /// 基础温度 (-1.0 ~ 1.0)
    pub temperature: f64,
    /// 基础湿度 (-1.0 ~ 1.0)
    pub humidity: f64,

    /// 地表方块ID
    pub surface_block: BlockId,
    /// 次表层方块ID
    pub under_surface_block: BlockId,
    /// 表土厚度
    pub soil_thickness: i32,

    /// 树木密度 (0.0 ~ 1.0)
    pub tree_density: f32,
    /// 树木类型
    pub tree_type: TreeType,
    /// 草地覆盖度 (0.0 ~ 1.0)
    pub grass_density: f32,

    /// 是否有积雪
    pub is_snowy: bool,
    /// 是否湿润（水体）
    pub is_wet: bool,
    /// 是否贫瘠（无植被）
    pub is_sparse: bool,
}

// ============================================================================
// 群系数据表
// ============================================================================

/// 沙漠群系 - 金色沙丘，无植被
pub static BIOME_DESERT: Biome = Biome {
    id: BiomeId::Desert,
    name: "沙漠",
    temperature: 0.9,
    humidity: 0.1,
    surface_block: 4,       // 沙子
    under_surface_block: 4, // 沙子
    soil_thickness: 2,
    tree_density: 0.0,
    tree_type: TreeType::None,
    grass_density: 0.0,
    is_snowy: false,
    is_wet: false,
    is_sparse: true,
};

/// 热带草原群系 - 稀疏树木，草地
pub static BIOME_SAVANNA: Biome = Biome {
    id: BiomeId::Savanna,
    name: "热带草原",
    temperature: 0.7,
    humidity: 0.3,
    surface_block: 1,       // 草方块
    under_surface_block: 3, // 泥土
    soil_thickness: 3,
    tree_density: 0.15,
    tree_type: TreeType::Oak,
    grass_density: 0.6,
    is_snowy: false,
    is_wet: false,
    is_sparse: false,
};

/// 丛林群系 - 密林，藤蔓
pub static BIOME_JUNGLE: Biome = Biome {
    id: BiomeId::Jungle,
    name: "丛林",
    temperature: 0.8,
    humidity: 0.8,
    surface_block: 1,       // 草方块
    under_surface_block: 3, // 泥土
    soil_thickness: 4,
    tree_density: 0.8,
    tree_type: TreeType::JungleTree,
    grass_density: 0.4,
    is_snowy: false,
    is_wet: true,
    is_sparse: false,
};

/// 沼泽群系 - 低洼积水，树木
pub static BIOME_SWAMP: Biome = Biome {
    id: BiomeId::Swamp,
    name: "沼泽",
    temperature: 0.5,
    humidity: 0.9,
    surface_block: 3,       // 泥土
    under_surface_block: 3, // 泥土
    soil_thickness: 4,
    tree_density: 0.4,
    tree_type: TreeType::SwampTree,
    grass_density: 0.3,
    is_snowy: false,
    is_wet: true,
    is_sparse: false,
};

/// 平原群系 - 平坦草地，花朵
pub static BIOME_PLAINS: Biome = Biome {
    id: BiomeId::Plains,
    name: "平原",
    temperature: 0.4,
    humidity: 0.4,
    surface_block: 1,       // 草方块
    under_surface_block: 3, // 泥土
    soil_thickness: 4,
    tree_density: 0.05,
    tree_type: TreeType::Oak,
    grass_density: 0.8,
    is_snowy: false,
    is_wet: false,
    is_sparse: false,
};

/// 森林群系 - 树木茂密
pub static BIOME_FOREST: Biome = Biome {
    id: BiomeId::Forest,
    name: "森林",
    temperature: 0.3,
    humidity: 0.5,
    surface_block: 1,       // 草方块
    under_surface_block: 3, // 泥土
    soil_thickness: 4,
    tree_density: 0.5,
    tree_type: TreeType::Oak,
    grass_density: 0.5,
    is_snowy: false,
    is_wet: false,
    is_sparse: false,
};

/// 针叶林群系 - 雪地松林
pub static BIOME_TAIGA: Biome = Biome {
    id: BiomeId::Taiga,
    name: "针叶林",
    temperature: -0.1,
    humidity: 0.4,
    surface_block: 1,       // 草方块（低海拔）
    under_surface_block: 2, // 石头
    soil_thickness: 3,
    tree_density: 0.4,
    tree_type: TreeType::Pine,
    grass_density: 0.3,
    is_snowy: true,
    is_wet: false,
    is_sparse: false,
};

/// 冰原群系 - 纯白雪原
pub static BIOME_TUNDRA: Biome = Biome {
    id: BiomeId::Tundra,
    name: "冰原",
    temperature: -0.4,
    humidity: 0.1,
    surface_block: 9,       // 雪
    under_surface_block: 2, // 石头
    soil_thickness: 2,
    tree_density: 0.0,
    tree_type: TreeType::None,
    grass_density: 0.0,
    is_snowy: true,
    is_wet: false,
    is_sparse: true,
};

/// 高山群系 - 裸露岩石，雪峰
pub static BIOME_MOUNTAINS: Biome = Biome {
    id: BiomeId::Mountains,
    name: "高山",
    temperature: -0.2,
    humidity: 0.3,
    surface_block: 2,       // 石头
    under_surface_block: 2, // 石头
    soil_thickness: 0,
    tree_density: 0.0,
    tree_type: TreeType::None,
    grass_density: 0.0,
    is_snowy: true,
    is_wet: false,
    is_sparse: true,
};

// ============================================================================
// 群系查询函数
// ============================================================================

/// 获取群系数据
pub fn get_biome(biome_id: BiomeId) -> &'static Biome {
    match biome_id {
        BiomeId::Desert => &BIOME_DESERT,
        BiomeId::Savanna => &BIOME_SAVANNA,
        BiomeId::Jungle => &BIOME_JUNGLE,
        BiomeId::Swamp => &BIOME_SWAMP,
        BiomeId::Plains => &BIOME_PLAINS,
        BiomeId::Forest => &BIOME_FOREST,
        BiomeId::Taiga => &BIOME_TAIGA,
        BiomeId::Tundra => &BIOME_TUNDRA,
        BiomeId::Mountains => &BIOME_MOUNTAINS,
    }
}

/// 获取所有群系ID的列表
pub fn get_all_biome_ids() -> [BiomeId; 9] {
    [
        BiomeId::Desert,
        BiomeId::Savanna,
        BiomeId::Jungle,
        BiomeId::Swamp,
        BiomeId::Plains,
        BiomeId::Forest,
        BiomeId::Taiga,
        BiomeId::Tundra,
        BiomeId::Mountains,
    ]
}

/// 根据名称获取群系ID
pub fn get_biome_by_name(name: &str) -> Option<BiomeId> {
    match name {
        "沙漠" => Some(BiomeId::Desert),
        "热带草原" => Some(BiomeId::Savanna),
        "丛林" => Some(BiomeId::Jungle),
        "沼泽" => Some(BiomeId::Swamp),
        "平原" => Some(BiomeId::Plains),
        "森林" => Some(BiomeId::Forest),
        "针叶林" => Some(BiomeId::Taiga),
        "冰原" => Some(BiomeId::Tundra),
        "高山" => Some(BiomeId::Mountains),
        _ => None,
    }
}

// ============================================================================
// 群系选择算法
// ============================================================================

/// 根据温度和湿度选择群系
///
/// # 参数
/// * `temperature` - 温度噪声值 (-1.0 ~ 1.0)
/// * `humidity` - 湿度噪声值 (-1.0 ~ 1.0)
/// * `elevation` - 海拔高度
///
/// # 返回
/// 选定的 BiomeId
pub fn select_biome(temperature: f64, humidity: f64, elevation: f64) -> BiomeId {
    // 海拔修正：高山群系
    if elevation > 512.0 {
        return BiomeId::Mountains;
    }

    // 高海拔寒冷修正
    if elevation > 256.0 && temperature < 0.0 {
        return BiomeId::Taiga;
    }

    // 温度-湿度查表
    match (temperature, humidity) {
        // 极热 (temperature > 0.7)
        (t, _) if t > 0.7 => {
            if humidity < 0.2 {
                BiomeId::Desert
            } else if humidity > 0.5 {
                BiomeId::Savanna
            } else {
                BiomeId::Desert
            }
        }
        // 热 (temperature > 0.5)
        (t, _) if t > 0.5 => {
            if humidity < 0.3 {
                BiomeId::Savanna
            } else if humidity > 0.6 {
                BiomeId::Jungle
            } else {
                BiomeId::Plains
            }
        }
        // 温 (temperature > 0.2)
        (t, _) if t > 0.2 => {
            if humidity < 0.3 {
                BiomeId::Plains
            } else if humidity > 0.7 {
                BiomeId::Swamp
            } else {
                BiomeId::Forest
            }
        }
        // 寒 (temperature > 0.0)
        (t, _) if t > 0.0 => BiomeId::Taiga,
        // 极寒 (temperature <= 0.0)
        _ => BiomeId::Tundra,
    }
}

/// 获取群系在给定温度湿度下的权重
///
/// 用于群系边界过渡区的模糊混合
///
/// # 参数
/// * `biome_id` - 群系ID
/// * `temperature` - 当前温度
/// * `humidity` - 当前湿度
///
/// # 返回
/// 权重值 (0.0 ~ 1.0)，越高表示越匹配
pub fn get_biome_weight(biome_id: BiomeId, temperature: f64, humidity: f64) -> f64 {
    let biome = get_biome(biome_id);

    // 计算温度差异（使用高斯衰减）
    let temp_diff = (temperature - biome.temperature).abs();
    let temp_weight = (-temp_diff * 4.0).exp(); // sigma = 0.25

    // 计算湿度差异（使用高斯衰减）
    let humid_diff = (humidity - biome.humidity).abs();
    let humid_weight = (-humid_diff * 4.0).exp(); // sigma = 0.25

    // 组合权重
    (temp_weight + humid_weight) / 2.0
}
