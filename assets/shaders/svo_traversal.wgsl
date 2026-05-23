// SVO Octree 遍历 Compute Shader (Phase 1)
//
// 输入:
//   @binding(0) - Node Buffer: 扁平节点数组 (u64 × 2 per node)
//   @binding(1) - Camera Uniform: 相机位置 + 渲染距离 + 视锥体
//   @binding(2) - Counter Buffer: 原子计数器 (输出)
//   @binding(3) - Visible List: 可见节点 ID 输出
//
// 每个线程处理一个 top-level 节点 (LOD=4)，执行距离 + 视锥体剔除

// ── 数据结构 ────────────────────────────────────────────────────────────

struct GpuNode {
    position: u64,  // 位置编码 (lvl:4|y:8|z:24|x:24|pad:4)
    data: u64,      // geometry_handle(24) | flags(8) | child_existence(8) | spare(24)
}

struct CameraUniform {
    camera_world_x: f32,
    camera_world_y: f32,
    camera_world_z: f32,
    render_distance: f32,
    frustum_planes: array<vec4<f32>, 6>,  // 6 个裁剪平面
    node_count: u32,
    _pad: u32,
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

// ── 位置解码 ────────────────────────────────────────────────────────────

fn decode_level(pos: u64) -> u32 {
    return u32((pos >> 60u) & u64(0xFu));
}

fn decode_x(pos: u64) -> i32 {
    // (pad:4) 不在编码中直接表示, x 在 bits 4-27
    let raw = u32((pos >> 4u) & u64(0xFFFFFFu));
    return i32(raw << 8u) >> 8; // 符号扩展
}

fn decode_y(pos: u64) -> i32 {
    let raw = u32((pos >> 52u) & u64(0xFFu));
    return i32(raw << 24u) >> 24; // 符号扩展
}

fn decode_z(pos: u64) -> i32 {
    let raw = u32((pos >> 28u) & u64(0xFFFFFFu));
    return i32(raw << 8u) >> 8; // 符号扩展
}

fn get_node_type(data: u64) -> u32 {
    return u32((data >> 24u) & u64(0x3u));
}

fn get_child_existence(data: u64) -> u32 {
    return u32((data >> 32u) & u64(0xFFu));
}

fn get_geometry_handle(data: u64) -> u32 {
    return u32(data & u64(0xFFFFFFu));
}

// ── 视锥体测试 ──────────────────────────────────────────────────────────

// AABB vs 平面测试 (任何平面外 → 裁剪)
fn test_plane(plane: vec4<f32>, center: vec3<f32>, half_size: f32) -> bool {
    // 计算 AABB 在平面法线方向上的投影半径
    let radius = half_size * abs(plane.x)
               + half_size * abs(plane.y)
               + half_size * abs(plane.z);
    // 距离 = dot(center, normal) + d > -radius 则可见
    return dot(plane.xyz, center) + plane.w >= -radius;
}

fn is_visible(center: vec3<f32>, half_size: f32) -> bool {
    for (var i = 0u; i < 6u; i = i + 1u) {
        if (!test_plane(camera.frustum_planes[i], center, half_size)) {
            return false;
        }
    }
    return true;
}

// ── 主遍历函数 ──────────────────────────────────────────────────────────

@compute @workgroup_size(64, 1, 1)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let node_index = id.x;
    if (node_index >= camera.node_count) {
        return;
    }

    // 读取节点 (每个节点 2 个 u64)
    let pos_word = node_buffer[node_index * 2u];
    let data_word = node_buffer[node_index * 2u + 1u];
    let node_type = get_node_type(data_word);

    // 跳过空节点
    if (node_type == NODE_NONE) {
        return;
    }

    // 解码位置
    let lvl = decode_level(pos_word);
    let nx = decode_x(pos_word);
    let ny = decode_y(pos_word);
    let nz = decode_z(pos_word);

    // 计算节点在世界空间中的中心位置和半边长
    // 每个 section = 32 体素; top-level (LOD=4) = 512 体素
    // 节点中心 = (nx * 32 + 16, ny * 32 + 16, nz * 32 + 16)
    // 实际上需要从位置编码计算出正确的世界坐标
    // 对于 LOD=lvl 的节点, 边长 = 32 << lvl 体素
    let section_size: f32 = 32.0;
    let scale = f32(1u << lvl);
    let half_size = section_size * scale * 0.5;
    let center = vec3<f32>(
        f32(nx) * section_size * scale + half_size,
        f32(ny) * section_size * scale + half_size,
        f32(nz) * section_size * scale + half_size,
    );

    // ── 距离剔除 ──────────────────────────────────────────────────────
    if (camera.render_distance > 0.0) {
        let dx = center.x - camera.camera_world_x;
        let dz = center.z - camera.camera_world_z;
        let dist_sq = dx * dx + dz * dz;
        let radius_sq = camera.render_distance * camera.render_distance;
        if (dist_sq > radius_sq + half_size * half_size * 2.0) {
            return; // 超出渲染距离
        }
    }

    // ── 视锥体剔除 (仅对非根节点加速) ─────────────────────────────────
    // LOD>=2 的节点大概率可见，跳过视锥测试以节省开销
    // 只有 LOD 0-1 做精确的视锥体测试
    if (lvl <= 1u || !is_visible(center, half_size)) {
        // 对于 LOD>1 的节点: 实际是 ALWAYS 保留
        // 只有小节点(LOD<=1)才做视锥测试
        if (lvl <= 1u) {
            if (!is_visible(center, half_size)) {
                return;
            }
        }
    }

    // ── 写入可见节点列表 ──────────────────────────────────────────────
    let visible_index = atomicAdd(&counter, 1u);
    visible_nodes[visible_index] = node_index;
}
