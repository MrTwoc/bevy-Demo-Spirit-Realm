//! 渲染距离管理 (RenderDistance)
//!
//! 参考 voxy-dev RenderDistanceTracker:
//! - 根据玩家位置动态加载/卸载 top-level sections
//! - 使用环形追踪器 (ring tracker) 逐步处理
//! - 避免每帧全量扫描

use bevy::prelude::*;
use std::collections::HashSet;

use crate::chunk_manager::RENDER_DISTANCE;
use crate::player::Player;
use crate::svo::{
    encode_position, format_pos,
    config::{MAX_LOD, TOP_LEVEL_SIZE, TOP_LEVEL_CACHE_SIZE},
    node_manager::NodeManager,
    section_tracker::SectionTracker,
    SectionCoord,
};

/// 渲染距离控制器
#[derive(Resource)]
pub struct RenderDistanceController {
    /// 渲染距离 (top-level section 数量, 默认 8 = 512 体素)
    pub render_distance: i32,
    /// 最小 Y
    pub min_y: i32,
    /// 最大 Y
    pub max_y: i32,
    /// 当前玩家所在的 top-level section 坐标
    current_tx: i32,
    current_tz: i32,
    /// 已加载的 top-level sections
    loaded: HashSet<(i32, i32)>,
    /// 每帧处理速率 (避免卡顿)
    pub process_rate: usize,
    /// 上次玩家位置
    last_px: f64,
    last_pz: f64,
    /// 是否需要重新扫描
    needs_rebuild: bool,
}

impl Default for RenderDistanceController {
    fn default() -> Self {
        Self {
            render_distance: RENDER_DISTANCE / 16,  // chunk 单位 → section 单位 (1 section = 16 chunks)
            min_y: -2,
            max_y: 2,
            current_tx: i32::MAX, // 强制首次更新
            current_tz: i32::MAX,
            loaded: HashSet::with_capacity(TOP_LEVEL_CACHE_SIZE),
            process_rate: 16,  // 每帧处理 16 个
            last_px: f64::MAX,
            last_pz: f64::MAX,
            needs_rebuild: false,
        }
    }
}

impl RenderDistanceController {
    /// 设置渲染距离
    pub fn set_render_distance(&mut self, distance: i32) {
        if distance != self.render_distance {
            self.render_distance = distance;
            self.needs_rebuild = true;
        }
    }

    /// 更新中心位置
    pub fn set_center(&mut self, px: f64, pz: f64) {
        let dx = px - self.last_px;
        let dz = pz - self.last_pz;
        // 超过 32 体素才触发更新
        if dx * dx + dz * dz > 1024.0 {
            self.last_px = px;
            self.last_pz = pz;
            self.needs_rebuild = true;
        }
    }

    /// 获取需要加载/卸载的 section 列表
    pub fn compute_updates(&mut self) -> (Vec<(i32, i32, i32)>, Vec<(i32, i32, i32)>) {
        let mut to_add = Vec::new();
        let mut to_remove = Vec::new();

        if !self.needs_rebuild {
            return (to_add, to_remove);
        }
        self.needs_rebuild = false;

        let tx = (self.last_px as i32) >> (MAX_LOD + 5); // 32*32 = 1024 >> (9+LOD)
        let tz = (self.last_pz as i32) >> (MAX_LOD + 5);

        // 校正: top-level section 的坐标应该除以 section_size << max_lod
        let tx = (self.last_px as i32) / (TOP_LEVEL_SIZE as i32);
        let tz = (self.last_pz as i32) / (TOP_LEVEL_SIZE as i32);

        // 新集合
        let mut new_loaded = HashSet::new();
        let rd = self.render_distance;

        for dz in -rd..=rd {
            for dx in -rd..=rd {
                let x = tx + dx;
                let z = tz + dz;
                // Chebyshev 距离（方形范围），确保对角线方向也被覆盖
                // 之前用欧氏距离 dx*dx+dz*dz <= rd*rd 会漏掉对角线方向的 section，
                // 导致玩家移动时身后/脚下的区块因 SVO 中缺少对应 top-level 节点而被错误隐藏。
                if dx.abs().max(dz.abs()) <= rd {
                    for y in self.min_y..=self.max_y {
                        new_loaded.insert((x, y, z));
                    }
                }
            }
        }

        // 计算差异
        let old_set: HashSet<(i32, i32, i32)> = self.loaded.iter()
            .flat_map(|&(x, z)| {
                (self.min_y..=self.max_y).map(move |y| (x, y, z))
            })
            .collect();

        for pos in &new_loaded {
            if !old_set.contains(pos) {
                to_add.push(*pos);
            }
        }

        for pos in &old_set {
            if !new_loaded.contains(pos) {
                to_remove.push(*pos);
            }
        }

        // 更新 loaded 集合 (只存 x,z)
        let new_xz: HashSet<(i32, i32)> = new_loaded.iter().map(|&(x, y, z)| (x, z)).collect();
        self.loaded = new_xz;

        (to_add, to_remove)
    }
}

