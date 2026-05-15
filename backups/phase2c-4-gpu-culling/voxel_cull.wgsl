// assets/shaders/voxel_cull.wgsl
//
// GPU 端视锥体剔除 Compute Shader
//
// 对每个区块执行视锥体测试，生成可见区块列表。
// 使用原子操作计数可见区块数量。

// ============================================================================
// 数据结构定义
// ============================================================================

// 区块元数据
struct ChunkMetadata {
    position: vec3<f32>,  // 区块世界坐标
    bounding_radius: f32, // 包围球半径
    vertex_count: u32,    // 顶点数量
    index_count: u32,     // 索引数量
    vertex_offset: u32,   // 顶点偏移
    index_offset: u32,    // 索引偏移
}

// 视锥体平面
struct FrustumPlane {
    normal: vec3<f32>,
    distance: f32,
}

// 视锥体（6个平面）
struct Frustum {
    planes: array<FrustumPlane, 6>,
}

// Indirect 命令
struct IndirectCommand {
    index_count: u32,
    instance_count: u32,
    first_index: u32,
    base_vertex: i32,
    first_instance: u32,
}

// ============================================================================
// 绑定组定义
// ============================================================================

@group(0) @binding(0)
var<storage, read> chunk_metadata: array<ChunkMetadata>;

@group(0) @binding(1)
var<storage, read_write> indirect_commands: array<IndirectCommand>;

@group(0) @binding(2)
var<storage, read_write> visible_count: atomic<u32>;

@group(0) @binding(3)
var<uniform> frustum: Frustum;

@group(0) @binding(4)
var<uniform> max_chunks: u32;

// ============================================================================
// 辅助函数
// ============================================================================

// 检查点是否在平面正面
fn is_point_on_positive_side(plane: FrustumPlane, point: vec3<f32>) -> bool {
    return dot(plane.normal, point) + plane.distance > 0.0;
}

// 检查包围球是否与平面相交
fn is_sphere_intersecting_plane(plane: FrustumPlane, center: vec3<f32>, radius: f32) -> bool {
    let distance = dot(plane.normal, center) + plane.distance;
    return distance > -radius;
}

// 检查包围球是否在视锥体内
fn is_sphere_in_frustum(center: vec3<f32>, radius: f32) -> bool {
    // 检查6个平面
    for (var i: u32 = 0u; i < 6u; i++) {
        if !is_sphere_intersecting_plane(frustum.planes[i], center, radius) {
            return false;
        }
    }
    return true;
}

// ============================================================================
// 主 Compute 函数
// ============================================================================

@compute @workgroup_size(64)
fn cull(@builtin(global_invocation_id) id: vec3<u32>) {
    let index = id.x;
    
    // 越界检查
    if index >= max_chunks {
        return;
    }
    
    let metadata = chunk_metadata[index];
    
    // 跳过空区块
    if metadata.vertex_count == 0u || metadata.index_count == 0u {
        return;
    }
    
    // 视锥体剔除
    if !is_sphere_in_frustum(metadata.position, metadata.bounding_radius) {
        return;
    }
    
    // 原子操作分配槽位
    let slot = atomicAdd(&visible_count, 1u);
    
    // 写入 Indirect 命令
    indirect_commands[slot] = IndirectCommand(
        metadata.index_count,
        1u,
        metadata.index_offset,
        i32(metadata.vertex_offset),
        slot,
    );
}
