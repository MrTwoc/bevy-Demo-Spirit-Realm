//! SVO 同步系统
//!
//! 替代 `render_distance.rs`：根据玩家位置管理 SVO top-level 节点，
//! 不再独立创建 Section / 填充地形，而是通过 `VoxelSource` 从
//! `LoadedChunks` 读取现有数据。
//!
//! # 与 Chunk 加载的关系
//!
//! 本系统负责管理 SVO 八叉树结构（用于可见性剔除），
//! `chunk_manager` 负责加载实际区块数据。
//! SVO 树的构建滞后于区块加载：仅当区块已在 LoadedChunks 中时，
//! `request_leaf_node` 才能判定其内容。
//!
//! # Plan B 过渡
//!
//! 当 Plan B 启用 GPU-driven 渲染时，`SvoSyncController` 的
//! top-level 管理逻辑保持不变，仅需将 `ChunkVoxelSource` 替换为
//! GPU buffer 回读适配器。

use bevy::prelude::*;
use std::collections::HashSet;

use crate::chunk_manager::{LoadedChunks, RENDER_DISTANCE};
use crate::player::Player;
use crate::svo::{
    config::{MAX_LOD, TOP_LEVEL_SIZE},
    encode_position,
    node_manager::NodeManager,
    voxel_source::{ChunkVoxelSource, VoxelSource},
};

/// SVO 同步控制器
///
/// 跟踪玩家位置，管理 SVO top-level 节点的加载/卸载。
/// 使用 Chebyshev 距离（方形范围）确保对角线方向不遗漏。
#[derive(Resource)]
pub struct SvoSyncController {
    /// 玩家当前所在的 top-level section 坐标 (xz 平面)
    current_tx: i32,
    current_tz: i32,
    /// 已加载的 top-level section 集合 (x, y, z)
    loaded: HashSet<(i32, i32, i32)>,
    /// 是否需要重建（玩家移动超过阈值时触发）
    needs_rebuild: bool,
    /// 上次玩家位置（用于移动检测）
    last_px: f64,
    last_pz: f64,
    /// 每帧最大处理数（避免卡顿）
    process_rate: usize,
}

impl Default for SvoSyncController {
    fn default() -> Self {
        Self {
            current_tx: i32::MAX, // 强制首次更新
            current_tz: i32::MAX,
            loaded: HashSet::new(),
            needs_rebuild: false,
            last_px: f64::MAX,
            last_pz: f64::MAX,
            process_rate: 16,
        }
    }
}

impl SvoSyncController {
    /// 更新玩家中心位置。
    ///
    /// 仅当移动距离超过 32 体素时才触发重建，
    /// 避免每帧计算差异。
    pub fn set_center(&mut self, px: f64, pz: f64) {
        let dx = px - self.last_px;
        let dz = pz - self.last_pz;
        if dx * dx + dz * dz > 1024.0 {
            // 1024 = 32²
            self.last_px = px;
            self.last_pz = pz;
            self.needs_rebuild = true;
        }
    }

    /// 计算需要加载和卸载的 top-level section 列表。
    ///
    /// 使用 Chebyshev 距离（方形），覆盖范围 = render_distance 个 top-level section。
    pub fn compute_updates(&mut self) -> (Vec<(i32, i32, i32)>, Vec<(i32, i32, i32)>) {
        let mut to_add = Vec::new();
        let mut to_remove = Vec::new();

        if !self.needs_rebuild {
            return (to_add, to_remove);
        }
        self.needs_rebuild = false;

        // 玩家所在 top-level 坐标
        let tx = (self.last_px as i32) / (TOP_LEVEL_SIZE as i32);
        let tz = (self.last_pz as i32) / (TOP_LEVEL_SIZE as i32);

        // 渲染距离：chunk 数 → top-level section 数
        // 1 top-level = 16 chunks
        let rd = (RENDER_DISTANCE / 16).max(1);

        // 构建新的加载集合（Y 范围覆盖地表到天空）
        let y_min = -2;
        let y_max = 2;

        let mut new_loaded = HashSet::new();
        for dz in -rd..=rd {
            for dx in -rd..=rd {
                if dx.abs().max(dz.abs()) <= rd {
                    for y in y_min..=y_max {
                        new_loaded.insert((tx + dx, y, tz + dz));
                    }
                }
            }
        }

        // 计算差异
        for pos in &new_loaded {
            if !self.loaded.contains(pos) {
                to_add.push(*pos);
            }
        }
        for pos in &self.loaded {
            if !new_loaded.contains(pos) {
                to_remove.push(*pos);
            }
        }

        self.loaded = new_loaded;
        (to_add, to_remove)
    }
}

/// 同步 SVO 八叉树与 LoadedChunks 的系统。
///
/// 每帧：
/// 1. 检测玩家位置变化 → 计算需加载/卸载的 top-level section
/// 2. 插入/移除 NodeManager 中的对应节点
/// 3. 调用 `process_pending` 展开八叉树（读取 LoadedChunks 数据）
pub fn sync_svo_with_loaded_chunks(
    mut controller: ResMut<SvoSyncController>,
    mut node_manager: ResMut<NodeManager>,
    loaded_chunks: Res<LoadedChunks>,
    camera_query: Query<&Transform, With<Player>>,
) {
    // ── 获取玩家位置 ──
    let cam_transform = match camera_query.iter().next() {
        Some(t) => t,
        None => return,
    };
    let px = cam_transform.translation.x;
    let pz = cam_transform.translation.z;

    controller.set_center(px as f64, pz as f64);

    let (to_add, to_remove) = controller.compute_updates();
    if to_add.is_empty() && to_remove.is_empty() {
        return;
    }

    // ── 处理插入 ──
    let process = controller.process_rate;
    let add_count = to_add.len().min(process);

    for &(x, y, z) in to_add.iter().take(add_count) {
        let pos = encode_position(MAX_LOD, x, y, z);
        node_manager.insert_top_level(pos);
    }

    // ── 处理移除 ──
    for &(x, y, z) in &to_remove {
        let pos = encode_position(MAX_LOD, x, y, z);
        node_manager.remove_top_level(pos);
    }

    // ── 处理待处理任务（从 LoadedChunks 读取体素数据构建八叉树） ──
    let source = ChunkVoxelSource {
        loaded: &loaded_chunks,
    };
    node_manager.process_pending(&source);

    // 统计日志（仅在变更时输出）
    if !to_add.is_empty() || !to_remove.is_empty() {
        bevy::log::info!(
            "[SVO-Sync] +{} / -{} top-level sections, {} nodes",
            to_add.len().min(process),
            to_remove.len(),
            node_manager.node_count(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_state() {
        let ctrl = SvoSyncController::default();
        assert!(ctrl.loaded.is_empty());
        assert!(ctrl.needs_rebuild);
    }

    #[test]
    fn test_set_center_triggers_rebuild() {
        let mut ctrl = SvoSyncController::default();
        ctrl.last_px = 0.0;
        ctrl.last_pz = 0.0;
        ctrl.needs_rebuild = false;

        // 移动超过 32 体素 → 触发重建
        ctrl.set_center(33.0, 0.0);
        assert!(ctrl.needs_rebuild);
    }

    #[test]
    fn test_small_movement_no_rebuild() {
        let mut ctrl = SvoSyncController::default();
        ctrl.last_px = 0.0;
        ctrl.last_pz = 0.0;
        ctrl.needs_rebuild = false;

        // 移动 10 体素 → 不触发
        ctrl.set_center(10.0, 0.0);
        assert!(!ctrl.needs_rebuild);
    }
}
