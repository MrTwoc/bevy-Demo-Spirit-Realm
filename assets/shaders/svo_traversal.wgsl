// SVO Octree 遍历 Compute Shader (Phase 2 - Voxy 风格)
//
// 输入:
//   @binding(0) - Node Buffer: 扁平节点数组 (u64 × 2 per node, 紧凑格式)
//   @binding(1) - Camera Uniform: 相机位置 + 渲染距离 + 视锥体
//   @binding(2) - Counter Buffer: 原子计数器 (输出)
//   @binding(3) - Visible List: 可见节点 ID 输出
//
// 每个线程处理一个节点，执行距离 + 视锥体剔除
// 借鉴 Voxy 的 HierarchicalOcclusionTraverser 设计

// ── 数据结构 ────────────────────────────────────────────────────────────

// Voxy 风格的节点数据 (紧凑格式, 16 字节)
// word0: position encoding (lvl:4 | x:20 | y:20 | z:20)
// word1: geometry_handle(24) | child_ptr(24) | flags(8) | child_existence(8)
struct GpuNode {
    position: u64,
    data: u64,
}

struct CameraUniform {
    camera_world_x: f32,
    camera_world_y: f32,
    camera_world_z: f32,
    render_distance: f32,
    frustum_planes: array<vec4<f32>, 6>,  // 6 个裁剪平面
    node_count: u32,
    _pad: u32,
    _pad2: u32,
    _pad3: u32,
}

// ── 绑定声明 ────────────────────────────────────────────────────────────

@group(0) @binding(0) var<storage, read> node_buffer: array<u64>;
@group(0) @binding(1) var<uniform> camera: CameraUniform;
@group(0) @binding(2) var<storage, read_write> counter: atomic<u32>;
@group(0) @binding(3) var<storage, read_write> visible_nodes: array<u32>;

// ── 节点类型常量 ────────────────────────────────────────────────────────

const NODE_NONE: u32 = 0u;
const NODE_LEAF: u32 = 1u;
const NODE_INNER: u32 = 2u;
const NODE_PENDING: u32 = 3u;

// ── 位置解码 (Voxy 风格: lvl:4 | x:20 | y:20 | z:20) ──────────────────

fn decode_level(pos: u64) -> u32 {
    return u32((pos >> 60u) & u64(0xFu));
}

fn decode_x(pos: u64) -> i32 {
    // x 在 bits 40-59, 需要符号扩展
    let raw = u32((pos >> 40u) & u64(0xFFFFFu));
    // 符号扩展: 如果 bit 19 是 1, 则扩展高 12 位
    return i32(raw << 12u) >> 12;
}

fn decode_y(pos: u64) -> i32 {
    // y 在 bits 20-39, 需要符号扩展
    let raw = u32((pos >> 20u) & u64(0xFFFFFu));
    // 符号扩展: 如果 bit 19 是 1, 则扩展高 12 位
    return i32(raw << 12u) >> 12;
}

fn decode_z(pos: u64) -> i32 {
    // z 在 bits 0-19, 需要符号扩展
    let raw = u32(pos & u64(0xFFFFFu));
    // 符号扩展: 如果 bit 19 是 1, 则扩展高 12 位
    return i32(raw << 12u) >> 12;
}

// ── 节点数据解码 ────────────────────────────────────────────────────────

fn get_node_type(data: u64) -> u32 {
    // flags 在 bits 48-55, 低 2 位是节点类型
    return u32((data >> 48u) & u64(0x3u));
}

fn get_child_existence(data: u64) -> u32 {
    // child_existence 在 bits 56-63
    return u32((data >> 56u) & u64(0xFFu));
}

fn get_geometry_handle(data: u64) -> u32 {
    // geometry_handle 在 bits 0-23
    return u32(data & u64(0xFFFFFFu));
}

fn get_child_ptr(data: u64) -> u32 {
    // child_ptr 在 bits 24-47
    return u32((data >> 24u) & u64(0xFFFFFFu));
}

// ── 屏幕空间误差判断 (借鉴 Voxy shouldDecend) ──────────────────────────

// 判断是否需要下降到子节点
// 基于节点在屏幕上的像素大小
fn should_descend(lod_level: u32, distance: f32) -> bool {
    // 节点大小 = 32 << lod_level 体素
    let node_size = f32(32u << lod_level);
    
    // 计算节点在屏幕上的像素大小 (简化版本)
    // 假设 FOV = 90 度, 屏幕高度 = 1080 像素
    let fov_factor = 1080.0 / 2.0;  // 简化的投影因子
    let pixel_size = (node_size / distance) * fov_factor;
    
    // 如果节点在屏幕上大于 1 像素, 应该下降
    return pixel_size > 1.0;
}

