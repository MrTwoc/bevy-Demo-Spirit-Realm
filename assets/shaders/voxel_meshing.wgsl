// assets/shaders/voxel_meshing.wgsl
//
// GPU 面剔除 Compute Shader
// 输入：32³ 体素数据（Storage Buffer）
// 输出：可见面的顶点和索引（Storage Buffer）
//
// 架构：
// - 每个线程处理一个体素
// - 使用原子操作分配顶点/索引空间
// - 输出格式：顶点打包为 vec4（位置.xyz + face_type），索引为 u32

// ============================================================================
// 常量定义
// ============================================================================

const CHUNK_SIZE: u32 = 32u;
const CHUNK_VOLUME: u32 = CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE;

// 最大面数：32³ 体素，每个最多 6 面
const MAX_FACES_PER_CHUNK: u32 = 32768u * 6u;
// 最大顶点数：每面 4 顶点
const MAX_VERTICES_PER_CHUNK: u32 = MAX_FACES_PER_CHUNK * 4u;
// 最大索引数：每面 2 三角形 = 6 索引
const MAX_INDICES_PER_CHUNK: u32 = MAX_FACES_PER_CHUNK * 6u;

// 面方向偏移量
const FACE_RIGHT: vec3<i32> = vec3<i32>(1, 0, 0);
const FACE_LEFT: vec3<i32> = vec3<i32>(-1, 0, 0);
const FACE_TOP: vec3<i32> = vec3<i32>(0, 1, 0);
const FACE_BOTTOM: vec3<i32> = vec3<i32>(0, -1, 0);
const FACE_FRONT: vec3<i32> = vec3<i32>(0, 0, 1);
const FACE_BACK: vec3<i32> = vec3<i32>(0, 0, -1);

// ============================================================================
// 数据结构
// ============================================================================

// 输入：体素数据（绑定 0）
// 每个体素 1 字节（u8），共 32³ = 32768 字节
@group(0) @binding(0)
var<storage, read> voxel_data: array<u8>;

// 输出：顶点缓冲（绑定 1）
// 格式：vec4（位置.xyz + padding），每顶点 16 字节
@group(0) @binding(1)
var<storage, read_write> vertex_output: array<vec4<f32>>;

// 输出：索引缓冲（绑定 2）
// 每索引 4 字节（u32）
@group(0) @binding(2)
var<storage, read_write> index_output: array<u32>;

// 输出：顶点计数原子变量（绑定 3）
@group(0) @binding(3)
var<storage, read_write> vertex_count: atomic<u32>;

// 输出：索引计数原子变量（绑定 4）
@group(0) @binding(4)
var<storage, read_write> index_count: atomic<u32>;

// 输入：实例偏移量（区块世界坐标）（绑定 5）
@group(0) @binding(5)
var<storage, read> instance_offset: vec3<f32>;

// 输入：UV 查找表（block_id -> UV坐标）（绑定 6）
// 格式：array<vec4<f32>, 256>，每条目 (u_min, v_min, u_max, v_max)
@group(0) @binding(6)
var<storage, read> uv_table: array<vec4<f32>, 256>;

// ============================================================================
// 辅助函数
// ============================================================================

/// 将 3D 坐标转换为 1D 索引
fn coord_to_index(x: u32, y: u32, z: u32) -> u32 {
    return z * CHUNK_SIZE * CHUNK_SIZE + y * CHUNK_SIZE + x;
}

/// 检查坐标是否在区块范围内
fn in_bounds(x: i32, y: i32, z: i32) -> bool {
    return x >= 0 && y >= 0 && z >= 0 
        && x < i32(CHUNK_SIZE) && y < i32(CHUNK_SIZE) && z < i32(CHUNK_SIZE);
}

/// 获取体素数据（越界返回 0 = 空气）
fn get_voxel(x: i32, y: i32, z: i32) -> u8 {
    if !in_bounds(x, y, z) {
        return 0u8; // 越界视为空气
    }
    return voxel_data[coord_to_index(u32(x), u32(y), u32(z))];
}

/// 检查面是否可见（邻居不是空气且类型不同）
fn is_face_visible(voxel_id: u8, nx: i32, ny: i32, nz: i32) -> bool {
    let neighbor_id = get_voxel(nx, ny, nz);
    return neighbor_id != voxel_id && neighbor_id != 0u8;
}

