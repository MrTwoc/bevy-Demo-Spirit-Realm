//! 扁平节点存储 (NodeStore)
//!
//! 移植自 Voxy 的 NodeStore 实现：
//! - 预分配的扁平数组，每个节点 32 字节 (4 × u64)
//! - 使用 HierarchicalBitSet 进行高效的位分配
//! - 支持几何体指针、子节点指针、请求管理、标志位
//!
//! 节点数据布局：
//! - word0 (u64): position encoding
//! - word1 (u64): geometry_handle(24) | child_ptr(24) | flags(8) | child_existence(8)
//! - word2 (u64): request_id(19) | all_children_are_leaf(1) | spare(44)
//! - word3 (u64): 备用

use crate::svo::hierarchical_bitset::{HierarchicalBitSet, SET_FULL};

/// 常量定义
pub const NODE_ID_MASK: u32 = (1 << 24) - 1;
pub const GEOMETRY_ID_MASK: u32 = (1 << 24) - 1;
pub const MAX_GEOMETRY_ID: u32 = (1 << 24) - 3;
pub const REQUEST_ID_MASK: u32 = (1 << 19) - 1;

/// 哨兵值
const SENTINEL_NULL_GEOMETRY_ID: u32 = (1 << 24) - 1;
const SENTINEL_EMPTY_GEOMETRY_ID: u32 = (1 << 24) - 2;
const SENTINEL_NULL_NODE_ID: u32 = NODE_ID_MASK;

/// 每个节点的 u64 数量
const LONGS_PER_NODE: usize = 4;

/// 动态扩容的增量大小
const INCREMENT_SIZE: u32 = 1 << 16;

/// 节点类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum NodeType {
    /// 空节点 (不可用)
    None = 0,
    /// 叶子节点: 含有几何体数据
    Leaf = 1,
    /// 内部节点: 含有子节点指针
    Inner = 2,
    /// 请求中: 几何体生成中
    Pending = 3,
}

/// 扁平节点存储
pub struct NodeStore {
    /// 节点数据数组 (u64 × 4 per node)
    data: Vec<u64>,
    /// 分配器
    allocation_set: HierarchicalBitSet,
}

impl NodeStore {
    /// 创建新的节点存储
    ///
    /// # Arguments
    /// * `max_node_count` - 最大节点数量，不能超过 2^24
    pub fn new(max_node_count: u32) -> Self {
        assert!(
            max_node_count < NODE_ID_MASK,
            "Max count too large"
        );

        let initial_size = INCREMENT_SIZE as usize;
        Self {
            data: vec![0u64; initial_size * LONGS_PER_NODE],
            allocation_set: HierarchicalBitSet::new(max_node_count),
        }
    }

    /// 将节点 ID 转换为数组索引
    #[inline]
    fn id_to_idx(node_id: u32) -> usize {
        node_id as usize * LONGS_PER_NODE
    }

    /// 分配一个新节点，返回节点 ID
    pub fn allocate(&mut self) -> Option<u32> {
        let id = self.allocation_set.allocate_next();
        if id == SET_FULL {
            return None;
        }
        self.ensure_sized(id);
        self.clear(id);
        Some(id)
    }

    /// 分配连续的 N 个节点，返回起始节点 ID
    pub fn allocate_contiguous(&mut self, count: u32) -> Option<u32> {
        if count == 0 {
            return None;
        }

        let id = self.allocation_set.allocate_next_consecutive(count);
        if id == SET_FULL {
            return None;
        }
        self.ensure_sized(id + count - 1);
        for i in 0..count {
            self.clear(id + i);
        }
        Some(id)
    }

    /// 确保数组大小足够
    fn ensure_sized(&mut self, index: u32) {
        let required = (index + 1) as usize * LONGS_PER_NODE;
        if required > self.data.len() {
            let new_size = ((index + INCREMENT_SIZE).min(self.allocation_set.limit())) as usize;
            let new_len = new_size * LONGS_PER_NODE;
            self.data.resize(new_len, 0);
        }
    }

    /// 清除节点数据
    fn clear(&mut self, node_id: u32) {
        let idx = Self::id_to_idx(node_id);
        self.data[idx] = u64::MAX; // Position = -1
        self.data[idx + 1] = GEOMETRY_ID_MASK as u64 | ((NODE_ID_MASK as u64) << 24);
        self.data[idx + 2] = REQUEST_ID_MASK as u64;
        self.data[idx + 3] = 0;
    }

