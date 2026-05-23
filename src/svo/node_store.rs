//! 扁平节点存储 (NodeStore)
//!
//! 参考 voxy-dev NodeStore:
//! - 预分配的扁平数组 (u64 × 2 per node)
//! - 每个节点 16 字节: pos(u64) + data(u64)
//! - 节点 ID = 数组索引
//! - 支持分配/释放/移动
//!
//! GPU 作为 SSBO 直接绑定读取

use crate::svo::config::MAX_NODES;

/// 节点类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
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

/// 节点标志位
const FLAG_TYPE_MASK: u8 = 0x03;
const FLAG_IN_FLIGHT: u8 = 0x04;   // 几何体正在生成
const FLAG_DIRTY: u8 = 0x08;       // 节点数据待更新
const FLAG_ALL_CHILDREN_LEAF: u8 = 0x10; // 所有子节点都是叶子

/// 节点数据布局 (每节点 16 字节)
///
/// word0 (u64): position encoding
/// word1 (u64):
///   bits 0-23: geometry_handle (或 child_ptr)
///   bits 24-31: flags (NodeType + metadata)
///   bits 32-55: child_existence_mask (8-bit) + spare
///   bits 56-63: 备用
#[derive(Debug, Clone)]
pub struct NodeEntry {
    pub position: u64,
    pub geometry_handle: u32,   // 或 child_ptr (低 24 位)
    pub flags: u8,
    pub child_existence_mask: u8, // 8-bit octant mask
}

/// 扁平节点存储
pub struct NodeStore {
    /// 节点数据数组 (u64 × 2 per node)
    data: Vec<u64>,
    /// 空闲节点索引栈
    free_list: Vec<u32>,
    /// 当前最大已用 ID
    max_id: u32,
    /// 节点数量
    count: u32,
}

impl NodeStore {
    pub fn new() -> Self {
        let capacity = MAX_NODES;
        Self {
            data: vec![0u64; capacity * 2],
            free_list: Vec::with_capacity(1024),
            max_id: 0,
            count: 0,
        }
    }

    /// 分配一个新节点，返回节点 ID
    pub fn allocate(&mut self, position: u64, node_type: NodeType) -> Option<u32> {
        let id = if let Some(free_id) = self.free_list.pop() {
            free_id
        } else {
            if self.max_id >= MAX_NODES as u32 {
                return None; // 节点池满了
            }
            let id = self.max_id;
            self.max_id += 1;
            id
        };

        self.count += 1;
        self.write_node(id, position, node_type);
        Some(id)
    }

    /// 释放节点
    pub fn free(&mut self, node_id: u32) {
        if !self.node_exists(node_id) {
            return;
        }
        let idx = node_id as usize * 2;
        self.data[idx] = 0;
        self.data[idx + 1] = 0;
        self.free_list.push(node_id);
        self.count -= 1;
    }

    /// 批量分配连续的节点 (用于子节点数组)
    pub fn allocate_contiguous(&mut self, count: u32) -> Option<u32> {
        if self.max_id + count <= MAX_NODES as u32 {
            let base = self.max_id;
            for i in 0..count {
                let idx = (base + i) as usize * 2;
                self.data[idx] = 0;
                self.data[idx + 1] = 0;
            }
            self.max_id += count;
            self.count += count;
            Some(base)
        } else {
            None
        }
    }

    /// 释放连续节点
    pub fn free_contiguous(&mut self, base_id: u32, count: u32) {
        for i in 0..count {
            let id = base_id + i;
            let idx = id as usize * 2;
            self.data[idx] = 0;
            self.data[idx + 1] = 0;
            self.free_list.push(id);
        }
        self.count -= count;
    }

    /// 读取节点信息
    pub fn read_node(&self, node_id: u32) -> NodeEntry {
        let idx = node_id as usize * 2;
        let pos = self.data[idx];
        let raw = self.data[idx + 1];
        NodeEntry {
            position: pos,
            geometry_handle: (raw & 0xFFFFFF) as u32,
            flags: ((raw >> 24) & 0xFF) as u8,
            child_existence_mask: ((raw >> 32) & 0xFF) as u8,
        }
    }

    /// 写入节点信息
    pub fn write_node(&mut self, node_id: u32, position: u64, node_type: NodeType) {
        let idx = node_id as usize * 2;
        self.data[idx] = position;
        let flags = node_type as u8;
        self.data[idx + 1] = (self.data[idx + 1] & 0xFFFF_FFFF_FFFF_0000) | flags as u64;
    }

