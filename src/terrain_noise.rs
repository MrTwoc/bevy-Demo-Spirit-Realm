//! 多层噪声地形生成系统
//!
//! 6+1 个独立噪声层驱动地形生成（参考 Tectonic 项目）：
//! - continentalness：大陆性（海洋 vs 陆地划分）
//! - erosion：侵蚀程度（削平山顶 / 切割河谷）
//! - ridge：山脊线（山脉生成）
//! - temperature：温度（S2 生物群系选择）
//! - vegetation：植被/湿度（S2 生物群系选择）
//! - region：区域选择器（4 区域地形特征）
//! - detail：高频细节（地表微起伏）
//!
//! # 设计原则
//!
//! - **确定性**：相同种子 + 相同坐标 = 相同结果
//! - **线程安全**：所有噪声函数通过 `OnceLock` 全局缓存
//! - **性能**：5 层噪声均为低频采样，单次 fill_terrain 增加约 2.5x 开销

use crate::spline::{Spline, SplinePoint};
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

/// 深海阈值：continentalness < 此值 → 深海（参考 Tectonic: -0.8 区间内）
pub const DEEP_OCEAN_THRESHOLD: f64 = -0.7;

/// 海洋阈值：continentalness < 此值 → 海洋（参考 Tectonic ocean_offset = -0.8）
/// 降低此值 → 海洋面积减小，陆地面积增大
pub const OCEAN_THRESHOLD: f64 = -0.5;

/// 内陆阈值：continentalness > 此值 → 内陆
pub const CONTINENT_THRESHOLD: f64 = 0.2;

// ══════════════════════════════════════════════════════════════════════════════
// 地形 Spline 曲线（参考 Tectonic terrain_spline 系统）
// ══════════════════════════════════════════════════════════════════════════════

/// 大陆性 → 基础高度偏移（相对于海平面）
///
/// 控制地形从深海到内陆的高度变化：
/// - 深海（-0.7）→ -80 格
/// - 浅海（-0.5）→ -30 格
/// - 海平面（-0.3）→ 0 格
/// - 海岸（-0.1）→ +15 格
/// - 内陆（0.2）→ +30 格
/// - 深内陆（0.5+）→ +45 格
///
/// 使用 spline 替代线性插值，产生更自然的大陆架过渡。
const CONTINENT_HEIGHT_SPLINE: Spline<7> = Spline::new([
    SplinePoint::auto(-0.7, -80.0), // 深海底
    SplinePoint::auto(-0.5, -30.0), // 浅海底
    SplinePoint::auto(-0.3,   0.0), // 海平面
    SplinePoint::auto(-0.1,  15.0), // 海岸平原
    SplinePoint::auto( 0.2,  30.0), // 内陆平原
    SplinePoint::auto( 0.5,  45.0), // 内陆高原
    SplinePoint::auto( 1.0,  50.0), // 深内陆
]);

/// Ridge → 山脊高度
///
/// 非线性映射：低 ridge 值产生缓坡，高 ridge 值产生陡峭山峰。
/// 参考 Tectonic 的 factor=5.6 陡度。
///
/// 负值（山谷）：线性凹陷
/// 正值（山峰）：S 形曲线，低值缓坡 → 高值陡峭
const RIDGE_HEIGHT_SPLINE: Spline<6> = Spline::new([
    SplinePoint::auto(-1.0, -20.0), // 深谷
    SplinePoint::auto(-0.3,  -6.0), // 浅谷
    SplinePoint::auto( 0.0,   0.0), // 平地
    SplinePoint::auto( 0.3,  20.0), // 缓坡
    SplinePoint::auto( 0.6,  60.0), // 丘陵
    SplinePoint::auto( 1.0, 150.0), // 高山峰
]);

