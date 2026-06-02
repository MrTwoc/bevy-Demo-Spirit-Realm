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
use std::sync::{Arc, Mutex, atomic::{AtomicU64, Ordering}};

use crate::svo::{
    encode_position, decode_level, decode_x, decode_y, decode_z,
    make_child_pos, format_pos,
    node_store::{NodeStore, NodeType},
};

/// GPU 可读的节点数据格式 (16 bytes)
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuNode {
    /// word0: position encoding
    pub position: u64,
    /// word1: geometry_handle(24) | flags(8) | child_existence(8) | child_ptr(24)
    pub data: u64,
}

impl GpuNode {
    pub fn from_store(store: &NodeStore, node_id: u32) -> Self {
        let (position, data) = store.write_node_compact(node_id);
        Self { position, data }
    }

    pub fn zeroed() -> Self {
        Self { position: 0, data: 0 }
    }
}

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
    // ── GPU 数据缓存 (优化：避免每帧全量重建) ──
    /// 节点数据代际号，任何节点数据变化时递增
    generation: u64,
    /// 缓存的 GPU 节点数据，使用 Arc 避免 clone 整个 Vec
    /// generation 未变时仅 Arc::clone（O(1) 引用计数递增）
    gpu_data_cache: Mutex<Arc<Vec<GpuNode>>>,
    /// 缓存对应的 generation，不一致时需要重建
    /// 使用 AtomicU64：has_dirty_nodes（render 线程）读，gpu_node_data（main 线程）写
    cached_generation: AtomicU64,
}

impl Default for NodeManager {
    fn default() -> Self {
        Self {
            store: NodeStore::new(1 << 18), // 262k nodes
            pos_to_id: HashMap::with_capacity(1024),
            top_level_ids: Vec::with_capacity(256),
            dirty_nodes: Vec::with_capacity(256),
            pending_insert: Vec::new(),
            pending_remove: Vec::new(),
            generation: 0,
            gpu_data_cache: Mutex::new(Arc::new(Vec::new())),
            cached_generation: AtomicU64::new(u64::MAX), // 初始强制重建
        }
    }
}

impl NodeManager {
    pub fn new() -> Self {
        Self::default()
    }

    // ===== Top-Level 管理 =====

