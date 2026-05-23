//! Section (区块) 数据结构
//!
//! Section = 32×32×32 体素区块，对应原有的 Chunk。
//! 但不再存储渲染网格数据，只存储原始体素数据。
//! 渲染数据由 SVO NodeManager 管理。

use crate::svo::config::{SECTION_SIZE, SECTION_VOLUME};
use crate::svo::encode_position;

/// Section ID (位置编码，LOD=0)
pub type SectionId = u64;

/// 体素值: 0=空气, 其他=方块ID
pub type Voxel = u16;

/// 区块坐标 (section 坐标)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SectionCoord {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl SectionCoord {
    pub fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    /// 编码为 SectionId (LOD=0)
    pub fn encode(&self) -> SectionId {
        encode_position(0, self.x, self.y, self.z)
    }

    /// 从 SectionId 解码
    pub fn decode(id: SectionId) -> Self {
        use crate::svo::{decode_x, decode_y, decode_z};
        Self {
            x: decode_x(id),
            y: decode_y(id),
            z: decode_z(id),
        }
    }

    /// 到玩家的距离平方 (体素单位)
    pub fn distance_sq(&self, px: f64, py: f64, pz: f64) -> f64 {
        let bx = self.x as f64 * SECTION_SIZE as f64 + SECTION_SIZE as f64 / 2.0;
        let by = self.y as f64 * SECTION_SIZE as f64 + SECTION_SIZE as f64 / 2.0;
        let bz = self.z as f64 * SECTION_SIZE as f64 + SECTION_SIZE as f64 / 2.0;
        (bx - px).powi(2) + (by - py).powi(2) + (bz - pz).powi(2)
    }
}

/// Section 数据
pub struct Section {
    /// 坐标
    pub coord: SectionCoord,
    /// 体素数据 [x][z][y] 展平存储
    pub voxels: Vec<Voxel>,
    /// LOD-1 非空子节点掩码 (8-bit)
    /// 每个 bit 表示对应 octant 是否有非空体素
    pub non_empty_children: u8,
    /// 非空体素数量
    pub solid_count: u32,
    /// 引用计数
    pub ref_count: u32,
    /// 是否已脏 (数据已修改)
    pub is_dirty: bool,
}

impl Section {
    pub fn new(coord: SectionCoord) -> Self {
        Self {
            coord,
            voxels: vec![0; SECTION_VOLUME],
            non_empty_children: 0,
            solid_count: 0,
            ref_count: 1,
            is_dirty: true,
        }
    }

    /// 获取体素值
    #[inline]
    pub fn get_voxel(&self, x: u32, y: u32, z: u32) -> Voxel {
        debug_assert!(x < SECTION_SIZE && y < SECTION_SIZE && z < SECTION_SIZE);
        let idx = (z * SECTION_SIZE + x) * SECTION_SIZE + y;
        self.voxels[idx as usize]
    }

    /// 设置体素值
    #[inline]
    pub fn set_voxel(&mut self, x: u32, y: u32, z: u32, value: Voxel) {
        debug_assert!(x < SECTION_SIZE && y < SECTION_SIZE && z < SECTION_SIZE);
        let idx = (z * SECTION_SIZE + x) * SECTION_SIZE + y;
        let old = self.voxels[idx as usize];
        if old != value {
            self.voxels[idx as usize] = value;
            self.is_dirty = true;
            if old == 0 && value != 0 {
                self.solid_count += 1;
            } else if old != 0 && value == 0 {
                self.solid_count -= 1;
            }
        }
    }

    /// 批量设置体素数据
    pub fn set_voxels(&mut self, data: &[Voxel]) {
        self.voxels.copy_from_slice(data);
        self.is_dirty = true;
        self.recalc_metadata();
    }

    /// 重新计算元数据 (solid_count, non_empty_children)
    pub fn recalc_metadata(&mut self) {
        self.solid_count = 0;
        self.non_empty_children = 0;

        let half = SECTION_SIZE / 2;
        // 检查 8 个 octant
        for oz in 0..2 {
            for ox in 0..2 {
                for oy in 0..2 {
                    let mut has_solid = false;
                    let z_start = oz * half;
                    let x_start = ox * half;
                    let y_start = oy * half;

                    'outer: for z in z_start..z_start + half {
                        for x in x_start..x_start + half {
                            for y in y_start..y_start + half {
                                let idx = (z * SECTION_SIZE + x) * SECTION_SIZE + y;
                                if self.voxels[idx as usize] != 0 {
                                    has_solid = true;
                                    self.solid_count += 1;
                                    break 'outer;
                                }
                            }
                        }
                    }

                    if has_solid {
                        let bit_idx = oz * 4 + ox * 2 + oy; // z,x,y 编码
                        self.non_empty_children |= 1 << bit_idx;
                    }
                }
            }
        }
    }

    /// 增加引用计数
    pub fn acquire(&mut self) {
        self.ref_count += 1;
    }

    /// 减少引用计数，返回是否应该释放
    pub fn release(&mut self) -> bool {
        self.ref_count -= 1;
        self.ref_count == 0
    }
}
