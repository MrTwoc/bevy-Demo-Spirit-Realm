//! 缓冲区管理模块
//!
//! 管理全局顶点缓冲区、索引缓冲区和Indirect命令缓冲区。
//! 所有区块的Mesh数据存储在共享的Storage Buffer中，
//! 通过偏移量访问各个区块的数据。

use std::collections::HashMap;

use bevy::prelude::*;
use bevy::render::render_resource::*;
use bevy::render::renderer::{RenderDevice, RenderQueue};

/// 间接绘制命令结构（与GPU端对齐）
///
/// 对应 Vulkan/DX12 的 VkDrawIndexedIndirectCommand
#[repr(C)]
#[derive(Copy, Clone, Debug, ShaderType)]
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

/// 打包后的顶点数据格式
///
/// 与 WGSL 着色器中的 PackedVertex 结构对齐
#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct PackedVertex {
    pub position: [f32; 3],
    pub normal_encoded: f32,
    pub uv: [f32; 2],
    pub extra: [f32; 2],
}

/// 区块偏移数据
#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct ChunkOffset {
    pub position: [f32; 4], // xyz + padding
}

/// 全局缓冲区资源（Render World）
#[derive(Resource)]
pub struct VoxelBuffers {
    /// 全局顶点缓冲区
    pub vertex_buffer: Option<Buffer>,
    /// 全局索引缓冲区
    pub index_buffer: Option<Buffer>,
    /// Indirect命令缓冲区
    pub indirect_buffer: Option<Buffer>,
    /// 区块偏移缓冲区（世界坐标）
    pub offset_buffer: Option<Buffer>,
    /// 已注册的区块及其Buffer区域
    pub chunk_regions: HashMap<crate::chunk::ChunkCoord, ChunkBufferRegion>,
    /// Buffer分配器（简单线性分配）
    pub allocator: BufferAllocator,
    /// 是否需要更新
    pub dirty: bool,
}

impl Default for VoxelBuffers {
    fn default() -> Self {
        Self {
            vertex_buffer: None,
            index_buffer: None,
            indirect_buffer: None,
            offset_buffer: None,
            chunk_regions: HashMap::new(),
            allocator: BufferAllocator::default(),
            dirty: false,
        }
    }
}

