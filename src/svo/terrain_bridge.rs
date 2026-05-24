//! 地形桥接: 将地形生成数据填入 SVO Section
//!
//! 当 SectionTracker 创建新的空 Section 时，通过回调填入地形数据。
//! 这样 NodeManager 在查询 section 数据时就能得到正确的体素信息。

use noise::NoiseFn;
use crate::svo::section::{Section, SectionCoord, Voxel};
use crate::svo::config::SECTION_SIZE;

// 从 chunk.rs 导入地形生成所需的噪声函数
use crate::chunk::{get_terrain_noise, TERRAIN_BASE_HEIGHT, TERRAIN_AMPLITUDE,
                    TERRAIN_MIN_Y, TERRAIN_MAX_Y, WATER_LEVEL};

/// 用地形生成数据填充一个 Section
///
/// 逻辑与 `chunk::fill_terrain` 相同，但写入 Section 的 Voxel 数组。
pub fn fill_section_terrain(section: &mut Section, coord: &SectionCoord) {
    let noise = get_terrain_noise();

    let size = SECTION_SIZE as usize;

    let surface_block: Voxel = 1;   // grass
    let under_surface_block: Voxel = 3; // dirt
    let deep_block: Voxel = 2;      // stone
    let soil_thickness: i32 = 4;

    for z in 0..size {
        for x in 0..size {
            let world_x = coord.x as f64 * SECTION_SIZE as f64 + x as f64;
            let world_z = coord.z as f64 * SECTION_SIZE as f64 + z as f64;

            let noise_val = noise.get([world_x, world_z]);
            let surface_height = TERRAIN_BASE_HEIGHT + (noise_val * TERRAIN_AMPLITUDE) as i32;

            for y in 0..size {
                let world_y = coord.y as i32 * SECTION_SIZE as i32 + y as i32;

                // 超出地形生成范围 → 空气 (默认就是 0)
                if world_y > TERRAIN_MAX_Y {
                    continue;
                }
                if world_y < TERRAIN_MIN_Y {
                    continue;
                }

                if world_y > surface_height {
                    // 检查是否应该填充水方块
                    if world_y < WATER_LEVEL && surface_height < WATER_LEVEL {
                        section.set_voxel(x as u32, y as u32, z as u32, 5); // water
                    }
                    continue;
                }

                let block_id: Voxel = if world_y == surface_height {
                    surface_block
                } else if world_y > surface_height - soil_thickness {
                    under_surface_block
                } else {
                    deep_block
                };

                section.set_voxel(x as u32, y as u32, z as u32, block_id);
            }
        }
    }

    // 填充完成后重新计算非空子节点掩码和固体计数
    section.recalc_metadata();
}

/// 创建 Section 的填充回调，用于 SectionTracker
///
/// 返回一个闭包，当 SectionTracker 创建新的 Section 时自动填入地形数据。
pub fn make_section_filler() -> Box<dyn FnMut(&mut Section) + Send + Sync> {
    Box::new(|section: &mut Section| {
        let coord = section.coord;
        fill_section_terrain(section, &coord);
        bevy::log::trace!(
            "[SVO-Terrain] Filled section ({},{},{}) with {} solid voxels",
            coord.x, coord.y, coord.z, section.solid_count,
        );
    })
}