    /// 释放节点
    pub fn free(&mut self, node_id: u32) {
        self.free_contiguous(node_id, 1);
    }

    /// 释放连续的节点
    pub fn free_contiguous(&mut self, base_node_id: u32, count: u32) {
        for i in 0..count {
            let node_id = base_node_id + i;
            if !self.allocation_set.free(node_id) {
                // 节点未分配，跳过
                continue;
            }
            self.clear(node_id);
        }
    }

    /// 复制节点数据
    pub fn copy_node(&mut self, from_id: u32, to_id: u32) {
        assert!(
            self.node_exists(from_id) && self.node_exists(to_id),
            "Both nodes must be allocated"
        );
        let f = Self::id_to_idx(from_id);
        let t = Self::id_to_idx(to_id);
        self.data[t] = self.data[f];
        self.data[t + 1] = self.data[f + 1];
        self.data[t + 2] = self.data[f + 2];
        self.data[t + 3] = self.data[f + 3];
    }

    /// 设置节点位置
    pub fn set_node_position(&mut self, node_id: u32, position: u64) {
        let idx = Self::id_to_idx(node_id);
        self.data[idx] = position;
    }

    /// 获取节点位置
    pub fn node_position(&self, node_id: u32) -> u64 {
        self.data[Self::id_to_idx(node_id)]
    }

    /// 检查节点是否存在
    pub fn node_exists(&self, node_id: u32) -> bool {
        self.allocation_set.is_set(node_id)
    }

    /// 获取几何体指针
    ///
    /// 返回值：
    /// - -1: 无几何体 (null)
    /// - -2: 空几何体 (empty)
    /// - >= 0: 有效的几何体 ID
    pub fn get_node_geometry(&self, node_id: u32) -> i32 {
        let raw = self.data[Self::id_to_idx(node_id) + 1];
        let geometry_ptr = (raw & GEOMETRY_ID_MASK as u64) as u32;

        if geometry_ptr == SENTINEL_NULL_GEOMETRY_ID {
            -1
        } else if geometry_ptr == SENTINEL_EMPTY_GEOMETRY_ID {
            -2
        } else {
            geometry_ptr as i32
        }
    }

    /// 设置几何体指针
    ///
    /// # Arguments
    /// * `geometry_id`:
    ///   - -1: 设置为 null
    ///   - -2: 设置为 empty
    ///   - >= 0: 设置为有效的几何体 ID
    pub fn set_node_geometry(&mut self, node_id: u32, geometry_id: i32) {
        let sentinel = match geometry_id {
            -1 => SENTINEL_NULL_GEOMETRY_ID,
            -2 => SENTINEL_EMPTY_GEOMETRY_ID,
            id if id >= 0 && id as u32 <= MAX_GEOMETRY_ID => id as u32,
            _ => panic!("Invalid geometry ID: {}", geometry_id),
        };

        let idx = Self::id_to_idx(node_id) + 1;
        let raw = self.data[idx];
        self.data[idx] = (raw & !(GEOMETRY_ID_MASK as u64)) | sentinel as u64;
    }

    /// 获取子节点指针
    ///
    /// 返回值：
    /// - -1: 无子节点 (null)
    /// - >= 0: 子节点数组的起始 ID
    pub fn get_child_ptr(&self, node_id: u32) -> i32 {
        let raw = self.data[Self::id_to_idx(node_id) + 1];
        let node_ptr = ((raw >> 24) & NODE_ID_MASK as u64) as u32;

        if node_ptr == SENTINEL_NULL_NODE_ID {
            -1
        } else {
            node_ptr as i32
        }
    }

    /// 设置子节点指针
    ///
    /// # Arguments
    /// * `ptr`:
    ///   - -1: 设置为 null
    ///   - >= 0: 设置为子节点数组的起始 ID
    pub fn set_child_ptr(&mut self, node_id: u32, ptr: i32) {
        let sentinel = if ptr == -1 {
            SENTINEL_NULL_NODE_ID
        } else {
            assert!(ptr >= 0 && (ptr as u32) < NODE_ID_MASK, "Invalid child ptr: {}", ptr);
            ptr as u32
        };

        let idx = Self::id_to_idx(node_id) + 1;
        let raw = self.data[idx];
        self.data[idx] = (raw & !((NODE_ID_MASK as u64) << 24)) | ((sentinel as u64) << 24);
    }