    /// 标记节点数据发生变化，递增代际号
    fn mark_generation_dirty(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    /// 插入一个 top-level section (LOD=4)
    pub fn insert_top_level(&mut self, section_pos: u64) {
        self.pending_insert.push(section_pos);
    }

    /// 移除一个 top-level section
    pub fn remove_top_level(&mut self, section_pos: u64) {
        self.pending_remove.push(section_pos);
    }

    /// 处理所有待处理的插入
    pub fn process_pending(&mut self, tracker: &mut crate::svo::section_tracker::SectionTracker) {
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

    fn _insert_top_level_inner(&mut self, pos: u64, tracker: &mut crate::svo::section_tracker::SectionTracker) {
        if self.pos_to_id.contains_key(&pos) {
            return; // 已存在
        }

        let lvl = decode_level(pos);
        debug_assert_eq!(lvl, 4, "Top-level must be LOD=4");

        let node_id = match self.store.allocate() {
            Some(id) => id,
            None => {
                bevy::log::warn!("[SVO] Node pool exhausted, cannot insert {}", format_pos(pos));
                return;
            }
        };

        // 设置节点位置和类型
        self.store.set_node_position(node_id, pos);
        self.store.set_node_type(node_id, NodeType::Pending);

        self.pos_to_id.insert(pos, node_id);
        self.top_level_ids.push(node_id);

        self.mark_generation_dirty();

        // 启动叶节点请求：逐步展开 octree
        self.request_leaf_node(node_id, tracker);
    }

    fn _remove_top_level_inner(&mut self, pos: u64, tracker: &mut crate::svo::section_tracker::SectionTracker) {
        let node_id = match self.pos_to_id.remove(&pos) {
            Some(id) => id,
            None => return,
        };

        self.top_level_ids.retain(|&id| id != node_id);
        self.recurse_remove_node(node_id, tracker);
        self.mark_generation_dirty();
    }

    // ===== 递归节点管理 =====

    /// 递归移除节点及其子节点
    fn recurse_remove_node(&mut self, node_id: u32, tracker: &mut crate::svo::section_tracker::SectionTracker) {
        let node_type = self.store.get_node_type(node_id);
        let pos = self.store.node_position(node_id);

        if node_type == NodeType::Inner {
            let child_ptr = self.store.get_child_ptr(node_id);
            let mask = self.store.get_node_child_existence(node_id);
            // 递归移除所有存在的子节点
            for i in 0..8 {
                if (mask & (1 << i)) != 0 {
                    let child_id = child_ptr as u32 + i;
                    self.pos_to_id.remove(&make_child_pos(pos, i));
                    self.recurse_remove_node(child_id, tracker);
                }
            }
            self.store.free_contiguous(child_ptr as u32, 8);
        } else if node_type == NodeType::Leaf {
            // 释放几何体 (由外部系统处理)
            self.clear_geometry(node_id, tracker);
        }

        self.store.free(node_id);
    }

    /// 请求展开叶节点 (加载/生成子节点数据)
    ///
    /// 优化：先检查 8 个子节点的内容，只有非空时才分配写入，
    /// 避免 "先分配再回退" 浪费。
    fn request_leaf_node(&mut self, node_id: u32, tracker: &mut crate::svo::section_tracker::SectionTracker) {
        let pos = self.store.node_position(node_id);
        let lvl = decode_level(pos);

        self.store.mark_request_in_flight(node_id);

        if lvl == 0 {
            // LOD=0: 直接从 tracker 获取 section 数据
            let coord = crate::svo::SectionCoord::decode(pos);
            let section = tracker.acquire(coord);
            let child_mask = section.non_empty_children;
            tracker.release(pos);

            // 如果 section 为空，标记为空几何体
            if child_mask == 0 {
                self.store.set_node_type(node_id, NodeType::Leaf);
                self.store.set_node_geometry(node_id, -2); // 空几何体
                self.store.unmark_request_in_flight(node_id);
                self.mark_node_dirty(node_id);
                return;
            }

            // 标记为叶子节点 (几何体由 meshing 系统处理)
            self.store.set_node_type(node_id, NodeType::Leaf);
            self.store.set_node_child_existence(node_id, child_mask);
            self.store.unmark_request_in_flight(node_id);
            self.mark_node_dirty(node_id);
        } else {
            // LOD>0: 先检查子节点内容，再分配
            // Step 1: 检查 8 个子节点哪些有内容
            let mut existence_mask: u8 = 0;
            let mut child_has_content = [false; 8];
            for i in 0..8u32 {
                let child_pos = make_child_pos(pos, i);
                let cx = decode_x(child_pos);
                let cy = decode_y(child_pos);
                let cz = decode_z(child_pos);
                let has_content = self.check_region_has_content(lvl - 1, cx, cy, cz, tracker);
                child_has_content[i as usize] = has_content;
                if has_content {
                    existence_mask |= 1u8 << (i as u8);
                }
            }

            // 所有子节点都为空 → 本节点为空叶子，不继续展开
            if existence_mask == 0 {
                self.store.set_node_type(node_id, NodeType::Leaf);
                self.store.set_node_geometry(node_id, -2); // 空几何体
                self.store.unmark_request_in_flight(node_id);
                self.mark_node_dirty(node_id);
                return;
            }

            // Step 2: 分配 8 个连续子节点槽位
            let child_base = match self.store.allocate_contiguous(8) {
                Some(base) => base,
                None => {
                    bevy::log::warn!("[SVO] Cannot allocate children for L{} node", lvl);
                    self.store.unmark_request_in_flight(node_id);
                    return;
                }
            };

            // 标记为内部节点
            self.store.set_node_type(node_id, NodeType::Inner);
            self.store.set_child_ptr(node_id, child_base as i32);
            self.store.set_node_child_existence(node_id, existence_mask);
            self.store.set_child_ptr_count(node_id, 8);

            // Step 3: 只写入非空子节点（空子节点保持 None 避免 HashMap 污染）
            for i in 0..8u32 {
                let child_pos = make_child_pos(pos, i);
                let child_id = child_base + i;
                if child_has_content[i as usize] {
                    self.store.set_node_position(child_id, child_pos);
                    self.store.set_node_type(child_id, NodeType::Pending);
                    self.pos_to_id.insert(child_pos, child_id);
                } else {
                    self.store.set_node_position(child_id, child_pos);
                    self.store.set_node_type(child_id, NodeType::None);
                }
            }

            // Step 4: 递归展开非空子节点
            for i in 0..8u32 {
                if child_has_content[i as usize] {
                    let child_id = child_base + i;
                    self.request_leaf_node(child_id, tracker);
                }
            }

            self.store.unmark_request_in_flight(node_id);
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
        tracker: &crate::svo::section_tracker::SectionTracker,
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
            // ── 漏判修复：检查 pending_insert 中是否有此 section 的顶层父节点 ──
            // 当一批 section 同时插入，tracker 尚未被 request_leaf_node 的
            // acquire() 填充时，check_region_has_content 会返回 false，
            // 导致递归分裂提前终止，子节点不被分配，区域不可见。
            let parent_lvl4 = crate::svo::encode_position(
                4,          // MAX_LOD
                x >> 4,     // LOD=0 → LOD=4 坐标转换
                y >> 4,
                z >> 4,
            );
            if self.pending_insert.contains(&parent_lvl4) {
                return true;
            }
            // section 不在缓存中，假设为空
            return false;
        } else {
            // 递归检查子区域
            let step = 1 << (lvl - 1);
            for oz in 0..2 {
                for ox in 0..2 {
                    for oy in 0..2 {
                        let child_pos = crate::svo::encode_position(
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
    fn clear_geometry(&mut self, node_id: u32, _tracker: &mut crate::svo::section_tracker::SectionTracker) {
        self.store.set_node_geometry(node_id, 0);
        self.store.set_node_child_existence(node_id, 0);
        self.mark_node_dirty(node_id);
    }

    /// 标记节点为脏
    pub fn mark_node_dirty(&mut self, node_id: u32) {
        if !self.store.node_exists(node_id) {
            return;
        }
        self.store.set_dirty(node_id, true);

        let gpu_node = GpuNode::from_store(&self.store, node_id);
        self.dirty_nodes.push(DirtyNode {
            node_id,
            gpu_data: gpu_node,
        });
        self.mark_generation_dirty();
    }

    /// 取出所有脏节点 (上传 GPU 用)
    pub fn drain_dirty_nodes(&mut self) -> Vec<DirtyNode> {
        let result = self.dirty_nodes.drain(..).collect::<Vec<_>>();
        for dn in &result {
            self.store.set_dirty(dn.node_id, false);
        }
        result
    }

    /// 是否有脏节点待上传 (供 extract 系统判断是否需更新 GPU buffer)
    pub fn has_dirty_nodes(&self) -> bool {
        self.generation != self.cached_generation.load(Ordering::Acquire)
    }

    /// 获取当前代际号（visibility_bridge 用于检测 SVO 树变化）
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// 获取 top-level 节点 ID 列表
    pub fn top_level_ids(&self) -> &[u32] {
        &self.top_level_ids
    }

    /// 获取 GPU 可读的节点数据（使用缓存，避免每帧全量重建）
    ///
    /// 返回 `Arc<Vec<GpuNode>>`：
    /// - 缓存命中时仅 clone Arc（O(1) 引用计数递增），不拷贝底层数据
    /// - 缓存失效时重建 Vec 并存入 Arc
    /// 采用 Mutex 实现 &self 下的内部可变性，兼容 `Res<NodeManager>` 只读访问。
    pub fn gpu_node_data(&self) -> Arc<Vec<GpuNode>> {
        let cur_gen = self.generation;
        let mut cache = self.gpu_data_cache.lock().unwrap();

        // 缓存命中 → O(1) Arc clone
        if self.cached_generation.load(Ordering::Acquire) == cur_gen {
            return Arc::clone(&cache);
        }

        // 缓存失效 → 重建
        let count = self.store.end_node_id() as usize + 1;
        let mut new_data = Vec::with_capacity(count);
        for id in 0..count as u32 {
            if self.store.node_exists(id) {
                new_data.push(GpuNode::from_store(&self.store, id));
            } else {
                new_data.push(GpuNode::zeroed());
            }
        }
        *cache = Arc::new(new_data);
        self.cached_generation.store(cur_gen, Ordering::Release);
        Arc::clone(&cache)
    }

    /// 当前节点总数
    pub fn node_count(&self) -> u32 {
        self.store.node_count()
    }
}
