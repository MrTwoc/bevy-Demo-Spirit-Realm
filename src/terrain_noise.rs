//! 多层噪声地形生成系统
//!
//! 5 个独立噪声层驱动地形生成（参考 Tectonic 项目）：
//! - continentalness：大陆性（海洋 vs 陆地划分）
//! - erosion：侵蚀程度（削平山顶 / 切割河谷）
//! - ridge：山脊线（山脉生成）
//! - temperature：温度（S2 生物群系选择）
//! - vegetation：植被/湿度（S2 生物群系选择）
//!
//! # 设计原则
//!
//! - **确定性**：相同种子 + 相同坐标 = 相同结果
//! - **线程安全**：所有噪声函数通过 `OnceLock` 全局缓存
//! - **性能**：5 层噪声均为低频采样，单次 fill_terrain 增加约 2.5x 开销

use noise::{Fbm, MultiFractal, NoiseFn, RidgedMulti, Simplex};
use std::sync::OnceLock;

// ══════════════════════════════════════════════════════════════════════════════
// 地形常量
// ══════════════════════════════════════════════════════════════════════════════

/// 地形噪声种子
pub const TERRAIN_SEED: u32 = 12345;

/// 海平面 Y 坐标（参考 Tectonic sea_level=63）
pub const SEA_LEVEL: i32 = 63;

/// 陆地基础高度（海平面以上 17 格）
pub const TERRAIN_BASE_HEIGHT: i32 = 80;

/// 泥土层厚度（地表以下）
pub const DIRT_LAYER_DEPTH: i32 = 4;

/// 最低生成高度（仿照 Minecraft min_y=-64）
pub const TERRAIN_MIN_Y: i32 = -64;

/// 最高生成高度（仿照 Minecraft max_y=320）
pub const TERRAIN_MAX_Y: i32 = 320;

// ══════════════════════════════════════════════════════════════════════════════
// 大陆性阈值
// ══════════════════════════════════════════════════════════════════════════════

/// 深海阈值：continentalness < 此值 → 深海
pub const DEEP_OCEAN_THRESHOLD: f64 = -0.5;

/// 海洋阈值：continentalness < 此值 → 海洋
pub const OCEAN_THRESHOLD: f64 = -0.3;

/// 内陆阈值：continentalness > 此值 → 内陆
pub const CONTINENT_THRESHOLD: f64 = 0.2;

// ══════════════════════════════════════════════════════════════════════════════
// 地形类型
// ══════════════════════════════════════════════════════════════════════════════

/// 由大陆性噪声决定的基础地形类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerrainType {
    DeepOcean,
    Ocean,
    Coast,
    Inland,
}

// ══════════════════════════════════════════════════════════════════════════════
// 数学工具
// ══════════════════════════════════════════════════════════════════════════════

/// 平滑阶跃函数（Hermite 插值）
///
/// 在 `[edge0, edge1]` 之间从 0.0 平滑过渡到 1.0。
/// 边界处导数为 0，避免硬切边。
///
/// ```text
/// smoothstep(-0.45, -0.15, x):
///   x < -0.45 → 0.0（深海）
///   x > -0.15 → 1.0（内陆）
///   中间 → S 形平滑曲线
/// ```
#[inline]
fn smoothstep(edge0: f64, edge1: f64, x: f64) -> f64 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

// ══════════════════════════════════════════════════════════════════════════════
// NoiseSample：单次采样结果
// ══════════════════════════════════════════════════════════════════════════════

/// 在世界坐标 (wx, wz) 处采样所有 5 个噪声层的结果。
///
/// 提供 `compute_base_height()` 和 `terrain_type()` 方法，
/// 避免在 fill_terrain 中重复采样噪声。
pub struct NoiseSample {
    pub continentalness: f64,
    pub erosion: f64,
    pub ridge: f64,
    pub temperature: f64,
    pub vegetation: f64,
    pub detail: f64,
}

impl NoiseSample {
    /// 根据大陆性值判断地形类型
    #[inline]
    pub fn terrain_type(&self) -> TerrainType {
        if self.continentalness < DEEP_OCEAN_THRESHOLD {
            TerrainType::DeepOcean
        } else if self.continentalness < OCEAN_THRESHOLD {
            TerrainType::Ocean
        } else if self.continentalness < CONTINENT_THRESHOLD {
            TerrainType::Coast
        } else {
            TerrainType::Inland
        }
    }