    /// 设置请求 ID
    pub fn set_node_request(&mut self, node_id: u32, request_id: u32) {
        assert!(
            request_id <= REQUEST_ID_MASK,
            "Too many requests to happen at once!"
        );

        let idx = Self::id_to_idx(node_id) + 2;
        let raw = self.data[idx];
        self.data[idx] = (raw & !(REQUEST_ID_MASK as u64)) | request_id as u64;
    }

    /// 获取请求 ID
    pub fn get_node_request(&self, node_id: u32) -> u32 {
        let raw = self.data[Self::id_to_idx(node_id) + 2];
        (raw & REQUEST_ID_MASK as u64) as u32
    }

    /// 标记请求正在处理中
    pub fn mark_request_in_flight(&mut self, node_id: u32) {
        let idx = Self::id_to_idx(node_id) + 1;
        self.data[idx] |= 1u64 << 63;
    }

    /// 取消标记请求正在处理中
    pub fn unmark_request_in_flight(&mut self, node_id: u32) {
        let idx = Self::id_to_idx(node_id) + 1;
        self.data[idx] &= !(1u64 << 63);
    }

    /// 检查请求是否正在处理中
    pub fn is_node_request_in_flight(&self, node_id: u32) -> bool {
        let raw = self.data[Self::id_to_idx(node_id) + 1];
        (raw >> 63) & 1 != 0
    }

    /// 设置所有子节点都是叶子节点的标志
    pub fn set_all_children_are_leaf(&mut self, node_id: u32, state: bool) {
        let idx = Self::id_to_idx(node_id) + 2;
        if state {
            self.data[idx] |= 1u64 << 19;
        } else {
            self.data[idx] &= !(1u64 << 19);
        }
    }

    /// 检查所有子节点是否都是叶子节点
    pub fn get_all_children_are_leaf(&self, node_id: u32) -> bool {
        let raw = self.data[Self::id_to_idx(node_id) + 2];
        (raw >> 19) & 1 != 0
    }

    /// 标记几何体正在生成中
    pub fn mark_node_geometry_in_flight(&mut self, node_id: u32) {
        let idx = Self::id_to_idx(node_id) + 1;
        self.data[idx] |= 1u64 << 59;
    }

    /// 取消标记几何体正在生成中
    pub fn unmark_node_geometry_in_flight(&mut self, node_id: u32) {
        let idx = Self::id_to_idx(node_id) + 1;
        self.data[idx] &= !(1u64 << 59);
    }

    /// 检查几何体是否正在生成中
    pub fn is_node_geometry_in_flight(&self, node_id: u32) -> bool {
        let raw = self.data[Self::id_to_idx(node_id) + 1];
        (raw >> 59) & 1 != 0
    }

    /// 标记节点为脏（数据待更新）
    pub fn set_dirty(&mut self, node_id: u32, dirty: bool) {
        let idx = Self::id_to_idx(node_id) + 2;
        if dirty {
            self.data[idx] |= 1u64 << 20; // 使用 bit 20 作为 dirty 标志
        } else {
            self.data[idx] &= !(1u64 << 20);
        }
    }

    /// 检查节点是否为脏
    pub fn is_dirty(&self, node_id: u32) -> bool {
        let raw = self.data[Self::id_to_idx(node_id) + 2];
        (raw >> 20) & 1 != 0
    }

    /// 获取节点类型
    pub fn get_node_type(&self, node_id: u32) -> NodeType {
        let raw = self.data[Self::id_to_idx(node_id) + 1];
        let t = ((raw >> 61) & 3) as u32;
        match t {
            1 => NodeType::Leaf,
            2 => NodeType::Inner,
            3 => NodeType::Pending,
            _ => NodeType::None,
        }
    }

    /// 设置节点类型
    pub fn set_node_type(&mut self, node_id: u32, node_type: NodeType) {
        let idx = Self::id_to_idx(node_id) + 1;
        let raw = self.data[idx];
        self.data[idx] = (raw & !(3u64 << 61)) | ((node_type as u64) << 61);
    }

    /// 获取子节点存在性掩码 (8-bit)
    pub fn get_node_child_existence(&self, node_id: u32) -> u8 {
        let raw = self.data[Self::id_to_idx(node_id) + 1];
        ((raw >> 48) & 0xFF) as u8
    }

