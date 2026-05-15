//! 缓冲区管理模块
//!
//! 定义渲染系统所需的数据结构。
//! 当前版本：仅定义数据结构，不包含 Buffer 管理逻辑。
//! 后续需要实现 Buffer 创建和上传。

use bevy::prelude::*;

/// 间接绘制命令结构（与GPU端对齐）
///
/// 对应 Vulkan/DX12 的 VkDrawIndexedIndirectCommand
#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct IndirectCommand {
    /// 索引数量（每个区块的三角形数 × 3）
    pub index_count: u32,
    /// 实例数量（固定为1）
    pub instance_count: u32,
    /// 索引起始位置（在全局索引缓冲区中的偏移）
    pub first_index: u32,
    /// 顶点偏移（在全局顶点缓冲区中的偏移）
    pub base_vertex: i32,
    /// 实例起始ID（用于查找区块偏移）
    pub first_instance: u32,
}

/// 区块Mesh数据（CPU端）
///
/// 存储单个区块的Mesh数据，用于上传到全局Buffer
#[derive(Clone, Debug)]
pub struct ChunkMeshData {
    /// 区块坐标
    pub coord: crate::chunk::ChunkCoord,
    /// 顶点位置数据
    pub positions: Vec<[f32; 3]>,
    /// 法线数据
    pub normals: Vec<[f32; 3]>,
    /// UV坐标
    pub uvs: Vec<[f32; 2]>,
    /// 索引数据
    pub indices: Vec<u32>,
    /// LOD级别
    pub lod_level: crate::lod::LodLevel,
}

/// 区块在全局Buffer中的位置信息
#[derive(Clone, Copy, Debug)]
pub struct ChunkBufferRegion {
    /// 顶点起始偏移
    pub vertex_offset: u32,
    /// 顶点数量
    pub vertex_count: u32,
    /// 索引起始偏移
    pub index_offset: u32,
    /// 索引数量
    pub index_count: u32,
}

/// 简单的Buffer分配器
///
/// 使用线性分配策略，当Buffer满时触发压缩
#[derive(Debug)]
pub struct BufferAllocator {
    /// 下一个可用的顶点偏移
    pub next_vertex_offset: u32,
    /// 下一个可用的索引偏移
    pub next_index_offset: u32,
    /// 空闲区域列表（用于碎片整理）
    pub free_regions: Vec<ChunkBufferRegion>,
}

impl Default for BufferAllocator {
    fn default() -> Self {
        Self {
            next_vertex_offset: 0,
            next_index_offset: 0,
            free_regions: Vec::new(),
        }
    }
}

impl BufferAllocator {
    /// 分配一块Buffer区域
    pub fn allocate(&mut self, vertex_count: u32, index_count: u32) -> ChunkBufferRegion {
        // 优先复用空闲区域
        for (i, region) in self.free_regions.iter().enumerate() {
            if region.vertex_count >= vertex_count && region.index_count >= index_count {
                let allocated = ChunkBufferRegion {
                    vertex_offset: region.vertex_offset,
                    vertex_count,
                    index_offset: region.index_offset,
                    index_count,
                };
                // 如果有剩余空间，保留剩余部分
                let remaining_vertex = region.vertex_count - vertex_count;
                let remaining_index = region.index_count - index_count;
                if remaining_vertex > 0 || remaining_index > 0 {
                    self.free_regions[i] = ChunkBufferRegion {
                        vertex_offset: region.vertex_offset + vertex_count,
                        vertex_count: remaining_vertex,
                        index_offset: region.index_offset + index_count,
                        index_count: remaining_index,
                    };
                } else {
                    self.free_regions.swap_remove(i);
                }
                return allocated;
            }
        }

        // 没有合适的空闲区域，线性分配
        let region = ChunkBufferRegion {
            vertex_offset: self.next_vertex_offset,
            vertex_count,
            index_offset: self.next_index_offset,
            index_count,
        };
        self.next_vertex_offset += vertex_count;
        self.next_index_offset += index_count;
        region
    }

    /// 释放一块Buffer区域
    pub fn free(&mut self, region: ChunkBufferRegion) {
        self.free_regions.push(region);
    }
}
