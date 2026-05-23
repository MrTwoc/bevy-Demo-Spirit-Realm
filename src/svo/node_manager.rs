//! CPU 端八叉树管理 (NodeManager)
//!
//! 参考 voxy-dev NodeManager:
//! - 维护八叉树结构：LEAF节点(有几何体) / INNER节点(有子节点)
//! - 处理 section 插入/移除
//! - 管理子节点分割和合并
//!
//! 核心原则：
//! 1. 所有非空节点至少有一个子节点 (child_existence mask != 0)
//! 2. 叶子节点始终包含几何体 (空几何体也算，只是不占内存)
//! 3. Top-level 节点 (LOD=4) 是树的根

use bevy::prelude::Resource;
use std::collections::HashMap;

use crate::svo::{
    encode_position, decode_level, decode_x, decode_y, decode_z,
    make_child_pos, format_pos,
    node_store::{NodeStore, NodeType, GpuNode},
    section_tracker::SectionTracker,
};

/// 脏节点记录，等待 GPU 更新
#[derive(Debug, Clone)]
pub struct DirtyNode {
    pub node_id: u32,
    pub gpu_data: GpuNode,
}

/// 节点管理器
#[derive(Resource)]
pub struct NodeManager {
    /// 扁平节点存储
    pub store: NodeStore,
    /// position → node_id 映射
    pos_to_id: HashMap<u64, u32>,
    /// Top-level 节点 ID 集合
    top_level_ids: Vec<u32>,
    /// 脏节点列表 (等待上传 GPU)
    dirty_nodes: Vec<DirtyNode>,
    /// 待处理的 section 插入队列
    pending_insert: Vec<u64>,
    /// 待处理的 section 移除队列
    pending_remove: Vec<u64>,
}

impl Default for NodeManager {
    fn default() -> Self {
        Self {
            store: NodeStore::new(),
            pos_to_id: HashMap::with_capacity(1024),
            top_level_ids: Vec::with_capacity(256),
            dirty_nodes: Vec::with_capacity(256),
            pending_insert: Vec::new(),
            pending_remove: Vec::new(),
        }
    }
}

impl NodeManager {
    pub fn new() -> Self {
        Self::default()
    }

    // ===== Top-Level 管理 =====

    /// 插入一个 top-level section (LOD=4)
    pub fn insert_top_level(&mut self, section_pos: u64) {
        self.pending_insert.push(section_pos);
    }

    /// 移除一个 top-level section
    pub fn remove_top_level(&mut self, section_pos: u64) {
        self.pending_remove.push(section_pos);
    }

    /// 处理所有待处理的插入
    pub fn process_pending(&mut self, tracker: &mut SectionTracker) {
        // 处理插入
        for &pos in &self.pending_insert.clone() {
            self._insert_top_level_inner(pos, tracker);
        }
        self.pending_insert.clear();

        // 处理移除
        for &pos in &self.pending_remove.clone() {
            self._remove_top_level_inner(pos, tracker);
        }
        self.pending_remove.clear();
    }

    fn _insert_top_level_inner(&mut self, pos: u64, tracker: &mut SectionTracker) {
        if self.pos_to_id.contains_key(&pos) {
            return; // 已存在
        }

        let lvl = decode_level(pos);
        debug_assert_eq!(lvl, 4, "Top-level must be LOD=4");

        let node_id = match self.store.allocate(pos, NodeType::Pending) {
            Some(id) => id,
            None => {
                bevy::log::warn!("[SVO] Node pool exhausted, cannot insert {}", format_pos(pos));
                return;
            }
        };

        self.pos_to_id.insert(pos, node_id);
        self.top_level_ids.push(node_id);

        // 启动叶节点请求：逐步展开 octree
        self.request_leaf_node(node_id, tracker);
    }

    fn _remove_top_level_inner(&mut self, pos: u64, tracker: &mut SectionTracker) {
        let node_id = match self.pos_to_id.remove(&pos) {
            Some(id) => id,
            None => return,
        };

        self.top_level_ids.retain(|&id| id != node_id);
        self.recurse_remove_node(node_id, tracker);
    }

    // ===== 递归节点管理 =====

    /// 递归移除节点及其子节点
    fn recurse_remove_node(&mut self, node_id: u32, tracker: &mut SectionTracker) {
        let node_type = self.store.get_node_type(node_id);
        let pos = self.store.read_node(node_id).position;

        if node_type == NodeType::Inner {
            let child_ptr = self.store.get_child_ptr(node_id);
            let mask = self.store.get_child_existence(node_id);
            // 递归移除所有存在的子节点
            for i in 0..8 {
                if (mask & (1 << i)) != 0 {
                    let child_id = child_ptr + i;
                    self.pos_to_id.remove(&make_child_pos(pos, i));
                    self.recurse_remove_node(child_id, tracker);
                }
            }
            self.store.free_contiguous(child_ptr, 8);
        } else if node_type == NodeType::Leaf {
            // 释放几何体 (由外部系统处理)
            self.clear_geometry(node_id, tracker);
        }

        self.store.free(node_id);
    }

