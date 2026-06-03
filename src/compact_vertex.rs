//! 紧凑顶点格式 (CompactVertex)
//!
//! 借鉴 Voxy 的 8 字节顶点格式，用于减少 GPU 带宽和内存占用。
//!
//! # 数据布局 (8 字节)
//!
//! ```text
//! data0 (u32):
//!   bits 0-9:   x (10 bits, 0-1023)
//!   bits 10-19: y (10 bits, 0-1023)
//!   bits 20-29: z (10 bits, 0-1023)
//!   bits 30-31: 法线方向 (2 bits, 6 个方向)
//!
//! data1 (u32):
//!   bits 0-7:   u (8 bits, 0-255)
//!   bits 8-15:  v (8 bits, 0-255)
//!   bits 16-31: 模型 ID + 光照 (16 bits)
//! ```
//!
//! # 压缩比
//!
//! | 格式 | 大小 | 压缩比 |
//! |------|------|--------|
//! | 标准格式 | 32 字节 | 1x |
//! | 紧凑格式 | 8 字节 | 4x |

use bytemuck::{Pod, Zeroable};

/// 法线方向枚举 (6 个方向)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum NormalDirection {
    Right = 0,   // +X
    Left = 1,    // -X
    Up = 2,      // +Y
    Down = 3,    // -Y
    Front = 4,   // +Z
    Back = 5,    // -Z
}

impl NormalDirection {
    /// 从法线向量创建方向
    pub fn from_normal(nx: f32, ny: f32, nz: f32) -> Self {
        let abs_nx = nx.abs();
        let abs_ny = ny.abs();
        let abs_nz = nz.abs();

        if abs_nx > abs_ny && abs_nx > abs_nz {
            if nx > 0.0 { NormalDirection::Right } else { NormalDirection::Left }
        } else if abs_ny > abs_nz {
            if ny > 0.0 { NormalDirection::Up } else { NormalDirection::Down }
        } else {
            if nz > 0.0 { NormalDirection::Front } else { NormalDirection::Back }
        }
    }

    /// 获取法线向量
    pub fn to_normal(&self) -> [f32; 3] {
        match self {
            NormalDirection::Right => [1.0, 0.0, 0.0],
            NormalDirection::Left => [-1.0, 0.0, 0.0],
            NormalDirection::Up => [0.0, 1.0, 0.0],
            NormalDirection::Down => [0.0, -1.0, 0.0],
            NormalDirection::Front => [0.0, 0.0, 1.0],
            NormalDirection::Back => [0.0, 0.0, -1.0],
        }
    }
}

/// 紧凑顶点格式 (8 字节)
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct CompactVertex {
    /// 位置 (3×10bit) + 法线方向 (2bit)
    pub data0: u32,
    /// UV (2×8bit) + 模型 ID + 光照 (16bit)
    pub data1: u32,
}

impl CompactVertex {
    /// 创建新的紧凑顶点
    ///
    /// # Arguments
    /// * `x` - X 坐标 (0-1023)
    /// * `y` - Y 坐标 (0-1023)
    /// * `z` - Z 坐标 (0-1023)
    /// * `normal` - 法线方向
    /// * `u` - U 纹理坐标 (0-255)
    /// * `v` - V 纹理坐标 (0-255)
    /// * `model_id` - 模型 ID (0-65535)
    pub fn new(
        x: u16,
        y: u16,
        z: u16,
        normal: NormalDirection,
        u: u8,
        v: u8,
        model_id: u16,
    ) -> Self {
        let data0 = (x as u32 & 0x3FF)
            | ((y as u32 & 0x3FF) << 10)
            | ((z as u32 & 0x3FF) << 20)
            | ((normal as u32 & 0x3) << 30);

        let data1 = (u as u32)
            | ((v as u32) << 8)
            | ((model_id as u32) << 16);

        Self { data0, data1 }
    }

    /// 从标准格式创建紧凑顶点
    pub fn from_standard(
        position: [f32; 3],
        normal: [f32; 3],
        uv: [f32; 2],
        model_id: u16,
    ) -> Self {
        // 将浮点位置转换为整数 (假设每个体素是 1.0 单位)
        let x = (position[0] * 32.0) as u16;
        let y = (position[1] * 32.0) as u16;
        let z = (position[2] * 32.0) as u16;

        // 将法线转换为方向
        let normal_dir = NormalDirection::from_normal(normal[0], normal[1], normal[2]);

        // 将 UV 转换为 0-255 范围
        let u = (uv[0] * 255.0) as u8;
        let v = (uv[1] * 255.0) as u8;

        Self::new(x, y, z, normal_dir, u, v, model_id)
    }

    /// 获取 X 坐标
    #[inline]
    pub fn x(&self) -> u16 {
        (self.data0 & 0x3FF) as u16
    }

    /// 获取 Y 坐标
    #[inline]
    pub fn y(&self) -> u16 {
        ((self.data0 >> 10) & 0x3FF) as u16
    }

    /// 获取 Z 坐标
    #[inline]
    pub fn z(&self) -> u16 {
        ((self.data0 >> 20) & 0x3FF) as u16
    }

