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

/// 深海阈值：continentalness < 此值 → 深海
/// 降低此值减少深海比例（-0.7 → -0.85）
pub const DEEP_OCEAN_THRESHOLD: f64 = -0.85;

/// 海洋阈值：continentalness < 此值 → 海洋
/// 降低此值大幅减少海洋面积（-0.5 → -0.70），海洋/陆地比从 ~60/40 → ~25/75
pub const OCEAN_THRESHOLD: f64 = -0.70;

/// 内陆阈值：continentalness > 此值 → 内陆
pub const CONTINENT_THRESHOLD: f64 = 0.15;

// ══════════════════════════════════════════════════════════════════════════════
// 地形 Spline 曲线（参考 Tectonic terrain_spline 系统）
// ══════════════════════════════════════════════════════════════════════════════

/// 大陆性 → 基础高度偏移（相对于海平面）
///
/// 控制地形从深海到内陆的高度变化。
/// 大幅提升内陆高度变化范围，消除"一马平川"问题：
/// - 深海（-1.0）→ -90 格（新增最深点）
/// - 深海底（-0.7）→ -70 格
/// - 浅海底（-0.5）→ -25 格
/// - 海平面（-0.3）→ 0 格
/// - 海岸（-0.1）→ +20 格
/// - 内陆（0.2）→ +45 格（+30→+45，大幅提升！）
/// - 高原（0.5）→ +80 格（+45→+80，大幅提升！）
/// - 深内陆（1.0）→ +100 格（+50→+100）
///
/// 使用 spline 替代线性插值，产生更自然的大陆架过渡。
const CONTINENT_HEIGHT_SPLINE: Spline<8> = Spline::new([
    SplinePoint::auto(-1.0, -90.0), // 最深海底
    SplinePoint::auto(-0.7, -70.0), // 深海底
    SplinePoint::auto(-0.5, -25.0), // 浅海底
    SplinePoint::auto(-0.3,   0.0), // 海平面
    SplinePoint::auto(-0.1,  20.0), // 海岸平原
    SplinePoint::auto( 0.2,  45.0), // 内陆平原
    SplinePoint::auto( 0.5,  80.0), // 内陆高原
    SplinePoint::auto( 1.0, 100.0), // 深内陆
]);

/// Ridge → 山脊高度
///
/// 非线性映射：低 ridge 值产生缓坡，高 ridge 值产生陡峭山峰。
/// 大幅提升山峰高度（150→220），让山脉更壮观。
///
/// 负值（山谷）：线性凹陷
/// 正值（山峰）：S 形曲线，低值缓坡 → 高值陡峭
const RIDGE_HEIGHT_SPLINE: Spline<6> = Spline::new([
    SplinePoint::auto(-1.0, -25.0), // 深谷（-20→-25，更深的峡谷）
    SplinePoint::auto(-0.3,  -8.0), // 浅谷（-6→-8）
    SplinePoint::auto( 0.0,   0.0), // 平地
    SplinePoint::auto( 0.3,  30.0), // 缓坡（20→30）
    SplinePoint::auto( 0.6,  90.0), // 丘陵（60→90）
    SplinePoint::auto( 1.0, 220.0), // 高山峰（150→220）
]);