/// 获取 UV 坐标
fn get_uv(block_id: u8, face_type: u32) -> vec4<f32> {
    // Texture Array 模式：每种方块有 3 个 UV（top, bottom, side）
    // UV 表存储的是 layer_index + 偏移量
    let idx = block_id * 3u + face_type;
    if idx < 256u {
        return uv_table[idx];
    }
    return vec4<f32>(0.0, 0.0, 1.0, 1.0); // 默认 UV
}

// ============================================================================
// 面生成函数
// ============================================================================

/// 生成单个面的 4 个顶点和 6 个索引
/// 
/// face_type: 0=right(+X), 1=left(-X), 2=top(+Y), 3=bottom(-Y), 4=front(+Z), 5=back(-Z)
fn emit_face(
    local_x: f32,
    local_y: f32,
    local_z: f32,
    face_type: u32,
    block_id: u8,
    vert_offset: u32,
    index_offset: u32,
) {
    // 4 个顶点的世界位置
    var p0: vec3<f32>;
    var p1: vec3<f32>;
    var p2: vec3<f32>;
    var p3: vec3<f32>;
    
    // 面法线
    var normal: vec3<f32>;
    
    // 根据面方向计算顶点位置和法线
    switch(face_type) {
        case 0u: { // RIGHT (+X)
            p0 = vec3<f32>(local_x + 1.0, local_y, local_z);
            p1 = vec3<f32>(local_x + 1.0, local_y, local_z + 1.0);
            p2 = vec3<f32>(local_x + 1.0, local_y + 1.0, local_z + 1.0);
            p3 = vec3<f32>(local_x + 1.0, local_y + 1.0, local_z);
            normal = vec3<f32>(1.0, 0.0, 0.0);
        }
        case 1u: { // LEFT (-X)
            p0 = vec3<f32>(local_x, local_y, local_z + 1.0);
            p1 = vec3<f32>(local_x, local_y, local_z);
            p2 = vec3<f32>(local_x, local_y + 1.0, local_z);
            p3 = vec3<f32>(local_x, local_y + 1.0, local_z + 1.0);
            normal = vec3<f32>(-1.0, 0.0, 0.0);
        }
        case 2u: { // TOP (+Y)
            p0 = vec3<f32>(local_x, local_y + 1.0, local_z);
            p1 = vec3<f32>(local_x + 1.0, local_y + 1.0, local_z);
            p2 = vec3<f32>(local_x + 1.0, local_y + 1.0, local_z + 1.0);
            p3 = vec3<f32>(local_x, local_y + 1.0, local_z + 1.0);
            normal = vec3<f32>(0.0, 1.0, 0.0);
        }
        case 3u: { // BOTTOM (-Y)
            p0 = vec3<f32>(local_x, local_y, local_z + 1.0);
            p1 = vec3<f32>(local_x + 1.0, local_y, local_z + 1.0);
            p2 = vec3<f32>(local_x + 1.0, local_y, local_z);
            p3 = vec3<f32>(local_x, local_y, local_z);
            normal = vec3<f32>(0.0, -1.0, 0.0);
        }
        case 4u: { // FRONT (+Z)
            p0 = vec3<f32>(local_x + 1.0, local_y, local_z + 1.0);
            p1 = vec3<f32>(local_x, local_y, local_z + 1.0);
            p2 = vec3<f32>(local_x, local_y + 1.0, local_z + 1.0);
            p3 = vec3<f32>(local_x + 1.0, local_y + 1.0, local_z + 1.0);
            normal = vec3<f32>(0.0, 0.0, 1.0);
        }
        case 5u: { // BACK (-Z)
            p0 = vec3<f32>(local_x, local_y, local_z);
            p1 = vec3<f32>(local_x + 1.0, local_y, local_z);
            p2 = vec3<f32>(local_x + 1.0, local_y + 1.0, local_z);
            p3 = vec3<f32>(local_x, local_y + 1.0, local_z);
            normal = vec3<f32>(0.0, 0.0, -1.0);
        }
        default: {}
    }
    
    // 添加实例偏移量（区块世界坐标）
    let offset = instance_offset;
    p0 = p0 + offset;
    p1 = p1 + offset;
    p2 = p2 + offset;
    p3 = p3 + offset;
    
    // 获取 UV 坐标
    let uv = get_uv(block_id, face_type);
    let u_min = uv.x;
    let u_max = uv.z;
    let v_min = uv.y;
    let v_max = uv.w;
    let eps = 0.016; // UV 收缩量
    
    // 计算 4 个顶点的 UV
    var uv0: vec2<f32> = vec2<f32>(u_min + eps, v_max - eps);
    var uv1: vec2<f32> = vec2<f32>(u_max - eps, v_max - eps);
    var uv2: vec2<f32> = vec2<f32>(u_max - eps, v_min + eps);
    var uv3: vec2<f32> = vec2<f32>(u_min + eps, v_min + eps);
    
    // 写入 4 个顶点（位置打包 + UV 在单独 buffer）
    // 顶点格式：vec4(position.xyz, face_type * 1000.0 + vertex_index)
    // face_type 用于解码法线方向
    vertex_output[vert_offset + 0u] = vec4<f32>(p0.x, p0.y, p0.z, f32(face_type) * 1000.0 + 0.0);
    vertex_output[vert_offset + 1u] = vec4<f32>(p1.x, p1.y, p1.z, f32(face_type) * 1000.0 + 1.0);
    vertex_output[vert_offset + 2u] = vec4<f32>(p2.x, p2.y, p2.z, f32(face_type) * 1000.0 + 2.0);
    vertex_output[vert_offset + 3u] = vec4<f32>(p3.x, p3.y, p3.z, f32(face_type) * 1000.0 + 3.0);
    
    // 写入 6 个索引（2 三角形）
    // 第一三角形：0, 2, 1
    // 第二三角形：0, 3, 2
    index_output[index_offset + 0u] = vert_offset + 0u;
    index_output[index_offset + 1u] = vert_offset + 2u;
    index_output[index_offset + 2u] = vert_offset + 1u;
    index_output[index_offset + 3u] = vert_offset + 0u;
    index_output[index_offset + 4u] = vert_offset + 3u;
    index_output[index_offset + 5u] = vert_offset + 2u;
}