    /// 设置子节点存在性掩码
    pub fn set_node_child_existence(&mut self, node_id: u32, existence: u8) {
        let idx = Self::id_to_idx(node_id) + 1;
        let raw = self.data[idx];
        self.data[idx] = (raw & !(0xFFu64 << 48)) | ((existence as u64) << 48);
    }

    /// 获取子节点指针数量
    pub fn get_child_ptr_count(&self, node_id: u32) -> u32 {
        let raw = self.data[Self::id_to_idx(node_id) + 1];
        ((raw >> 56) & 0x7) as u32 + 1
    }

    /// 设置子节点指针数量
    pub fn set_child_ptr_count(&mut self, node_id: u32, count: u32) {
        assert!(count > 0 && count <= 8, "Invalid count: {}", count);
        let idx = Self::id_to_idx(node_id) + 1;
        let raw = self.data[idx];
        self.data[idx] = (raw & !(7u64 << 56)) | (((count - 1) as u64) << 56);
    }

    /// 将节点数据写入到 GPU 可读的紧凑格式
    ///
    /// # Arguments
    /// * `node_id` - 节点 ID
    ///
    /// # Returns
    /// 返回 16 字节的紧凑节点数据 (2 × u64)
    pub fn write_node_compact(&self, node_id: u32) -> (u64, u64) {
        if !self.node_exists(node_id) {
            return (u64::MAX, u64::MAX);
        }

        let pos = self.node_position(node_id);

        // 构建 flags
        let mut flags: u16 = 0;
        flags |= if self.is_node_request_in_flight(node_id) { 1 } else { 0 };
        flags |= ((self.get_child_ptr_count(node_id) - 1) as u16) << 2;

        let mut is_eligible_for_cleaning = false;
        is_eligible_for_cleaning |= self.get_all_children_are_leaf(node_id);
        flags |= if is_eligible_for_cleaning { 1 << 5 } else { 0 };

        // 构建 geometry 和 child_ptr
        let geometry = self.get_node_geometry(node_id);
        let mut z: u32 = 0;
        match geometry {
            -2 => z |= (1 << 24) - 2, // EMPTY
            -1 => z |= (1 << 24) - 1, // NULL
            id => z |= (id as u32) & 0xFFFFFF,
        }

        let child_ptr = self.get_child_ptr(node_id);
        let mut w: u32 = 0;
        w |= (child_ptr as u32) & 0xFFFFFF;

        z |= ((flags as u32) & 0xFF) << 24;
        w |= (((flags >> 8) as u32) & 0xFF) << 24;

        (pos, ((w as u64) << 32) | z as u64)
    }

    /// 获取节点数量
    pub fn node_count(&self) -> u32 {
        self.allocation_set.count()
    }

    /// 获取最大节点 ID
    pub fn end_node_id(&self) -> i32 {
        self.allocation_set.max_index()
    }