    /// 获取法线方向
    #[inline]
    pub fn normal_direction(&self) -> NormalDirection {
        let dir = (self.data0 >> 30) & 0x3;
        match dir {
            0 => NormalDirection::Right,
            1 => NormalDirection::Left,
            2 => NormalDirection::Up,
            3 => NormalDirection::Down,
            _ => unreachable!(),
        }
    }

    /// 获取法线向量
    #[inline]
    pub fn normal(&self) -> [f32; 3] {
        self.normal_direction().to_normal()
    }

    /// 获取 U 纹理坐标 (0.0-1.0)
    #[inline]
    pub fn u(&self) -> f32 {
        (self.data1 & 0xFF) as f32 / 255.0
    }

    /// 获取 V 纹理坐标 (0.0-1.0)
    #[inline]
    pub fn v(&self) -> f32 {
        ((self.data1 >> 8) & 0xFF) as f32 / 255.0
    }

    /// 获取模型 ID
    #[inline]
    pub fn model_id(&self) -> u16 {
        ((self.data1 >> 16) & 0xFFFF) as u16
    }

    /// 转换为标准格式
    pub fn to_standard(&self) -> ([f32; 3], [f32; 3], [f32; 2]) {
        let position = [
            self.x() as f32 / 32.0,
            self.y() as f32 / 32.0,
            self.z() as f32 / 32.0,
        ];
        let normal = self.normal();
        let uv = [self.u(), self.v()];
        (position, normal, uv)
    }
}

/// 紧凑 Mesh 数据
#[derive(Clone, Debug)]
pub struct CompactMeshData {
    /// 紧凑顶点数据
    pub vertices: Vec<CompactVertex>,
    /// 索引数据
    pub indices: Vec<u32>,
    /// 三角形数量
    pub triangle_count: u32,
}

impl CompactMeshData {
    pub fn new() -> Self {
        Self {
            vertices: Vec::new(),
            indices: Vec::new(),
            triangle_count: 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.vertices.is_empty()
    }

    /// 从标准格式创建紧凑 Mesh
    pub fn from_standard(
        positions: &[[f32; 3]],
        normals: &[[f32; 3]],
        uvs: &[[f32; 2]],
        indices: &[u32],
        model_id: u16,
    ) -> Self {
        let vertex_count = positions.len();
        let mut vertices = Vec::with_capacity(vertex_count);

        for i in 0..vertex_count {
            let vertex = CompactVertex::from_standard(
                positions[i],
                normals[i],
                uvs[i],
                model_id,
            );
            vertices.push(vertex);
        }

        Self {
            vertices,
            indices: indices.to_vec(),
            triangle_count: indices.len() as u32 / 3,
        }
    }

    /// 转换为标准格式
    pub fn to_standard(&self) -> (Vec<[f32; 3]>, Vec<[f32; 3]>, Vec<[f32; 2]>) {
        let mut positions = Vec::with_capacity(self.vertices.len());
        let mut normals = Vec::with_capacity(self.vertices.len());
        let mut uvs = Vec::with_capacity(self.vertices.len());

        for vertex in &self.vertices {
            let (pos, norm, uv) = vertex.to_standard();
            positions.push(pos);
            normals.push(norm);
            uvs.push(uv);
        }

        (positions, normals, uvs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compact_vertex_encoding() {
        let vertex = CompactVertex::new(100, 200, 300, NormalDirection::Up, 128, 64, 42);

        assert_eq!(vertex.x(), 100);
        assert_eq!(vertex.y(), 200);
        assert_eq!(vertex.z(), 300);
        assert_eq!(vertex.normal_direction(), NormalDirection::Up);
        assert_eq!(vertex.model_id(), 42);
    }

    #[test]
    fn test_normal_direction() {
        assert_eq!(
            NormalDirection::from_normal(1.0, 0.0, 0.0),
            NormalDirection::Right
        );
        assert_eq!(
            NormalDirection::from_normal(-1.0, 0.0, 0.0),
            NormalDirection::Left
        );
        assert_eq!(
            NormalDirection::from_normal(0.0, 1.0, 0.0),
            NormalDirection::Up
        );
        assert_eq!(
            NormalDirection::from_normal(0.0, -1.0, 0.0),
            NormalDirection::Down
        );
        assert_eq!(
            NormalDirection::from_normal(0.0, 0.0, 1.0),
            NormalDirection::Front
        );
        assert_eq!(
            NormalDirection::from_normal(0.0, 0.0, -1.0),
            NormalDirection::Back
        );
    }

    #[test]
    fn test_compact_vertex_size() {
        assert_eq!(std::mem::size_of::<CompactVertex>(), 8);
    }

    #[test]
    fn test_roundtrip() {
        let vertex = CompactVertex::new(100, 200, 300, NormalDirection::Up, 128, 64, 42);
        let (pos, norm, uv) = vertex.to_standard();

        // 由于精度损失，我们只检查大致正确
        assert!((pos[0] - 100.0 / 32.0).abs() < 0.01);
        assert!((pos[1] - 200.0 / 32.0).abs() < 0.01);
        assert!((pos[2] - 300.0 / 32.0).abs() < 0.01);
        assert_eq!(norm, [0.0, 1.0, 0.0]);
    }
}
