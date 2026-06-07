//! 轻量级 Spline 插值系统
//!
//! 使用单调三次 Hermite 插值（Fritsch-Carlson 方法），
//! 保证通过所有控制点，单调区间不产生过冲，C1 连续。
//!
//! # 设计目标
//!
//! - **零堆分配**：编译时固定大小（const 泛型）
//! - **高性能**：二分查找 + 三次插值，~10 周期/求值
//! - **确定性**：纯函数，相同输入 = 相同输出
//! - **易调参**：控制点数组，修改 x/y/d 即可调整地形曲线

// ══════════════════════════════════════════════════════════════════════════════
// SplinePoint：控制点
// ══════════════════════════════════════════════════════════════════════════════

/// Spline 控制点
///
/// - `x`：输入值（如 continentalness、ridge 等）
/// - `y`：输出值（如高度、因子等）
/// - `d`：斜率（导数）。设为 `-1.0` 表示自动计算单调斜率
#[derive(Clone, Copy)]
pub struct SplinePoint {
    pub x: f64,
    pub y: f64,
    pub d: f64,
}

impl SplinePoint {
    /// 创建控制点，斜率自动计算
    #[inline]
    pub const fn auto(x: f64, y: f64) -> Self {
        Self { x, y, d: -1.0 }
    }