/// Erosion → 地形高度乘数
///
/// 控制侵蚀对地形的削弱程度：
/// - 低侵蚀（<0）：无影响（乘数 1.0）
/// - 零侵蚀（0）：无影响（乘数 1.0）
/// - 中等侵蚀（0.5）：轻微削弱（乘数 0.75）
/// - 强侵蚀（1.0）：大幅削平（乘数 0.4）
///
/// 使用 spline 替代线性公式，低侵蚀区域保持原始地形。
const EROSION_FACTOR_SPLINE: Spline<5> = Spline::new([
    SplinePoint::auto(-1.0, 1.0),  // 负侵蚀：无影响
    SplinePoint::auto( 0.0, 1.0),  // 零侵蚀：无影响
    SplinePoint::auto( 0.3, 0.85), // 轻微侵蚀
    SplinePoint::auto( 0.6, 0.6),  // 中等侵蚀
    SplinePoint::auto( 1.0, 0.4),  // 强侵蚀：大幅削平
]);

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
    /// 区域选择器：极低频噪声，决定地形区域类型
    pub region: f64,
    /// 风化噪声：决定山脉哪里被风化侵蚀
    pub weathering: f64,
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
    /// 使用 **Spline 曲线** 精确控制地形高度变化（参考 Tectonic），
    /// 替代简单的线性公式，产生更自然的地形过渡。
    ///
    /// 地形由多层叠加：
    /// 1. **大陆基底**：Spline 控制的 continentalness → 高度曲线
    /// 2. **山脊叠加**：Spline 控制的非线性山脊高度（低值缓坡，高值陡峭）
    /// 3. **群系地形调制**：用温度/植被值调制地形特征
    /// 4. **侵蚀修饰**：Spline 控制的侵蚀因子
    /// 5. **海岸过渡**：smoothstep 权重控制山脊叠加，海洋区域抑制山脊
    ///
    /// # 输出范围
    ///
    /// 约 -80（深海底）~ 200（高山顶）
    #[inline]
    pub fn compute_base_height(&self) -> f64 {
        let c = self.continentalness;
        let e = self.erosion;
        let r = self.ridge;
        let t = self.temperature;
        let v = self.vegetation;

        // ── 大陆性高度（Spline 控制）──
        // 替代旧的线性公式，产生更自然的大陆架过渡
        let continent_offset = CONTINENT_HEIGHT_SPLINE.evaluate(c);

        // 2^detail 非线性叠加：让平原偶尔拔高，产生自然的丘陵/高峰
        // detail ∈ [-1, 1]，2^detail ∈ [0.5, 2.0]
        // 乘以 0.5 再加 0.5 → 映射到 [0.75, 1.5]，效果温和
        // 只在大陆区域叠加（continent_offset > 0），海洋不受影响
        let nonlinear = if continent_offset > 0.0 {
            let detail_exp = 2.0_f64.powf(self.detail);
            continent_offset * (detail_exp * 0.5 + 0.5)
        } else {
            continent_offset
        };

        let base_height = SEA_LEVEL as f64 + nonlinear;

        // ── 群系地形调制（用温度/植被值，不依赖 biome 标签）──

        // 沙丘效果：热+干 → 叠加正弦波状起伏
        let dryness = (0.17 - v).max(0.0) / 0.55;
        let hotness = (t - (-0.12)).max(0.0) / 0.64;
        let dune_factor = dryness * hotness;

        let sand_dunes = if dune_factor > 0.1 {
            let dune_wave = (self.detail * 6.283).sin();
            dune_wave * 8.0 * dune_factor
        } else {
            0.0
        };

        // 山脊增强：寒冷地区山脉更陡峭
        let coldness = ((-0.12 - t) / 0.36).clamp(0.0, 1.0);
        let ridge_boost = 1.0 + coldness * 0.4;

        // 平坦化：湿润地区地形更平缓
        let wetness = ((v - 0.07) / 0.26).clamp(0.0, 1.0);
        let flatten_factor = 1.0 - wetness * 0.25;

        // ── 区域地形特征（参考 Tectonic 4-region 系统）──
        // region < -0.1 → Club：高原+河谷（高基底，尖锐山脊）
        // region ~ 0    → Diamond：标准地形
        // region > 0.1  → Heart：起伏丘陵（低基底，柔和山脊）

        // 区域权重：smoothstep 平滑过渡
        let region = self.region;
        let club_weight = smoothstep(0.0, -0.2, region);    // 0.0 ~ 1.0（region < -0.2 时最大）
        let heart_weight = smoothstep(0.0, 0.2, region);    // 0.0 ~ 1.0（region > 0.2 时最大）

        // Club 效果：高原基底抬升 + 河谷切割
        // 高原区域基底抬升 20 格，ridge 负值更深（河谷）
        let club_plateau = club_weight * 20.0;
        let club_valley_deep = if r < 0.0 { club_weight * r * 15.0 } else { 0.0 };

        // Heart 效果：起伏丘陵（用 detail 噪声产生柔和波动）
        // 基底降低 10 格，叠加柔和起伏
        let heart_hills = heart_weight * (self.detail.abs() * 15.0 - 7.5);

        // ── 山脉风化（参考 Tectonic weathering 系统）──
        // 低频噪声产生大面积风化区域，山脊在风化区自然断裂
        // |weathering| < 0.3 → 风化区域（山脊削弱至 50%）
        // |weathering| > 0.3 → 完整山脊
        let weathering_val = self.weathering.abs();
        let weathering_factor = if weathering_val < 0.3 {
            let fade = weathering_val / 0.3;
            0.5 + fade * 0.5 // 0.5 ~ 1.0
        } else {
            1.0
        };

        // ── 山脊高度（Spline 控制，非线性 + 区域调制 + 风化）──
        let ridge_height = (RIDGE_HEIGHT_SPLINE.evaluate(r) * ridge_boost
            + club_valley_deep) * weathering_factor;

        // ── 侵蚀因子（Spline 控制）──
        // 替代旧的 1.0 - (e.max(0.0) * 0.5) 线性公式
        // 低侵蚀无影响，高侵蚀大幅削平
        let erosion_factor = EROSION_FACTOR_SPLINE.evaluate(e);

        // ── 细节层 ──
        let detail = self.detail * 5.0;

        // ── 最终陆地高度 ──
        // 山脊只在陆地区域叠加，海洋区域抑制
        // 使用 smoothstep 权重避免海岸线处的硬切边
        // land_weight: 0.0（深海）→ 1.0（内陆）
        let land_weight = smoothstep(-0.3, 0.0, c);

        let land_extra = (ridge_height + detail + sand_dunes)
            * erosion_factor * flatten_factor * land_weight;

        // 区域效果叠加（Club 高原抬升 + Heart 丘陵起伏）
        let region_effect = (club_plateau + heart_hills) * land_weight;

        base_height + land_extra + region_effect
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
    /// 区域选择器：极低频噪声，产生大尺度地形区域（~3000 格跨度）
    pub region: Fbm<Simplex>,
    /// 洞穴噪声：单个 3D Simplex 噪声
    pub cave: Simplex,
    /// 风化噪声：决定山脉哪里被风化侵蚀（山脊断裂效果）
    pub weathering: Fbm<Simplex>,
}

