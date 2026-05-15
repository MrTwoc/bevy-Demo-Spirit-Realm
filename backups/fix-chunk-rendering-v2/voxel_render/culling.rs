//! GPU 端视锥体剔除模块
//!
//! 使用 Compute Shader 执行视锥体测试，生成可见区块列表。
//!
//! # 实现方案
//!
//! 1. 上传区块元数据到 GPU
//! 2. 执行 Compute Shader 进行视锥体测试
//! 3. 读取可见区块列表
//! 4. 更新 Indirect 命令
//!
//! # 注意事项
//!
//! 当前实现为占位框架，实际的 Compute Shader 集成需要：
//! - 创建 Compute Pipeline
//! - 创建 BindGroup
//! - 执行 Dispatch
//! - 读取结果

use bevy::prelude::*;

use super::buffers::{ChunkBufferRegion, VoxelBuffers};

/// 区块元数据（与 GPU 端对齐）
#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct ChunkMetadata {
    /// 区块世界坐标
    pub position: [f32; 3],
    /// 包围球半径
    pub bounding_radius: f32,
    /// 顶点数量
    pub vertex_count: u32,
    /// 索引数量
    pub index_count: u32,
    /// 顶点偏移
    pub vertex_offset: u32,
    /// 索引偏移
    pub index_offset: u32,
}

/// 视锥体平面
#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct FrustumPlane {
    /// 法线
    pub normal: [f32; 3],
    /// 距离
    pub distance: f32,
}

/// 视锥体（6个平面）
#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct Frustum {
    /// 6个平面
    pub planes: [FrustumPlane; 6],
}

/// GPU 剔除资源
#[derive(Resource)]
pub struct GpuCullingResources {
    /// 区块元数据
    pub chunk_metadata: Vec<ChunkMetadata>,
    /// 是否需要更新
    pub dirty: bool,
}

impl Default for GpuCullingResources {
    fn default() -> Self {
        Self {
            chunk_metadata: Vec::new(),
            dirty: false,
        }
    }
}

/// 更新区块元数据
pub fn update_chunk_metadata(
    buffers: Res<VoxelBuffers>,
    mut culling_resources: ResMut<GpuCullingResources>,
) {
    if !buffers.dirty {
        return;
    }

    culling_resources.chunk_metadata.clear();

    for (coord, region) in &buffers.chunk_regions {
        let world_pos = coord.to_world_origin();
        let bounding_radius = calculate_bounding_radius(region);

        culling_resources.chunk_metadata.push(ChunkMetadata {
            position: [world_pos.x, world_pos.y, world_pos.z],
            bounding_radius,
            vertex_count: region.vertex_count,
            index_count: region.index_count,
            vertex_offset: region.vertex_offset,
            index_offset: region.index_offset,
        });
    }

    culling_resources.dirty = true;
}

/// 计算包围球半径
fn calculate_bounding_radius(region: &ChunkBufferRegion) -> f32 {
    // 区块大小为 32x32x32，对角线长度约为 55.4
    // 使用 32.0 作为保守估计
    32.0
}

/// 从相机提取视锥体
pub fn extract_frustum(camera: &Camera, transform: &GlobalTransform) -> Frustum {
    // 简化实现：返回一个足够大的视锥体
    // 实际应该从相机矩阵中提取6个平面
    Frustum {
        planes: [
            FrustumPlane { normal: [1.0, 0.0, 0.0], distance: 1000.0 },
            FrustumPlane { normal: [-1.0, 0.0, 0.0], distance: 1000.0 },
            FrustumPlane { normal: [0.0, 1.0, 0.0], distance: 1000.0 },
            FrustumPlane { normal: [0.0, -1.0, 0.0], distance: 1000.0 },
            FrustumPlane { normal: [0.0, 0.0, 1.0], distance: 1000.0 },
            FrustumPlane { normal: [0.0, 0.0, -1.0], distance: 1000.0 },
        ],
    }
}

/// GPU 剔除插件
pub struct GpuCullingPlugin;

impl Plugin for GpuCullingPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GpuCullingResources>();

        app.add_systems(
            Update,
            update_chunk_metadata.after(super::plugin::update_voxel_render_state),
        );
    }
}