/// Erosion → 地形高度乘数
///
/// 控制侵蚀对山脊的削弱程度（仅在山地生效，平原不受影响）：
/// - 低侵蚀（<0）：无影响（乘数 1.0）
/// - 零侵蚀（0）：无影响（乘数 1.0）
/// - 轻微侵蚀（0.3）：几乎不影响（乘数 0.9）
/// - 中等侵蚀（0.6）：温和削弱（乘数 0.7）
/// - 强侵蚀（1.0）：削平山峰（乘数 0.55，原 0.4→0.55 减少过度削弱）
const EROSION_FACTOR_SPLINE: Spline<5> = Spline::new([
    SplinePoint::auto(-1.0, 1.0),  // 负侵蚀：无影响
    SplinePoint::auto( 0.0, 1.0),  // 零侵蚀：无影响
    SplinePoint::auto( 0.3, 0.9),  // 轻微侵蚀（0.85→0.9）
    SplinePoint::auto( 0.6, 0.7),  // 中等侵蚀（0.6→0.7）
    SplinePoint::auto( 1.0, 0.55), // 强侵蚀（0.4→0.55）
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
    /// # 算法（简化版，4 层叠加）
    ///
    /// 1. **大陆基底**：Spline 控制的 continentalness → 高度曲线
    /// 2. **山脊叠加**：Spline 控制的非线性山脊高度，
    ///    仅在山区 (r > 0.2) 受侵蚀和风化削弱，最小保底 0.35
    /// 3. **区域特征**：Club 高原/河谷 + Heart 起伏丘陵
    /// 4. **细节起伏**：高频噪声直接加到最终结果
    ///
    /// # 改进要点
    ///
    /// - 侵蚀/风化只削山顶，不削山谷和平原
    /// - 山脊保底因子防止过度削弱
    /// - 移除多余乘数链（flatten_factor），简化公式
    /// - 细节层直接叠加，不受侵蚀影响
    ///
    /// # 输出范围
    ///
    /// 约 -90（深海底）~ 320（最高山峰）
    #[inline]
    pub fn compute_base_height(&self) -> f64 {
        let c = self.continentalness;
        let e = self.erosion;
        let r = self.ridge;
        let t = self.temperature;
        let v = self.vegetation;

        // ── 1. 大陆基底（Spline 控制）──
        let continent_offset = CONTINENT_HEIGHT_SPLINE.evaluate(c);

        // 2^detail 非线性叠加：仅在山区渐进启用，平原保持线性（修复小山丘问题）
        // continent_offset ≤ 20（平原/海岸）：纯线性，0 抖动
        // continent_offset ≥ 50（高原/山脉）：完全非线性
        // 中间区间平滑过渡
        //
        // 原代码对所有陆地 (offset>0) 都启用非线性，导致平原抖动 ~34 格。
        // 现在平原 offset≈20 时完全线性，山脉 offset≈80 时才有非线性效果。
        let nonlinear = if continent_offset > 20.0 {
            let detail_exp = 2.0_f64.powf(self.detail);
            let nonlinear_scale = detail_exp * 0.5 + 0.5; // [0.75, 1.5]
            let blend = ((continent_offset - 20.0) / 30.0).clamp(0.0, 1.0);
            // blend 从 0→1，scale 从 1.0→nonlinear_scale
            continent_offset * (1.0 + blend * (nonlinear_scale - 1.0))
        } else {
            continent_offset // 平原/海岸：纯线性，零噪声抖动
        };

        let base_height = SEA_LEVEL as f64 + nonlinear;

        // ── 2. 山脊叠加 ──

        // 群系调制：寒冷地区山脉更陡峭
        let coldness = ((-0.12 - t) / 0.36).clamp(0.0, 1.0);
        let ridge_boost = 1.0 + coldness * 0.5; // 原 0.4 → 0.5

        let ridge_height = RIDGE_HEIGHT_SPLINE.evaluate(r) * ridge_boost;

        // 陆地过渡权重：海洋区域抑制山脊
        let land_weight = smoothstep(-0.3, 0.0, c);

        // 风化：仅在山脊区域 (r > 0.2) 生效，削弱锯齿状山峰
        // 风化区域 fade 从 0.7→1.0（原 0.5→1.0），减少过度削弱
        let weathering_val = self.weathering.abs();
        let weathering_factor = if r > 0.2 && weathering_val < 0.3 {
            let fade = weathering_val / 0.3;
            0.7 + fade * 0.3 // 0.7 ~ 1.0
        } else {
            1.0
        };

        // 侵蚀：仅在山脊区域 (r > 0.2) 生效，削平山顶
        let erosion_factor = if r > 0.2 {
            EROSION_FACTOR_SPLINE.evaluate(e)
        } else {
            1.0 // 平原/山谷不受侵蚀影响
        };

        // 组合衰减因子：最小保底 0.35，防止过度削弱
        let reduction = (erosion_factor * weathering_factor).max(0.35);

        let ridge_contribution = ridge_height * reduction * land_weight;

        // ── 3. 区域特征（Club 高原/河谷 + Heart 丘陵）──

        let region = self.region;
        let club_weight = smoothstep(0.0, -0.2, region);
        let heart_weight = smoothstep(0.0, 0.2, region);

        // Club：高原基底抬升 + 河谷切割（增强）
        let club_plateau = club_weight * 35.0; // 原 20.0 → 35.0
        let club_valley_deep = if r < 0.0 {
            club_weight * r * 25.0 // 原 15.0 → 25.0
        } else {
            0.0
        };

        // Heart：起伏丘陵（增强）
        let heart_hills = heart_weight * (self.detail.abs() * 25.0 - 12.5); // 原 15.0 → 25.0

        let region_effect = (club_plateau + club_valley_deep + heart_hills) * land_weight;

        // ── 4. 细节起伏（直接叠加，不受侵蚀/风化影响）──
        // 平原：极轻微起伏（2 格），山脉：更大起伏（12 格）
        // 修复平原小山丘问题：原代码对所有区域统一 12.0 振幅
        let detail_amp = if c > 0.15 {
            // 内陆：振幅随大陆性渐进增大（2→12 格）
            let inland_blend = ((c - 0.15) / 0.85).clamp(0.0, 1.0);
            2.0 + inland_blend * 10.0
        } else {
            2.0 // 海岸/近海：极轻微起伏
        };
        let detail = self.detail * detail_amp;

        // ── 5. 沙丘效果（热+干区域）──
        let dryness = (0.17 - v).max(0.0) / 0.55;
        let hotness = (t - (-0.12)).max(0.0) / 0.64;
        let dune_factor = dryness * hotness;

        let sand_dunes = if dune_factor > 0.1 {
            let dune_wave = (self.detail * 6.283).sin();
            dune_wave * 8.0 * dune_factor
        } else {
            0.0
        };

        // ── 最终高度 ──
        base_height + ridge_contribution + region_effect + detail + sand_dunes
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
            // 大陆性：低频 6 octaves，更大尺度的大陆形状
            // 频率 0.0006（原 0.0008）→ 大陆面积增大
            // lacunarity 2.5（原 2.0）→ 更丰富的层级结构
            // persistence 0.4（原 0.5）→ 更平滑的大陆边界
            continentalness: Fbm::<Simplex>::new(seed)
                .set_octaves(6)
                .set_frequency(0.0006)
                .set_lacunarity(2.5)
                .set_persistence(0.4),
            // 侵蚀：中频 4 octaves，更丰富的中频变化
            // lacunarity 2.2, persistence 0.55 → 更复杂的侵蚀细节
            erosion: Fbm::<Simplex>::new(seed.wrapping_add(1))
                .set_octaves(4)
                .set_frequency(0.0015)
                .set_lacunarity(2.2)
                .set_persistence(0.55),
            // 山脊：低频 RidgedMulti，更尖锐的山脊（低 persistence）
            // octaves 5（原 3）→ 更丰富的山脉层级
            // lacunarity 1.9, persistence 0.45 → 尖锐山脊线
            ridge: RidgedMulti::<Simplex>::new(seed.wrapping_add(2))
                .set_octaves(5)
                .set_frequency(0.0005)
                .set_lacunarity(1.9)
                .set_persistence(0.45),
            // 温度：极低频，大尺度气候带（保持）
            temperature: Fbm::<Simplex>::new(seed.wrapping_add(3))
                .set_octaves(4)
                .set_frequency(0.0006)
                .set_lacunarity(2.0)
                .set_persistence(0.5),
            // 植被：极低频，大尺度湿度分布（保持）
            vegetation: Fbm::<Simplex>::new(seed.wrapping_add(4))
                .set_octaves(4)
                .set_frequency(0.0006)
                .set_lacunarity(2.0)
                .set_persistence(0.5),
            // 细节：高频低振幅，更丰富的微起伏
            // octaves 4（原 3），frequency 0.01（原 0.02→0.01，周期 100 格，更缓坡）
            // lacunarity 2.2, persistence 0.55
            detail: Fbm::<Simplex>::new(seed.wrapping_add(5))
                .set_octaves(4)
                .set_frequency(0.01)
                .set_lacunarity(2.2)
                .set_persistence(0.55),
            // 区域选择器：保持极低频
            region: Fbm::<Simplex>::new(seed.wrapping_add(7))
                .set_octaves(4)
                .set_frequency(0.0003)
                .set_lacunarity(2.0)
                .set_persistence(0.5),
            // 洞穴：单个 3D Simplex 噪声（最基础实现）
            cave: Simplex::new(seed.wrapping_add(8)),
            // 风化：低频噪声，更平滑的风化区域边界
            // persistence 0.45 → 风化过度更柔和
            weathering: Fbm::<Simplex>::new(seed.wrapping_add(10))
                .set_octaves(3)
                .set_frequency(0.008)
                .set_lacunarity(2.3)
                .set_persistence(0.45),
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