// ============================================================================
// 主 Compute 函数
// ============================================================================

@compute @workgroup_size(8, 8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let x = id.x;
    let y = id.y;
    let z = id.z;
    
    // 越界检查
    if x >= CHUNK_SIZE || y >= CHUNK_SIZE || z >= CHUNK_SIZE {
        return;
    }
    
    // 获取体素 ID
    let voxel_id = voxel_data[coord_to_index(x, y, z)];
    
    // 空气跳过
    if voxel_id == 0u8 {
        return;
    }
    
    // 检查 6 个面是否可见
    let xi = i32(x);
    let yi = i32(y);
    let zi = i32(z);
    
    // 收集可见面
    var visible_faces: array<u32, 6>;
    var face_count: u32 = 0u;
    
    // +X (Right)
    if is_face_visible(voxel_id, xi + 1, yi, zi) {
        visible_faces[face_count] = 0u;
        face_count += 1u;
    }
    // -X (Left)
    if is_face_visible(voxel_id, xi - 1, yi, zi) {
        visible_faces[face_count] = 1u;
        face_count += 1u;
    }
    // +Y (Top)
    if is_face_visible(voxel_id, xi, yi + 1, zi) {
        visible_faces[face_count] = 2u;
        face_count += 1u;
    }
    // -Y (Bottom)
    if is_face_visible(voxel_id, xi, yi - 1, zi) {
        visible_faces[face_count] = 3u;
        face_count += 1u;
    }
    // +Z (Front)
    if is_face_visible(voxel_id, xi, yi, zi + 1) {
        visible_faces[face_count] = 4u;
        face_count += 1u;
    }
    // -Z (Back)
    if is_face_visible(voxel_id, xi, yi, zi - 1) {
        visible_faces[face_count] = 5u;
        face_count += 1u;
    }
    
    // 无可见面跳过
    if face_count == 0u {
        return;
    }
    
    // 分配顶点/索引空间（原子操作保证线程安全）
    let vert_offset = atomicAdd(&vertex_count, face_count * 4u);
    let index_offset = atomicAdd(&index_count, face_count * 6u);
    
    // 生成所有可见面
    let local_x = f32(x);
    let local_y = f32(y);
    let local_z = f32(z);
    
    for (var i: u32 = 0u; i < face_count; i++) {
        emit_face(
            local_x,
            local_y,
            local_z,
            visible_faces[i],
            voxel_id,
            vert_offset + i * 4u,
            index_offset + i * 6u,
        );
    }
}