// ── 视锥体测试 ──────────────────────────────────────────────────────────

// AABB vs 平面测试 (任何平面外 → 裁剪)
fn test_plane(plane: vec4<f32>, center: vec3<f32>, half_size: f32) -> bool {
    // 计算 AABB 在平面法线方向上的投影半径
    // 优化：将 3 次乘法合并为 1 次（half_size 提取公因子）
    let abs_normal = abs(plane.xyz);
    let radius = half_size * (abs_normal.x + abs_normal.y + abs_normal.z);
    // 距离 = dot(center, normal) + d > -radius 则可见
    return dot(plane.xyz, center) + plane.w >= -radius;
}

// 完整 6 平面视锥体测试（LOD 0-1 使用）
fn is_visible(center: vec3<f32>, half_size: f32) -> bool {
    for (var i = 0u; i < 6u; i = i + 1u) {
        if (!test_plane(camera.frustum_planes[i], center, half_size)) {
            return false;
        }
    }
    return true;
}

// 粗视锥体测试：仅测试 near/far 平面（索引 4 和 5）
// 用于 LOD≥2 的大节点，2 次测试而非 6 次
fn is_visible_coarse(center: vec3<f32>, half_size: f32) -> bool {
    return test_plane(camera.frustum_planes[4], center, half_size)
        && test_plane(camera.frustum_planes[5], center, half_size);
}

// ── 距离计算 ────────────────────────────────────────────────────────────

// 计算节点到相机的距离平方 (XZ 平面)
fn distance_sq_xz(center: vec3<f32>, cam_pos: vec3<f32>) -> f32 {
    let dx = center.x - cam_pos.x;
    let dz = center.z - cam_pos.z;
    return dx * dx + dz * dz;
}

// 计算节点到相机的完整距离平方
fn distance_sq(center: vec3<f32>, cam_pos: vec3<f32>) -> f32 {
    let d = center - cam_pos;
    return dot(d, d);
}

// ── 主遍历函数 ──────────────────────────────────────────────────────────

@compute @workgroup_size(64, 1, 1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let node_index = id.x;
    if (node_index >= camera.node_count) {
        return;
    }

    // 读取节点 (每个节点 2 个 u64, 紧凑格式)
    let pos_word = node_buffer[node_index * 2u];
    let data_word = node_buffer[node_index * 2u + 1u];
    let node_type = get_node_type(data_word);

    // 跳过空节点
    if (node_type == NODE_NONE) {
        return;
    }

    // 解码位置 (Voxy 风格: lvl:4 | x:20 | y:20 | z:20)
    let lvl = decode_level(pos_word);
    let nx = decode_x(pos_word);
    let ny = decode_y(pos_word);
    let nz = decode_z(pos_word);

    // 计算节点在世界空间中的中心位置和半边长
    // 每个 section = 32 体素; 节点边长 = 32 << lvl 体素
    let section_size: f32 = 32.0;
    let scale = f32(1u << lvl);
    let node_size = section_size * scale;
    let half_size = node_size * 0.5;
    let center = vec3<f32>(
        f32(nx) * node_size + half_size,
        f32(ny) * node_size + half_size,
        f32(nz) * node_size + half_size,
    );

    // ── 距离剔除 ──────────────────────────────────────────────────────
    let cam_pos = vec3<f32>(camera.camera_world_x, camera.camera_world_y, camera.camera_world_z);
    
    if (camera.render_distance > 0.0) {
        let dy = abs(center.y - cam_pos.y);
        
        // Y 轴快速剔除：超出渲染距离的垂直范围直接跳过
        if (dy > camera.render_distance) {
            return;
        }

        let dist_xz_sq = distance_sq_xz(center, cam_pos);
        let render_dist_sq = camera.render_distance * camera.render_distance;
        
        // XZ 平面距离剔除 (加上节点半径的缓冲)
        if (dist_xz_sq > render_dist_sq + half_size * half_size * 2.0) {
            return;
        }
    }

    // ── 视锥体剔除（分级策略）──────────────────────────────────────
    // LOD 0-1：精确 6 平面测试
    // LOD 2+：粗略 near/far 测试（2 次），剔除相机背后的节点
    if (lvl <= 1u) {
        if (!is_visible(center, half_size)) {
            return;
        }
    } else {
        if (!is_visible_coarse(center, half_size)) {
            return;
        }
    }

    // ── 写入可见节点列表 ──────────────────────────────────────────────
    let visible_index = atomicAdd(&counter, 1u);
    visible_nodes[visible_index] = node_index;
}
