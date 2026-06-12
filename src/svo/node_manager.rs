//! CPU 端八叉树管理 (NodeManager)
//!
//! 维护八叉树结构：LEAF 节点(有几何体) / INNER 节点(有子节点)。
//! 体素数据通过 `VoxelSource` trait 抽象获取，支持 Plan A (ChunkData)
//! 和 Plan B (GPU buffer) 两种后端。
//!
//! # 设计原则
//!
//! 1. **数据源解耦**：所有体素查询通过 `&dyn VoxelSource`，不持有数据所有权。
//! 2. **扁平数组存储**：NodeStore 是预分配的 u64 数组，适合 GPU 直接映射。
//! 3. **Top-level 节点 (LOD=4) 是树的根**：所有可见性判断从此开始。

use bevy::prelude::Resource;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, atomic::{AtomicU64, Ordering}};

use crate::svo::{
    encode_position, decode_level, decode_x, decode_y, decode_z,
    make_child_pos, format_pos,
    node_store::{NodeStore, NodeType},
    voxel_source::VoxelSource,
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

    /// 处理所有待处理的插入和移除。
    ///
    /// `source` 提供体素内容查询，用于构建八叉树。
    /// Plan A 传入 `ChunkVoxelSource`（读取 LoadedChunks），
    /// Plan B 可传入 GPU buffer 回读适配器。
    pub fn process_pending(&mut self, source: &dyn VoxelSource) {
        // 处理插入（用 take 避免 clone 整个 Vec）
        let pending_insert = std::mem::take(&mut self.pending_insert);
        for pos in pending_insert {
            self._insert_top_level_inner(pos, source);
        }

        // 处理移除（不再需要 tracker，清理仅释放内部节点数据）
        let pending_remove = std::mem::take(&mut self.pending_remove);
        for pos in pending_remove {
            self._remove_top_level_inner(pos);
        }
    }

    fn _insert_top_level_inner(&mut self, pos: u64, source: &dyn VoxelSource) {
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

        // 启动叶节点请求：从 VoxelSource 读取体素数据展开 octree
        self.request_leaf_node(node_id, source);
    }

    fn _remove_top_level_inner(&mut self, pos: u64) {
        let node_id = match self.pos_to_id.remove(&pos) {
            Some(id) => id,
            None => return,
        };

        self.top_level_ids.retain(|&id| id != node_id);
        self.recurse_remove_node(node_id);
        self.mark_generation_dirty();
    }

    // ===== 递归节点管理 =====

    /// 递归移除节点及其子节点。
    ///
    /// 不涉及体素数据访问 — 仅释放 NodeStore 中的节点槽位。
    fn recurse_remove_node(&mut self, node_id: u32) {
        let node_type = self.store.get_node_type(node_id);
        let pos = self.store.node_position(node_id);
        let lvl = decode_level(pos);

        if node_type == NodeType::Inner {
            let child_ptr = self.store.get_child_ptr(node_id);
            let mask = self.store.get_node_child_existence(node_id);
            // 递归移除所有存在的子节点
            for i in 0..8 {
                if (mask & (1 << i)) != 0 {
                    let child_id = child_ptr as u32 + i;
                    self.pos_to_id.remove(&make_child_pos(pos, i));
                    self.recurse_remove_node(child_id);
                }
            }
            self.store.free_contiguous(child_ptr as u32, 8);
        } else if node_type == NodeType::Leaf {
            self.clear_geometry(node_id);
        }

        self.store.free(node_id);
    }

    /// 请求展开叶节点，递归构建子节点直到 LOD=0。
    ///
    /// 优化：先检查 8 个子节点的内容，只有非空时才分配写入，
    /// 避免"先分配再回退"浪费 NodeStore 槽位。
    ///
    /// Plan A：`source` 从 LoadedChunks 读取 ChunkData。
    /// Plan B：`source` 从 GPU voxel buffer 回读。
    fn request_leaf_node(&mut self, node_id: u32, source: &dyn VoxelSource) {
        let pos = self.store.node_position(node_id);
        let lvl = decode_level(pos);

        self.store.mark_request_in_flight(node_id);

        if lvl == 0 {
            // LOD=0: 直接从 VoxelSource 查询对应 chunk 是否有内容
            let nx = decode_x(pos);
            let ny = decode_y(pos);
            let nz = decode_z(pos);
            let has_content = source.region_has_content(0, nx, ny, nz);

            if !has_content {
                // 空区域 → 空几何体叶子
                self.store.set_node_type(node_id, NodeType::Leaf);
                self.store.set_node_geometry(node_id, -2); // -2 = 空几何体
                self.store.unmark_request_in_flight(node_id);
                self.mark_node_dirty(node_id);
                return;
            }

            // 有内容 → 叶子节点，保守设置 child_existence = 0xFF
            // （不深入 chunk 内部计算 octant 级掩码）
            self.store.set_node_type(node_id, NodeType::Leaf);
            self.store.set_node_child_existence(node_id, 0xFF);
            self.store.unmark_request_in_flight(node_id);
            self.mark_node_dirty(node_id);
        } else {
            // LOD>0: 先检查子节点内容，再分配
            let mut existence_mask: u8 = 0;
            let mut child_has_content = [false; 8];
            for i in 0..8u32 {
                let child_pos = make_child_pos(pos, i);
                let cx = decode_x(child_pos);
                let cy = decode_y(child_pos);
                let cz = decode_z(child_pos);
                let has_content = self.check_region_has_content(lvl - 1, cx, cy, cz, source);
                child_has_content[i as usize] = has_content;
                if has_content {
                    existence_mask |= 1u8 << (i as u8);
                }
            }

            // 所有子节点都为空 → 本节点为空叶子，不继续展开
            if existence_mask == 0 {
                self.store.set_node_type(node_id, NodeType::Leaf);
                self.store.set_node_geometry(node_id, -2); // -2 = 空几何体
                self.store.unmark_request_in_flight(node_id);
                self.mark_node_dirty(node_id);
                return;
            }

            // 分配 8 个连续子节点槽位
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

            // 只写入非空子节点（空子节点保持 None 避免 HashMap 污染）
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

            // 递归展开非空子节点
            for i in 0..8u32 {
                if child_has_content[i as usize] {
                    let child_id = child_base + i;
                    self.request_leaf_node(child_id, source);
                }
            }

            self.store.unmark_request_in_flight(node_id);
            self.mark_node_dirty(node_id);
        }
    }

    /// 检查区域内是否有体素内容。
    ///
    /// 两级策略：
    /// - LOD=0：委托给 `VoxelSource`（Plan A 查 LoadedChunks，Plan B 查 GPU buffer）。
    /// - LOD>0：检查 pos_to_id 中是否已有该区域任一子节点，
    ///   有则说明该区域之前被判定为有内容，无需重复 VoxelSource 查询。
    fn check_region_has_content(
        &self,
        lvl: u32,
        x: i32,
        y: i32,
        z: i32,
        source: &dyn VoxelSource,
    ) -> bool {
        if lvl == 0 {
            // 直接查询体素数据源
            source.region_has_content(0, x, y, z)
        } else {
            // 检查 pos_to_id 中是否有已存在的子节点
            for oz in 0..2 {
                for ox in 0..2 {
                    for oy in 0..2 {
                        let child_pos = crate::svo::encode_position(
                            lvl - 1,
                            x * 2 + ox,
                            y * 2 + oy,
                            z * 2 + oz,
                        );
                        if self.pos_to_id.contains_key(&child_pos) {
                            return true;
                        }
                    }
                }
            }
            // pos_to_id 中无记录 → 递归查询 VoxelSource
            source.region_has_content(lvl, x, y, z)
        }
    }

    /// 清除节点几何体数据，重置为默认状态。
    fn clear_geometry(&mut self, node_id: u32) {
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
