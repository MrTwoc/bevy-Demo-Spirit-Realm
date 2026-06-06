//! 地形桥接: 将地形生成数据填入 SVO Section
//!
//! 当 SectionTracker 创建新的空 Section 时，通过回调填入地形数据。
//! 这样 NodeManager 在查询 section 数据时就能得到正确的体素信息。
//!
//! 使用与 `chunk::fill_terrain` 完全相同的 5 层噪声 + 生物群系系统。

use crate::svo::section::{Section, SectionCoord, Voxel};
use crate::svo::config::SECTION_SIZE;

// 从 terrain_noise 导入地形常量和函数
use crate::terrain_noise::{get_terrain_noise,
                    TERRAIN_MIN_Y, TERRAIN_MAX_Y, SEA_LEVEL};
use crate::biome;

/// 用地形生成数据填充一个 Section
///
/// 使用 5 层噪声系统 + 生物群系判定，与 `chunk::fill_terrain` 完全一致。
pub fn fill_section_terrain(section: &mut Section, coord: &SectionCoord) {
    let size = SECTION_SIZE as usize;
    let noise = get_terrain_noise();

    for z in 0..size {
        for x in 0..size {
            let world_x = coord.x as f64 * SECTION_SIZE as f64 + x as f64;
            let world_z = coord.z as f64 * SECTION_SIZE as f64 + z as f64;

            let sample = noise.sample_all(world_x, world_z);
            let surface_height = sample.compute_base_height() as i32;
            let biome = biome::get_biome(&sample, surface_height);

            for y in 0..size {
                let world_y = coord.y as i32 * SECTION_SIZE as i32 + y as i32;

                if world_y > TERRAIN_MAX_Y || world_y < TERRAIN_MIN_Y {
                    continue;
                }

                if world_y > surface_height {
                    if world_y <= SEA_LEVEL && surface_height < SEA_LEVEL {
                        section.set_voxel(x as u32, y as u32, z as u32, 5); // water
                    }
                    continue;
                }

                let depth = surface_height - world_y;
                let block_id: Voxel = if depth == 0 {
                    biome.surface_block(world_y) as Voxel
                } else {
                    biome.subsurface_block(depth) as Voxel
                };

                section.set_voxel(x as u32, y as u32, z as u32, block_id);
            }
        }
    }

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
