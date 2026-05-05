//! Texture Atlas — UV 计算和纹理槽位定义
//!
//! array_texture.png 布局：250×1000 像素，子图 32×32 像素，水平排列。
//! Slot (col, row) 表示第 col 列、第 row 行的子图区域。

/// 子图像素尺寸（长和宽均为 32px）
pub const TILE_SIZE_PX: u32 = 32;

/// Atlas 图片尺寸
pub const ATLAS_WIDTH_PX: f32 = 250.0;
pub const ATLAS_HEIGHT_PX: f32 = 1000.0;

/// Atlas 中的一个槽位（子图位置）
#[derive(Clone, Copy, Debug)]
pub struct AtlasSlot {
    pub col: u32,
    pub row: u32,
}

impl AtlasSlot {
    /// 从行列计算 UV 范围（返回 u_min, u_max, v_min, v_max）
    pub fn uv(&self) -> (f32, f32, f32, f32) {
        let u_min = (self.col * TILE_SIZE_PX) as f32 / ATLAS_WIDTH_PX;
        let u_max = ((self.col + 1) * TILE_SIZE_PX) as f32 / ATLAS_WIDTH_PX;
        let v_min = (self.row * TILE_SIZE_PX) as f32 / ATLAS_HEIGHT_PX;
        let v_max = ((self.row + 1) * TILE_SIZE_PX) as f32 / ATLAS_HEIGHT_PX;
        (u_min, u_max, v_min, v_max)
    }
}

/// 草方块的 6 个面各自使用的 atlas slot
pub mod grass {
    use super::AtlasSlot;
    pub const TOP:    AtlasSlot = AtlasSlot { col: 0, row: 0 }; // 草地顶
    pub const BOTTOM: AtlasSlot = AtlasSlot { col: 0, row: 1 }; // 泥土（底面用土）
    pub const RIGHT:  AtlasSlot = AtlasSlot { col: 1, row: 0 }; // 草土侧面
    pub const LEFT:   AtlasSlot = AtlasSlot { col: 1, row: 0 }; // 草土侧面
    pub const FRONT:  AtlasSlot = AtlasSlot { col: 1, row: 0 }; // 草土侧面
    pub const BACK:   AtlasSlot = AtlasSlot { col: 1, row: 0 }; // 草土侧面
}

/// 泥土方块的 6 个面
pub mod dirt {
    use super::AtlasSlot;
    pub const TOP:    AtlasSlot = AtlasSlot { col: 0, row: 1 }; // 土顶
    pub const BOTTOM: AtlasSlot = AtlasSlot { col: 0, row: 1 }; // 土底
    pub const RIGHT:  AtlasSlot = AtlasSlot { col: 1, row: 1 }; // 土侧
    pub const LEFT:   AtlasSlot = AtlasSlot { col: 1, row: 1 }; // 土侧
    pub const FRONT:  AtlasSlot = AtlasSlot { col: 1, row: 1 }; // 土侧
    pub const BACK:   AtlasSlot = AtlasSlot { col: 1, row: 1 }; // 土侧
}

/// 石头方块的 6 个面
pub mod stone {
    use super::AtlasSlot;
    pub const TOP:    AtlasSlot = AtlasSlot { col: 2, row: 0 }; // 石顶
    pub const BOTTOM: AtlasSlot = AtlasSlot { col: 2, row: 0 }; // 石底
    pub const RIGHT:  AtlasSlot = AtlasSlot { col: 2, row: 0 }; // 石侧
    pub const LEFT:   AtlasSlot = AtlasSlot { col: 2, row: 0 }; // 石侧
    pub const FRONT:  AtlasSlot = AtlasSlot { col: 2, row: 0 }; // 石侧
    pub const BACK:   AtlasSlot = AtlasSlot { col: 2, row: 0 }; // 石侧
}