    /// 检查节点是否存在
    pub fn node_exists(&self, node_id: u32) -> bool {
        let idx = node_id as usize * 2;
        self.data[idx] != 0 || self.data[idx + 1] != 0
    }

    /// 获取节点类型
    pub fn get_node_type(&self, node_id: u32) -> NodeType {
        let raw = self.data[node_id as usize * 2 + 1];
        let t = ((raw >> 24) & FLAG_TYPE_MASK as u64) as u8;
        match t {
            1 => NodeType::Leaf,
            2 => NodeType::Inner,
            3 => NodeType::Pending,
            _ => NodeType::None,
        }
    }

    /// 设置节点类型
    pub fn set_node_type(&mut self, node_id: u32, node_type: NodeType) {
        let idx = node_id as usize * 2 + 1;
        let raw = self.data[idx];
        self.data[idx] = (raw & !(FLAG_TYPE_MASK as u64)) | ((node_type as u64) << 24);
    }

    /// 获取几何体句柄
    pub fn get_geometry_handle(&self, node_id: u32) -> u32 {
        (self.data[node_id as usize * 2 + 1] & 0xFFFFFF) as u32
    }

    /// 设置几何体句柄
    pub fn set_geometry_handle(&mut self, node_id: u32, handle: u32) {
        let idx = node_id as usize * 2 + 1;
        let raw = self.data[idx];
        self.data[idx] = (raw & !0xFFFFFF) | (handle as u64 & 0xFFFFFF);
    }

    /// 获取子节点指针 (base ID of 8 children)
    pub fn get_child_ptr(&self, node_id: u32) -> u32 {
        self.get_geometry_handle(node_id)
    }

    /// 设置子节点指针
    pub fn set_child_ptr(&mut self, node_id: u32, ptr: u32) {
        self.set_geometry_handle(node_id, ptr);
    }

    /// 获取子节点存在性掩码 (8-bit)
    pub fn get_child_existence(&self, node_id: u32) -> u8 {
        ((self.data[node_id as usize * 2 + 1] >> 32) & 0xFF) as u8
    }

    /// 设置子节点存在性掩码
    pub fn set_child_existence(&mut self, node_id: u32, mask: u8) {
        let idx = node_id as usize * 2 + 1;
        let raw = self.data[idx];
        self.data[idx] = (raw & !(0xFF << 32)) | ((mask as u64) << 32);
    }

    /// 标记/清除 in-flight 标志
    pub fn set_in_flight(&mut self, node_id: u32, in_flight: bool) {
        let idx = node_id as usize * 2 + 1;
        let raw = self.data[idx];
        if in_flight {
            self.data[idx] = raw | ((FLAG_IN_FLIGHT as u64) << 24);
        } else {
            self.data[idx] = raw & !((FLAG_IN_FLIGHT as u64) << 24);
        }
    }

    pub fn is_in_flight(&self, node_id: u32) -> bool {
        let raw = self.data[node_id as usize * 2 + 1];
        ((raw >> 24) & FLAG_IN_FLIGHT as u64) != 0
    }

    /// 标记/清除 dirty 标志
    pub fn set_dirty(&mut self, node_id: u32, dirty: bool) {
        let idx = node_id as usize * 2 + 1;
        let raw = self.data[idx];
        if dirty {
            self.data[idx] = raw | ((FLAG_DIRTY as u64) << 24);
        } else {
            self.data[idx] = raw & !((FLAG_DIRTY as u64) << 24);
        }
    }

    pub fn is_dirty(&self, node_id: u32) -> bool {
        let raw = self.data[node_id as usize * 2 + 1];
        ((raw >> 24) & FLAG_DIRTY as u64) != 0
    }

    /// 原始数据指针 (用于上传到 GPU)
    pub fn raw_data(&self) -> &[u64] {
        &self.data[..self.max_id as usize * 2]
    }

    /// 当前节点数
    pub fn count(&self) -> u32 {
        self.count
    }

    /// 最大节点 ID
    pub fn max_id(&self) -> u32 {
        self.max_id
    }
}

/// GPU 可读的节点数据格式 (16 bytes)
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuNode {
    /// word0: position encoding
    pub position: u64,
    /// word1: geometry_handle(24) | flags(8) | child_existence(8) | spare(24)
    pub data: u64,
}

impl GpuNode {
    pub fn from_entry(entry: &NodeEntry) -> Self {
        let data = (entry.geometry_handle as u64)
            | ((entry.flags as u64) << 24)
            | ((entry.child_existence_mask as u64) << 32);
        Self {
            position: entry.position,
            data,
        }
    }

    pub fn zeroed() -> Self {
        Self { position: 0, data: 0 }
    }
}