    /// 请求展开叶节点 (加载/生成子节点数据)
    fn request_leaf_node(&mut self, node_id: u32, tracker: &mut SectionTracker) {
        let entry = self.store.read_node(node_id);
        let pos = entry.position;
        let lvl = decode_level(pos);

        self.store.set_in_flight(node_id, true);

        if lvl == 0 {
            // LOD=0: 直接从 tracker 获取 section 数据
            let coord = crate::svo::SectionCoord::decode(pos);
            let section = tracker.acquire(coord);
            let child_mask = section.non_empty_children;
            tracker.release(pos);

            // 如果 section 为空，标记为空几何体
            if child_mask == 0 {
                self.store.set_node_type(node_id, NodeType::Leaf);
                self.store.set_geometry_handle(node_id, 0); // 空几何体
                self.store.set_in_flight(node_id, false);
                self.mark_node_dirty(node_id);
                return;
            }

            // 标记为叶子节点 (几何体由 meshing 系统处理)
            self.store.set_node_type(node_id, NodeType::Leaf);
            self.store.set_child_existence(node_id, child_mask);
            self.store.set_in_flight(node_id, false);
            self.mark_node_dirty(node_id);
        } else {
            // LOD>0: 尝试继续展开
            let x = decode_x(pos);
            let y = decode_y(pos);
            let z = decode_z(pos);

            // 先标记为内部节点
            let child_base = match self.store.allocate_contiguous(8) {
                Some(base) => base,
                None => {
                    bevy::log::warn!("[SVO] Cannot allocate children for L{} node", lvl);
                    self.store.set_in_flight(node_id, false);
                    return;
                }
            };

            self.store.set_node_type(node_id, NodeType::Inner);
            self.store.set_child_ptr(node_id, child_base);
            let half = 1 << (lvl - 1);

            // 为每个子节点递归请求
            for i in 0..8 {
                let child_pos = make_child_pos(pos, i);
                let child_id = child_base + i;
                self.store.write_node(child_id, child_pos, NodeType::Pending);
                self.pos_to_id.insert(child_pos, child_id);
            }

            // 现在逐个子节点检查非空子节
            let mut existence_mask: u8 = 0;
            for i in 0..8 {
                let child_pos = make_child_pos(pos, i);
                let child_id = child_base + i;
                let cx = decode_x(child_pos);
                let cy = decode_y(child_pos);
                let cz = decode_z(child_pos);

                // 检查该子区域是否有体素 (通过 tracker)
                let has_content = self.check_region_has_content(lvl - 1, cx, cy, cz, tracker);
                if has_content {
                    existence_mask |= 1 << i;
                    self.request_leaf_node(child_id, tracker);
                } else {
                    // 空子节点，释放
                    self.store.set_node_type(child_id, NodeType::None);
                    self.pos_to_id.remove(&child_pos);
                }
            }

            self.store.set_child_existence(node_id, existence_mask);
            self.store.set_in_flight(node_id, false);
            self.mark_node_dirty(node_id);
        }
    }

    /// 检查区域是否有体素内容
    fn check_region_has_content(
        &self,
        lvl: u32,
        x: i32,
        y: i32,
        z: i32,
        tracker: &SectionTracker,
    ) -> bool {
        if lvl == 0 {
            // 直接检查 section
            let coord = crate::svo::SectionCoord::new(x, y, z);
            let id = coord.encode();
            if let Some(section) = tracker.get_active(id) {
                return section.solid_count > 0;
            }
            if let Some(section) = tracker.get_cached(id) {
                return section.solid_count > 0;
            }
            // section 不在缓存中，假设为空
            return false;
        } else {
            // 递归检查子区域
            let step = 1 << (lvl - 1);
            for oz in 0..2 {
                for ox in 0..2 {
                    for oy in 0..2 {
                        let child_pos = encode_position(
                            lvl - 1,
                            x * 2 + ox,
                            y * 2 + oy,
                            z * 2 + oz,
                        );
                        // 检查是否有已存在的节点
                        if self.pos_to_id.contains_key(&child_pos) {
                            return true;
                        }
                    }
                }
            }
            false
        }
    }

    /// 清除节点几何体
    fn clear_geometry(&mut self, node_id: u32, _tracker: &mut SectionTracker) {
        self.store.set_geometry_handle(node_id, 0);
        self.store.set_child_existence(node_id, 0);
        self.mark_node_dirty(node_id);
    }

    /// 标记节点为脏
    pub fn mark_node_dirty(&mut self, node_id: u32) {
        if !self.store.node_exists(node_id) {
            return;
        }
        self.store.set_dirty(node_id, true);

        let entry = self.store.read_node(node_id);
        let gpu_node = GpuNode::from_entry(&entry);
        self.dirty_nodes.push(DirtyNode {
            node_id,
            gpu_data: gpu_node,
        });
    }

    /// 取出所有脏节点 (上传 GPU 用)
    pub fn drain_dirty_nodes(&mut self) -> Vec<DirtyNode> {
        let result = self.dirty_nodes.drain(..).collect::<Vec<_>>();
        for dn in &result {
            self.store.set_dirty(dn.node_id, false);
        }
        result
    }

    /// 获取 top-level 节点 ID 列表
    pub fn top_level_ids(&self) -> &[u32] {
        &self.top_level_ids
    }

    /// 获取 GPU 可读的节点数据切片
    pub fn gpu_node_data(&self) -> Vec<GpuNode> {
        let count = self.store.max_id() as usize;
        let mut result = Vec::with_capacity(count);
        for id in 0..count as u32 {
            if self.store.node_exists(id) {
                let entry = self.store.read_node(id);
                result.push(GpuNode::from_entry(&entry));
            } else {
                result.push(GpuNode::zeroed());
            }
        }
        result
    }

    /// 当前节点总数
    pub fn node_count(&self) -> u32 {
        self.store.count()
    }
}
