//! 统一面方向定义和四边形生成。
//!
//! 消除 chunk.rs / async_mesh.rs / lod.rs 三处 Face 枚举和 face_quad 函数的重复。

/// 面方向枚举
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Face {
    Top,
    Bottom,
    Right,
    Left,
    Front,
    Back,
}

impl Face {
    /// 面名称（用于 UV 查找：top / bottom / side）
    pub const fn to_face_name(self) -> &'static str {
        match self {
            Face::Top => "top",
            Face::Bottom => "bottom",
            _ => "side",
        }
    }

    /// UV 索引（0=top, 1=bottom, 2=side）
    pub const fn face_index(self) -> usize {
        match self {
            Face::Top => 0,
            Face::Bottom => 1,
            _ => 2,
        }
    }
}

/// 6 个面方向的定义：(Face, 偏移量, UV 索引)。
///
/// 顺序：+X, -X, +Y, -Y, +Z, -Z（与 NEIGHBOR_OFFSETS 一致）。
pub const FACES: [(Face, [i32; 3], usize); 6] = [
    (Face::Right, [1, 0, 0], 2),
    (Face::Left, [-1, 0, 0], 2),
    (Face::Top, [0, 1, 0], 0),
    (Face::Bottom, [0, -1, 0], 1),
    (Face::Front, [0, 0, 1], 2),
    (Face::Back, [0, 0, -1], 2),
];

/// 6 个方向的邻居区块偏移量。
///
/// 顺序与 `FACES` 一致：[+X, -X, +Y, -Y, +Z, -Z]。
pub const NEIGHBOR_OFFSETS: [(i32, i32, i32); 6] = [
    (1, 0, 0),
    (-1, 0, 0),
    (0, 1, 0),
    (0, -1, 0),
    (0, 0, 1),
    (0, 0, -1),
];

/// 生成单个面的四边形顶点（4 个顶点 + UV + 法线）。
///
/// 顶点顺序：左下 → 右下 → 右上 → 左上（逆时针，从面外侧观察）。
/// 索引顺序：[0,2,1], [0,3,2] 构成两个三角形。
pub fn face_quad(
    x: usize,
    y: usize,
    z: usize,
    face: Face,
    uv: (f32, f32, f32, f32),
) -> ([[f32; 3]; 4], [[f32; 2]; 4], [f32; 3]) {
    let x_f = x as f32;
    let y_f = y as f32;
    let z_f = z as f32;

    let (verts, normal) = match face {
        Face::Top => (
            [
                [x_f, y_f + 1.0, z_f],
                [x_f + 1.0, y_f + 1.0, z_f],
                [x_f + 1.0, y_f + 1.0, z_f + 1.0],
                [x_f, y_f + 1.0, z_f + 1.0],
            ],
            [0.0, 1.0, 0.0],
        ),
        Face::Bottom => (
            [
                [x_f, y_f, z_f + 1.0],
                [x_f + 1.0, y_f, z_f + 1.0],
                [x_f + 1.0, y_f, z_f],
                [x_f, y_f, z_f],
            ],
            [0.0, -1.0, 0.0],
        ),
        Face::Right => (
            [
                [x_f + 1.0, y_f, z_f],
                [x_f + 1.0, y_f, z_f + 1.0],
                [x_f + 1.0, y_f + 1.0, z_f + 1.0],
                [x_f + 1.0, y_f + 1.0, z_f],
            ],
            [1.0, 0.0, 0.0],
        ),
        Face::Left => (
            [
                [x_f, y_f, z_f + 1.0],
                [x_f, y_f, z_f],
                [x_f, y_f + 1.0, z_f],
                [x_f, y_f + 1.0, z_f + 1.0],
            ],
            [-1.0, 0.0, 0.0],
        ),
        Face::Front => (
            [
                [x_f + 1.0, y_f, z_f + 1.0],
                [x_f, y_f, z_f + 1.0],
                [x_f, y_f + 1.0, z_f + 1.0],
                [x_f + 1.0, y_f + 1.0, z_f + 1.0],
            ],
            [0.0, 0.0, 1.0],
        ),
        Face::Back => (
            [
                [x_f, y_f, z_f],
                [x_f + 1.0, y_f, z_f],
                [x_f + 1.0, y_f + 1.0, z_f],
                [x_f, y_f + 1.0, z_f],
            ],
            [0.0, 0.0, -1.0],
        ),
    };

    let face_uvs = [
        [uv.0, uv.3],
        [uv.1, uv.3],
        [uv.1, uv.2],
        [uv.0, uv.2],
    ];

    (verts, face_uvs, normal)
}

/// LOD 面四边形生成（顶点归一化版）。
///
/// 顶点坐标除以 `step_f`，使模型空间保持 1x1。
/// 世界空间放大由 `Transform::scale(Vec3::splat(step_f))` 通过 GPU 矩阵完成。
pub fn face_quad_lod(
    x: usize,
    y: usize,
    z: usize,
    face: Face,
    uv: (f32, f32, f32, f32),
    step_f: f32,
) -> ([[f32; 3]; 4], [[f32; 2]; 4], [f32; 3]) {
    let x_f = x as f32 / step_f;
    let y_f = y as f32 / step_f;
    let z_f = z as f32 / step_f;

    let (verts, normal) = match face {
        Face::Top => (
            [
                [x_f, y_f + 1.0, z_f],
                [x_f + 1.0, y_f + 1.0, z_f],
                [x_f + 1.0, y_f + 1.0, z_f + 1.0],
                [x_f, y_f + 1.0, z_f + 1.0],
            ],
            [0.0, 1.0, 0.0],
        ),
        Face::Bottom => (
            [
                [x_f, y_f, z_f + 1.0],
                [x_f + 1.0, y_f, z_f + 1.0],
                [x_f + 1.0, y_f, z_f],
                [x_f, y_f, z_f],
            ],
            [0.0, -1.0, 0.0],
        ),
        Face::Right => (
            [
                [x_f + 1.0, y_f, z_f],
                [x_f + 1.0, y_f, z_f + 1.0],
                [x_f + 1.0, y_f + 1.0, z_f + 1.0],
                [x_f + 1.0, y_f + 1.0, z_f],
            ],
            [1.0, 0.0, 0.0],
        ),
        Face::Left => (
            [
                [x_f, y_f, z_f + 1.0],
                [x_f, y_f, z_f],
                [x_f, y_f + 1.0, z_f],
                [x_f, y_f + 1.0, z_f + 1.0],
            ],
            [-1.0, 0.0, 0.0],
        ),
        Face::Front => (
            [
                [x_f + 1.0, y_f, z_f + 1.0],
                [x_f, y_f, z_f + 1.0],
                [x_f, y_f + 1.0, z_f + 1.0],
                [x_f + 1.0, y_f + 1.0, z_f + 1.0],
            ],
            [0.0, 0.0, 1.0],
        ),
        Face::Back => (
            [
                [x_f, y_f, z_f],
                [x_f + 1.0, y_f, z_f],
                [x_f + 1.0, y_f + 1.0, z_f],
                [x_f, y_f + 1.0, z_f],
            ],
            [0.0, 0.0, -1.0],
        ),
    };

    let face_uvs = [
        [uv.0, uv.3],
        [uv.1, uv.3],
        [uv.1, uv.2],
        [uv.0, uv.2],
    ];

    (verts, face_uvs, normal)
}