/// 处理渲染距离的系统
pub fn process_render_distance(
    mut controller: ResMut<RenderDistanceController>,
    mut node_manager: ResMut<NodeManager>,
    mut tracker: ResMut<SectionTracker>,
    camera_query: Query<&Transform, With<Player>>,
) {
    let cam_transform = if let Some(t) = camera_query.iter().next() {
        t
    } else {
        return;
    };

    let px = cam_transform.translation.x;
    let pz = cam_transform.translation.z;

    controller.set_center(px as f64, pz as f64);

    let (to_add, to_remove) = controller.compute_updates();
    if to_add.is_empty() && to_remove.is_empty() {
        return;
    }

    // 分批处理
    let process = controller.process_rate;
    let add_count = to_add.len().min(process);

    // 处理插入
    for &(x, y, z) in to_add.iter().take(add_count) {
        let pos = encode_position(MAX_LOD, x, y, z);
        // 先确保 section 在 tracker 中（acquire 会创建 section 并填充地形数据）
        let coord = SectionCoord::new(
            x << MAX_LOD,
            y << MAX_LOD,
            z << MAX_LOD,
        );
        // acquire 会创建 section 并调用 on_create_section 回调填充地形数据
        // 不立即 release，让 section 保持活跃（ref_count > 0）
        let _section = tracker.acquire(coord);
        // 插入节点
        node_manager.insert_top_level(pos);
    }

    // 处理移除 (全部移除)
    for &(x, y, z) in &to_remove {
        let pos = encode_position(MAX_LOD, x, y, z);
        // 先释放 section（坐标与插入时一致）
        let coord = SectionCoord::new(
            x << MAX_LOD,
            y << MAX_LOD,
            z << MAX_LOD,
        );
        tracker.release(coord.encode());
        // 再移除节点
        node_manager.remove_top_level(pos);
    }

    // 处理待处理的任务
    node_manager.process_pending(&mut *tracker);

    // 统计信息
    let node_count = node_manager.node_count();
    let active_sections = tracker.active_count();
    let cached_sections = tracker.cached_count();

    if to_add.len() > process {
        bevy::log::trace!(
            "[SVO] {} pending loads, {} active sections, {} cached, {} nodes",
            to_add.len() - add_count,
            active_sections,
            cached_sections,
            node_count,
        );
    }

    // 仅在变更时输出日志
    if !to_add.is_empty() || !to_remove.is_empty() {
        bevy::log::info!(
            "[SVO] Loaded {} sections, unloaded {}, total: {} active, {} cached, {} nodes",
            to_add.len(),
            to_remove.len(),
            active_sections,
            cached_sections,
            node_count,
        );
        
        // 调试：检查 section 引用计数
        if active_sections == 0 && node_count > 0 {
            bevy::log::warn!(
                "[SVO-DEBUG] No active sections but {} nodes exist. This indicates section lifecycle issue.",
                node_count
            );
        }
    }
}