    /// 获取原始数据（用于 GPU 上传）
    pub fn raw_data(&self) -> &[u64] {
        let max_idx = (self.allocation_set.max_index() as usize + 1) * LONGS_PER_NODE;
        &self.data[..max_idx.min(self.data.len())]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_allocation() {
        let mut store = NodeStore::new(1024);

        // 分配节点
        let id = store.allocate().unwrap();
        assert!(store.node_exists(id));
        assert_eq!(store.node_count(), 1);

        // 释放节点
        store.free(id);
        assert!(!store.node_exists(id));
        assert_eq!(store.node_count(), 0);
    }

    #[test]
    fn test_position_encoding() {
        let mut store = NodeStore::new(1024);
        let id = store.allocate().unwrap();

        // 设置位置
        let position = 0x1234567890ABCDEF;
        store.set_node_position(id, position);

        // 读取位置
        assert_eq!(store.node_position(id), position);
    }

    #[test]
    fn test_geometry_handle() {
        let mut store = NodeStore::new(1024);
        let id = store.allocate().unwrap();

        // 设置几何体
        store.set_node_geometry(id, 42);
        assert_eq!(store.get_node_geometry(id), 42);

        // 设置为 null
        store.set_node_geometry(id, -1);
        assert_eq!(store.get_node_geometry(id), -1);

        // 设置为 empty
        store.set_node_geometry(id, -2);
        assert_eq!(store.get_node_geometry(id), -2);
    }

    #[test]
    fn test_child_ptr() {
        let mut store = NodeStore::new(1024);
        let id = store.allocate().unwrap();

        // 设置子节点指针
        store.set_child_ptr(id, 100);
        assert_eq!(store.get_child_ptr(id), 100);

        // 设置为 null
        store.set_child_ptr(id, -1);
        assert_eq!(store.get_child_ptr(id), -1);
    }

    #[test]
    fn test_node_type() {
        let mut store = NodeStore::new(1024);
        let id = store.allocate().unwrap();

        // 设置节点类型
        store.set_node_type(id, NodeType::Leaf);
        assert_eq!(store.get_node_type(id), NodeType::Leaf);

        store.set_node_type(id, NodeType::Inner);
        assert_eq!(store.get_node_type(id), NodeType::Inner);
    }

    #[test]
    fn test_flags() {
        let mut store = NodeStore::new(1024);
        let id = store.allocate().unwrap();

        // 测试 in-flight 标志
        store.mark_request_in_flight(id);
        assert!(store.is_node_request_in_flight(id));
        store.unmark_request_in_flight(id);
        assert!(!store.is_node_request_in_flight(id));

        // 测试 geometry in-flight 标志
        store.mark_node_geometry_in_flight(id);
        assert!(store.is_node_geometry_in_flight(id));
        store.unmark_node_geometry_in_flight(id);
        assert!(!store.is_node_geometry_in_flight(id));

        // 测试 all_children_are_leaf 标志
        store.set_all_children_are_leaf(id, true);
        assert!(store.get_all_children_are_leaf(id));
        store.set_all_children_are_leaf(id, false);
        assert!(!store.get_all_children_are_leaf(id));
    }

    #[test]
    fn test_child_existence() {
        let mut store = NodeStore::new(1024);
        let id = store.allocate().unwrap();

        // 设置子节点存在性掩码
        store.set_node_child_existence(id, 0b10101010);
        assert_eq!(store.get_node_child_existence(id), 0b10101010);
    }

    #[test]
    fn test_child_ptr_count() {
        let mut store = NodeStore::new(1024);
        let id = store.allocate().unwrap();

        // 设置子节点数量
        store.set_child_ptr_count(id, 5);
        assert_eq!(store.get_child_ptr_count(id), 5);
    }

    #[test]
    fn test_request_id() {
        let mut store = NodeStore::new(1024);
        let id = store.allocate().unwrap();

        // 设置请求 ID
        store.set_node_request(id, 12345);
        assert_eq!(store.get_node_request(id), 12345);
    }

    #[test]
    fn test_write_node_compact() {
        let mut store = NodeStore::new(1024);
        let id = store.allocate().unwrap();

        // 设置一些数据
        store.set_node_position(id, 0x1234567890ABCDEF);
        store.set_node_geometry(id, 42);
        store.set_child_ptr(id, 100);

        // 写入紧凑格式
        let (pos, data) = store.write_node_compact(id);

        // 验证位置
        assert_eq!(pos, 0x1234567890ABCDEF);

        // 验证几何体指针
        let geometry = (data & 0xFFFFFF) as u32;
        assert_eq!(geometry, 42);

        // 验证子节点指针
        let child_ptr = ((data >> 32) & 0xFFFFFF) as u32;
        assert_eq!(child_ptr, 100);
    }

    #[test]
    fn test_contiguous_allocation() {
        let mut store = NodeStore::new(1024);

        // 分配连续的节点
        let base = store.allocate_contiguous(8).unwrap();
        assert_eq!(store.node_count(), 8);

        // 验证所有节点都存在
        for i in 0..8 {
            assert!(store.node_exists(base + i));
        }

        // 释放连续节点
        store.free_contiguous(base, 8);
        assert_eq!(store.node_count(), 0);
    }

    #[test]
    fn test_copy_node() {
        let mut store = NodeStore::new(1024);

        let id1 = store.allocate().unwrap();
        let id2 = store.allocate().unwrap();

        // 设置第一个节点的数据
        store.set_node_position(id1, 0x1234567890ABCDEF);
        store.set_node_geometry(id1, 42);
        store.set_child_ptr(id1, 100);

        // 复制到第二个节点
        store.copy_node(id1, id2);

        // 验证数据一致
        assert_eq!(store.node_position(id2), 0x1234567890ABCDEF);
        assert_eq!(store.get_node_geometry(id2), 42);
        assert_eq!(store.get_child_ptr(id2), 100);
    }
}