    /// 创建控制点，指定斜率
    #[inline]
    pub const fn with_derivative(x: f64, y: f64, d: f64) -> Self {
        Self { x, y, d }
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// Spline：1D 插值曲线
// ══════════════════════════════════════════════════════════════════════════════

/// 1D 单调三次 Hermite Spline
///
/// `N` 为控制点数量，编译时固定。
///
/// # 示例
///
/// ```rust
/// const HEIGHT_SPLINE: Spline<4> = Spline::new([
///     SplinePoint::auto(-1.0, -80.0),  // 深海
///     SplinePoint::auto(-0.5, -40.0),  // 浅海
///     SplinePoint::auto( 0.0,   0.0),  // 海平面
///     SplinePoint::auto( 1.0,  50.0),  // 深内陆
/// ]);
///
/// assert_eq!(HEIGHT_SPLINE.evaluate(-0.75), -60.0); // 深海和浅海之间
/// ```
pub struct Spline<const N: usize> {
    points: [SplinePoint; N],
}

impl<const N: usize> Spline<N> {
    /// 创建 Spline（编译时可用）
    ///
    /// 控制点必须按 x 值升序排列。
    /// 斜率为 -1.0 的点会自动计算单调斜率。
    pub const fn new(mut points: [SplinePoint; N]) -> Self {
        // 自动计算斜率（单调 Fritsch-Carlson 方法）
        // 对每个 d == -1.0 的点，使用相邻点的差分作为初始斜率，
        // 然后钳位到单调范围。
        let mut i = 0;
        while i < N {
            if points[i].d == -1.0 {
                points[i].d = Self::auto_derivative(&points, i);
            }
            i += 1;
        }
        Self { points }
    }

    /// 自动计算第 i 个控制点的单调斜率
    ///
    /// 使用 Fritsch-Carlson 方法：
    /// 1. 计算相邻区间的差分斜率
    /// 2. 如果相邻斜率同号，使用调和平均
    /// 3. 否则设为 0（避免过冲）
    const fn auto_derivative(points: &[SplinePoint], i: usize) -> f64 {
        if N < 2 {
            return 0.0;
        }

        // 边界点：使用单侧差分
        if i == 0 {
            let dx = points[1].x - points[0].x;
            if dx.abs() < 1e-12 {
                return 0.0;
            }
            return (points[1].y - points[0].y) / dx;
        }
        if i == N - 1 {
            let dx = points[N - 1].x - points[N - 2].x;
            if dx.abs() < 1e-12 {
                return 0.0;
            }
            return (points[N - 1].y - points[N - 2].y) / dx;
        }

        // 内部点：使用相邻差分的调和平均
        let dx_prev = points[i].x - points[i - 1].x;
        let dx_next = points[i + 1].x - points[i].x;

        if dx_prev.abs() < 1e-12 || dx_next.abs() < 1e-12 {
            return 0.0;
        }

        let slope_prev = (points[i].y - points[i - 1].y) / dx_prev;
        let slope_next = (points[i + 1].y - points[i].y) / dx_next;

        // 如果相邻斜率异号，设为 0（避免过冲）
        if slope_prev * slope_next < 0.0 {
            return 0.0;
        }

        // 调和平均：2 / (1/s1 + 1/s2) = 2*s1*s2 / (s1+s2)
        let sum = slope_prev + slope_next;
        if sum.abs() < 1e-12 {
            return 0.0;
        }
        2.0 * slope_prev * slope_next / sum
    }

    /// 求值：输入 x，输出插值后的 y
    ///
    /// - x 超出范围时钳位到最近的控制点值
    /// - 使用二分查找定位区间，然后三次 Hermite 插值
    #[inline]
    pub fn evaluate(&self, x: f64) -> f64 {
        // 钳位到有效范围
        if x <= self.points[0].x {
            return self.points[0].y;
        }
        if x >= self.points[N - 1].x {
            return self.points[N - 1].y;
        }

        // 二分查找所在区间
        let mut lo = 0usize;
        let mut hi = N - 1;
        while hi - lo > 1 {
            let mid = (lo + hi) / 2;
            if self.points[mid].x <= x {
                lo = mid;
            } else {
                hi = mid;
            }
        }

        // 三次 Hermite 插值
        let p0 = &self.points[lo];
        let p1 = &self.points[hi];
        let dx = p1.x - p0.x;

        if dx.abs() < 1e-12 {
            return p0.y;
        }

        let t = (x - p0.x) / dx; // 归一化参数 [0, 1]
        let t2 = t * t;
        let t3 = t2 * t;

        // Hermite 基函数
        let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
        let h10 = t3 - 2.0 * t2 + t;
        let h01 = -2.0 * t3 + 3.0 * t2;
        let h11 = t3 - t2;

        h00 * p0.y + h10 * dx * p0.d + h01 * p1.y + h11 * dx * p1.d
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// 测试
// ══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_linear_spline() {
        // 线性插值应该精确
        const S: Spline<2> = Spline::new([
            SplinePoint::auto(0.0, 0.0),
            SplinePoint::auto(1.0, 100.0),
        ]);
        assert!((S.evaluate(0.0) - 0.0).abs() < 0.01);
        assert!((S.evaluate(0.5) - 50.0).abs() < 0.01);
        assert!((S.evaluate(1.0) - 100.0).abs() < 0.01);
    }

    #[test]
    fn test_clamping() {
        const S: Spline<3> = Spline::new([
            SplinePoint::auto(-1.0, -10.0),
            SplinePoint::auto(0.0, 0.0),
            SplinePoint::auto(1.0, 10.0),
        ]);
        assert!((S.evaluate(-2.0) - (-10.0)).abs() < 0.01); // 钳位到左端
        assert!((S.evaluate(2.0) - 10.0).abs() < 0.01);     // 钳位到右端
    }

    #[test]
    fn test_passes_through_points() {
        const S: Spline<4> = Spline::new([
            SplinePoint::auto(-1.0, 0.0),
            SplinePoint::auto(-0.3, 5.0),
            SplinePoint::auto(0.3, 8.0),
            SplinePoint::auto(1.0, 20.0),
        ]);
        assert!((S.evaluate(-1.0) - 0.0).abs() < 0.01);
        assert!((S.evaluate(-0.3) - 5.0).abs() < 0.01);
        assert!((S.evaluate(0.3) - 8.0).abs() < 0.01);
        assert!((S.evaluate(1.0) - 20.0).abs() < 0.01);
    }
}