impl VoxelBuffers {
    /// 创建GPU缓冲区
    pub fn create_buffers(&mut self, render_device: &RenderDevice) {
        // 创建顶点缓冲区（Storage Buffer）
        let vertex_buffer_size = (super::config::MAX_CHUNKS
            * super::config::MAX_VERTICES_PER_CHUNK
            * std::mem::size_of::<PackedVertex>()) as u64;
        self.vertex_buffer = Some(render_device.create_buffer(&BufferDescriptor {
            label: Some("voxel_vertex_buffer"),
            size: vertex_buffer_size,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));

        // 创建索引缓冲区（Storage Buffer）
        let index_buffer_size = (super::config::MAX_CHUNKS
            * super::config::MAX_INDICES_PER_CHUNK
            * std::mem::size_of::<u32>()) as u64;
        self.index_buffer = Some(render_device.create_buffer(&BufferDescriptor {
            label: Some("voxel_index_buffer"),
            size: index_buffer_size,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));

        // 创建Indirect命令缓冲区
        let indirect_buffer_size =
            (super::config::MAX_CHUNKS * std::mem::size_of::<IndirectCommand>()) as u64;
        self.indirect_buffer = Some(render_device.create_buffer(&BufferDescriptor {
            label: Some("voxel_indirect_buffer"),
            size: indirect_buffer_size,
            usage: BufferUsages::STORAGE | BufferUsages::INDIRECT | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));

        // 创建偏移缓冲区
        let offset_buffer_size =
            (super::config::MAX_CHUNKS * std::mem::size_of::<ChunkOffset>()) as u64;
        self.offset_buffer = Some(render_device.create_buffer(&BufferDescriptor {
            label: Some("voxel_offset_buffer"),
            size: offset_buffer_size,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));

        info!(
            "VoxelBuffers created: vertex={}, index={}, indirect={}, offset={}",
            vertex_buffer_size, index_buffer_size, indirect_buffer_size, offset_buffer_size
        );
    }

    /// 上传区块Mesh数据到GPU
    pub fn upload_chunk_mesh(&mut self, render_queue: &RenderQueue, mesh_data: &ChunkMeshData) {
        let vertex_count = mesh_data.positions.len() as u32;
        let index_count = mesh_data.indices.len() as u32;

        // 检查是否超过最大区块数量
        if self.chunk_regions.len() >= super::config::MAX_CHUNKS {
            warn!("Maximum chunk count reached, skipping upload");
            return;
        }

        // 分配Buffer区域
        let region = self.allocator.allocate(vertex_count, index_count);

        // 打包顶点数据
        let packed_vertices: Vec<PackedVertex> = (0..vertex_count as usize)
            .map(|i| {
                let face_type = if i < mesh_data.normals.len() {
                    encode_normal(&mesh_data.normals[i])
                } else {
                    0.0
                };
                PackedVertex {
                    position: mesh_data.positions[i],
                    normal_encoded: face_type,
                    uv: if i < mesh_data.uvs.len() {
                        mesh_data.uvs[i]
                    } else {
                        [0.0, 0.0]
                    },
                    extra: [0.0, 0.0],
                }
            })
            .collect();

        // 上传顶点数据
        if let Some(vertex_buffer) = &self.vertex_buffer {
            let byte_offset =
                (region.vertex_offset as usize * std::mem::size_of::<PackedVertex>()) as u64;
            let vertex_bytes = bytemuck::cast_slice(&packed_vertices);
            render_queue.write_buffer(vertex_buffer, byte_offset, vertex_bytes);
        }

        // 调整索引偏移并上传
        let adjusted_indices: Vec<u32> = mesh_data
            .indices
            .iter()
            .map(|&idx| idx + region.vertex_offset)
            .collect();

        if let Some(index_buffer) = &self.index_buffer {
            let byte_offset = (region.index_offset as usize * std::mem::size_of::<u32>()) as u64;
            let index_bytes = bytemuck::cast_slice(&adjusted_indices);
            render_queue.write_buffer(index_buffer, byte_offset, index_bytes);
        }

        // 记录区块的Buffer区域
        self.chunk_regions.insert(mesh_data.coord, region);
        self.dirty = true;
    }

    /// 更新Indirect命令缓冲区
    pub fn update_indirect_buffer(&mut self, render_queue: &RenderQueue) {
        if !self.dirty {
            return;
        }

        let mut commands = Vec::new();
        let mut offsets = Vec::new();

        // 限制命令数量不超过 MAX_CHUNKS
        let max_commands = super::config::MAX_CHUNKS;
        
        for (coord, region) in &self.chunk_regions {
            if commands.len() >= max_commands {
                warn!("Maximum command count reached, truncating");
                break;
            }

            let command_index = commands.len() as u32;

            commands.push(IndirectCommand {
                index_count: region.index_count,
                instance_count: 1,
                first_index: region.index_offset,
                base_vertex: region.vertex_offset as i32,
                first_instance: command_index,
            });

            let world_pos = coord.to_world_origin();
            offsets.push(ChunkOffset {
                position: [world_pos.x, world_pos.y, world_pos.z, 0.0],
            });
        }

        // 上传Indirect命令
        if let Some(indirect_buffer) = &self.indirect_buffer {
            let command_bytes = bytemuck::cast_slice(&commands);
            // 确保不超过缓冲区大小
            let max_bytes = max_commands * std::mem::size_of::<IndirectCommand>();
            let bytes_to_write = command_bytes.len().min(max_bytes);
            render_queue.write_buffer(indirect_buffer, 0, &command_bytes[..bytes_to_write]);
        }

        // 上传偏移数据
        if let Some(offset_buffer) = &self.offset_buffer {
            let offset_bytes = bytemuck::cast_slice(&offsets);
            // 确保不超过缓冲区大小
            let max_bytes = max_commands * std::mem::size_of::<ChunkOffset>();
            let bytes_to_write = offset_bytes.len().min(max_bytes);
            render_queue.write_buffer(offset_buffer, 0, &offset_bytes[..bytes_to_write]);
        }

        self.dirty = false;
    }

    /// 移除区块
    pub fn remove_chunk(&mut self, coord: &crate::chunk::ChunkCoord) {
        if let Some(region) = self.chunk_regions.remove(coord) {
            self.allocator.free(region);
            self.dirty = true;
        }
    }
}

/// 编码法线方向为浮点数
fn encode_normal(normal: &[f32; 3]) -> f32 {
    if normal[0] > 0.5 {
        0.0 // +X
    } else if normal[0] < -0.5 {
        1.0 // -X
    } else if normal[1] > 0.5 {
        2.0 // +Y
    } else if normal[1] < -0.5 {
        3.0 // -Y
    } else if normal[2] > 0.5 {
        4.0 // +Z
    } else if normal[2] < -0.5 {
        5.0 // -Z
    } else {
        0.0
    }
}

/// 使 bytemuck 支持 PackedVertex
unsafe impl bytemuck::Pod for PackedVertex {}
unsafe impl bytemuck::Zeroable for PackedVertex {}

/// 使 bytemuck 支持 ChunkOffset
unsafe impl bytemuck::Pod for ChunkOffset {}
unsafe impl bytemuck::Zeroable for ChunkOffset {}

/// 使 bytemuck 支持 IndirectCommand
unsafe impl bytemuck::Pod for IndirectCommand {}
unsafe impl bytemuck::Zeroable for IndirectCommand {}