impl TerrainNoise {
    pub fn new(seed: u32) -> Self {
        Self {
            // 大陆性：低频 4 octaves（从 6 降至 4，减少 33% 计算量）
            continentalness: Fbm::<Simplex>::new(seed)
                .set_octaves(4)
                .set_frequency(0.0008)
                .set_lacunarity(2.0)
                .set_persistence(0.5),
            // 侵蚀：中频 4 octaves（从 6 降至 4）
            erosion: Fbm::<Simplex>::new(seed.wrapping_add(1))
                .set_octaves(4)
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
            // 频率 0.015 → 细节跨度约 67 格
            detail: Fbm::<Simplex>::new(seed.wrapping_add(5))
                .set_octaves(3)
                .set_frequency(0.015)
                .set_lacunarity(2.0)
                .set_persistence(0.5),
            // 区域选择器：极低频，产生大尺度地形区域（~3000 格跨度）
            // 参考 Tectonic region_selector firstOctave=-11
            region: Fbm::<Simplex>::new(seed.wrapping_add(7))
                .set_octaves(4)
                .set_frequency(0.0003)
                .set_lacunarity(2.0)
                .set_persistence(0.5),
            // 洞穴：单个 3D Simplex 噪声（最基础实现）
            cave: Simplex::new(seed.wrapping_add(8)),
            // 风化：低频噪声，产生大面积风化区域（~100 格跨度）
            weathering: Fbm::<Simplex>::new(seed.wrapping_add(10))
                .set_octaves(3)
                .set_frequency(0.008)
                .set_lacunarity(2.0)
                .set_persistence(0.5),
        }
    }

    /// 在世界坐标 (wx, wz) 处采样所有 7+1 个噪声层。
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
            region: self.region.get([wx, wz]),
            weathering: self.weathering.get([wx, wz]),
        }
    }

    /// 计算世界坐标 (wx, wy, wz) 处是否应该挖空（洞穴）。
    ///
    /// 使用两种洞穴类型：
    /// 1. **大型空腔**：Simplex 噪声 > 阈值 → 球状空腔
    /// 2. **蜿蜒隧道**：`abs(Simplex)` < 阈值 → 连接的脊线状隧道
    ///
    /// # 深度衰减
    ///
    /// - 地表以下 5 格内：不生成洞穴（避免地表穿透）
    /// - 深度 5~40 格：渐进增强
    /// - 深度 40+ 格：完全生效
    ///
    /// # 返回值
    ///
    /// `true` = 该位置应该挖空（空气）
    #[inline]
    pub fn is_cave(&self, wx: f64, wy: f64, wz: f64, depth_from_surface: i32) -> bool {
        // 地表附近不生成洞穴
        if depth_from_surface < 5 {
            return false;
        }

        // 深度衰减因子：5 格开始，40 格完全生效
        let depth_factor = ((depth_from_surface - 5) as f64 / 35.0).clamp(0.0, 1.0);

        // ── 大型空腔 ──
        // 反转逻辑：噪声 < 阈值 → 洞穴（低值区域大面积连通）
        let cheese_val = self.cave.get([wx * 0.012, wy * 0.015, wz * 0.012]);
        let cheese_threshold = -0.3 + depth_factor * 0.15; // 浅层 -0.3，深层 -0.15
        if cheese_val < cheese_threshold {
            return true;
        }

        // ── 蜿蜒隧道 ──
        // 反转逻辑：abs(noise) > 阈值 → 洞穴（脊线之间的区域连通）
        let spaghetti_val = self.cave.get([wx * 0.025, wy * 0.035, wz * 0.025]).abs();
        let spaghetti_threshold = 0.6 - depth_factor * 0.15; // 浅层 0.6，深层 0.45
        if spaghetti_val > spaghetti_threshold {
            return true;
        }

        false
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
/// 供 `tree_gen` 等模块使用。
///
/// 此函数是确定性的——相同的坐标总是返回相同的高度值。
#[inline]
pub fn compute_surface_height(world_x: f64, world_z: f64) -> i32 {
    let noise = get_terrain_noise();
    let sample = noise.sample_all(world_x, world_z);
    sample.compute_base_height() as i32
}