    /// 计算基础地形高度（参考 Tectonic sloped_cheese 管线）
    ///
    /// # 算法
    ///
    /// 使用 **smoothstep** 在海洋和陆地之间创建平滑过渡带（海岸线），
    /// 避免 continentalness 阈值处的硬切边。
    ///
    /// 地形由多层叠加：
    /// 1. **大陆基底**：由 continentalness 决定的缓慢起伏（宽广的高原/平原）
    /// 2. **山脊叠加**：RidgedMulti 噪声产生的山脊（宽厚、圆润）
    /// 3. **群系地形调制**：用温度/植被值调制地形特征（无需 biome 标签）
    ///    - 热+干 → 沙丘起伏
    ///    - 冷 → 山脊增强
    ///    - 湿 → 地形更平坦
    /// 4. **侵蚀修饰**：削平山顶，切割河谷
    ///
    /// # 输出范围
    ///
    /// 约 -17（深海底）~ 230（高山顶）
    #[inline]
    pub fn compute_base_height(&self) -> f64 {
        let c = self.continentalness;
        let e = self.erosion;
        let r = self.ridge;
        let t = self.temperature;
        let v = self.vegetation;

        // ── 海洋深度 ──
        let ocean_t = ((c - DEEP_OCEAN_THRESHOLD) / (OCEAN_THRESHOLD - DEEP_OCEAN_THRESHOLD))
            .clamp(0.0, 1.0);
        let ocean_height = SEA_LEVEL as f64 - 80.0 * (1.0 - ocean_t);

        // ── 大陆基底 ──
        let inlandness = ((c - CONTINENT_THRESHOLD) / (1.0 - CONTINENT_THRESHOLD))
            .clamp(0.0, 1.0);
        let continental_base = SEA_LEVEL as f64 + 20.0 + inlandness * 40.0; // 83 ~ 123

        // ── 群系地形调制（用温度/植被值，不依赖 biome 标签）──

        // 沙丘效果：热+干 → 叠加正弦波状起伏
        // dryness: 0.0（湿润）→ 1.0（干旱）
        // hotness: 0.0（寒冷）→ 1.0（炎热）
        let dryness = (0.17 - v).max(0.0) / 0.55; // vegetation < 0.17 → 干燥
        let hotness = (t - (-0.12)).max(0.0) / 0.64; // temperature > -0.12 → 温暖
        let dune_factor = dryness * hotness; // 0.0 ~ 1.0

        let sand_dunes = if dune_factor > 0.1 {
            // 用 detail 噪声模拟沙丘（频率已足够高）
            // 正弦调制让沙丘有波浪感
            let dune_wave = (self.detail * 6.283).sin(); // [-1, 1]
            dune_wave * 8.0 * dune_factor // 最高 ±8 格沙丘
        } else {
            0.0
        };

        // 山脊增强：寒冷地区山脉更陡峭
        // coldness: 0.0（温暖）→ 1.0（极寒）
        let coldness = ((-0.12 - t) / 0.36).clamp(0.0, 1.0);
        let ridge_boost = 1.0 + coldness * 0.4; // 1.0 ~ 1.4 倍山脊振幅

        // 平坦化：湿润地区地形更平缓
        // wetness: 0.0（干旱）→ 1.0（湿润）
        let wetness = ((v - 0.07) / 0.26).clamp(0.0, 1.0);
        let flatten_factor = 1.0 - wetness * 0.25; // 1.0 ~ 0.75 倍

        // ── 山脊叠加 ──
        let ridge_height = if r > 0.0 {
            r * 120.0 * ridge_boost
        } else {
            r * 20.0
        };

        // ── 侵蚀调制 ──
        let erosion_factor = 1.0 - (e.max(0.0) * 0.5);

        // ── 细节层 ──
        let detail = self.detail * 5.0;

        // ── 最终陆地高度 ──
        let land_height = (continental_base + ridge_height + detail + sand_dunes)
            * erosion_factor * flatten_factor;

        // ── 海岸过渡 ──
        const COAST_START: f64 = OCEAN_THRESHOLD - 0.15;
        const COAST_END: f64 = OCEAN_THRESHOLD + 0.15;
        let land_factor = smoothstep(COAST_START, COAST_END, c);

        // 海岸细节
        let coastal_detail = if land_factor > 0.05 && land_factor < 0.95 {
            let coast_proximity = (land_factor * (1.0 - land_factor) * 4.0).min(1.0);
            e * 8.0 * coast_proximity
        } else {
            0.0
        };

        ocean_height * (1.0 - land_factor) + land_height * land_factor + coastal_detail
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// TerrainNoise：5 层噪声管理器
// ══════════════════════════════════════════════════════════════════════════════

/// 5+1 层噪声管理器，持有所有噪声生成器。
///
/// 5 个低频层决定大尺度地形结构，1 个高频细节层添加地表微起伏。
/// 通过 `OnceLock` 全局缓存，线程安全，只初始化一次。
pub struct TerrainNoise {
    pub continentalness: Fbm<Simplex>,
    pub erosion: Fbm<Simplex>,
    pub ridge: RidgedMulti<Simplex>,
    pub temperature: Fbm<Simplex>,
    pub vegetation: Fbm<Simplex>,
    /// 高频细节层：小丘陵/凹陷，让地表不那么"光滑"
    pub detail: Fbm<Simplex>,
}

impl TerrainNoise {
    pub fn new(seed: u32) -> Self {
        Self {
            // 大陆性：低频 6 octaves，决定海洋/陆地的大尺度划分
            continentalness: Fbm::<Simplex>::new(seed)
                .set_octaves(6)
                .set_frequency(0.0008)
                .set_lacunarity(2.0)
                .set_persistence(0.5),
            // 侵蚀：中频 6 octaves，决定地形侵蚀程度
            erosion: Fbm::<Simplex>::new(seed.wrapping_add(1))
                .set_octaves(6)
                .set_frequency(0.0015)
                .set_lacunarity(2.0)
                .set_persistence(0.5),
            // 山脊：低频 ridged 噪声，产生宽厚的山脉而非薄脊线
            // 频率 0.0004 → 山脉跨度约 2500 格（周期），octaves 3 让山形更柔和
            ridge: RidgedMulti::<Simplex>::new(seed.wrapping_add(2))
                .set_octaves(3)
                .set_frequency(0.0004)
                .set_lacunarity(2.0)
                .set_persistence(0.5),
            // 温度：极低频，大尺度气候带
            temperature: Fbm::<Simplex>::new(seed.wrapping_add(3))
                .set_octaves(4)
                .set_frequency(0.0006)
                .set_lacunarity(2.0)
                .set_persistence(0.5),
            // 植被：极低频，大尺度湿度分布
            vegetation: Fbm::<Simplex>::new(seed.wrapping_add(4))
                .set_octaves(4)
                .set_frequency(0.0006)
                .set_lacunarity(2.0)
                .set_persistence(0.5),
            // 细节：高频低振幅，为山坡添加微起伏，也用于沙丘波浪效果
            // 频率 0.015 → 细节跨度约 67 格，振幅 ±5 格
            detail: Fbm::<Simplex>::new(seed.wrapping_add(5))
                .set_octaves(3)
                .set_frequency(0.015)
                .set_lacunarity(2.0)
                .set_persistence(0.5),
        }
    }

    /// 在世界坐标 (wx, wz) 处采样所有 5 个噪声层。
    ///
    /// 一次调用获取全部噪声值，避免重复采样。
    #[inline]
    pub fn sample_all(&self, wx: f64, wz: f64) -> NoiseSample {
        NoiseSample {
            continentalness: self.continentalness.get([wx, wz]),
            erosion: self.erosion.get([wx, wz]),
            ridge: self.ridge.get([wx, wz]),
            temperature: self.temperature.get([wx, wz]),
            vegetation: self.vegetation.get([wx, wz]),
            detail: self.detail.get([wx, wz]),
        }
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// 全局缓存
// ══════════════════════════════════════════════════════════════════════════════

static TERRAIN_NOISE: OnceLock<TerrainNoise> = OnceLock::new();

/// 获取全局缓存的 5 层噪声管理器（线程安全，只初始化一次）
pub fn get_terrain_noise() -> &'static TerrainNoise {
    TERRAIN_NOISE.get_or_init(|| TerrainNoise::new(TERRAIN_SEED))
}

// ══════════════════════════════════════════════════════════════════════════════
// 便捷函数
// ══════════════════════════════════════════════════════════════════════════════

/// 计算世界坐标 (world_x, world_z) 处的地表高度。
///
/// 这是一个便捷函数，内部调用 `get_terrain_noise().sample_all().compute_base_height()`。
/// 供 `terrain_bridge`、`tree_gen` 等模块使用。
///
/// 此函数是确定性的——相同的坐标总是返回相同的高度值。
#[inline]
pub fn compute_surface_height(world_x: f64, world_z: f64) -> i32 {
    let noise = get_terrain_noise();
    let sample = noise.sample_all(world_x, world_z);
    sample.compute_base_height() as i32
}
