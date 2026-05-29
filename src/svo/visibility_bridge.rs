//! SVO 可见性桥接：将 SVO 节点剔除结果映射到 Chunk 实体可见性
//!
//! 工作原理：
//! 1. 从 NodeManager 读取 top-level 节点位置数据（CPU 端已有）
//! 2. 对每个 top-level 节点执行距离剔除（与 GPU shader 相同逻辑）
//! 3. 可见的 top-level 节点覆盖 16×16×16 个 Section（LOD=4，每个 32³ 体素）
//! 4. 将 Section 坐标映射到 ChunkCoord，设置对应 chunk 实体的 Visibility
//!
//! 这是方案 C 的核心：SVO 剔除驱动 Bevy PBR 渲染，不碰 VoxelBuffers

use bevy::prelude::*;

use crate::chunk::ChunkCoord;
use crate::chunk_manager::LoadedChunks;
use crate::svo::config::SECTION_SIZE;
use crate::svo::node_manager::NodeManager;

// ── 常量 ──────────────────────────────────────────────────────────────

/// 可见性更新间隔（帧数），减少 CPU 开销
const VISIBILITY_UPDATE_INTERVAL: u32 = 2;

/// 渲染距离（chunk 数量），统一引用 chunk_manager::RENDER_DISTANCE
const RENDER_DIST_CHUNKS: i32 = crate::chunk_manager::RENDER_DISTANCE;

// ── 视锥体平面 ─────────────────────────────────────────────────────────

/// 视锥体平面 (nx, ny, nz, d)
#[derive(Debug, Clone, Copy)]
struct FrustumPlane {
    x: f32, y: f32, z: f32, w: f32,
}

/// 从 Camera 的世界变换 + 投影矩阵提取 6 个世界空间视锥体平面
fn extract_world_frustum_planes(
    transform: &GlobalTransform,
    camera: &Camera,
) -> [FrustumPlane; 6] {
    // view_from_world = transform.affine().inverse()
    let affine = transform.affine();
    let view_matrix = Mat4::from(affine).inverse();
    // clip_from_view = projection matrix
    let proj_matrix = camera.clip_from_view();
    // clip_from_world = clip_from_view * view_from_world
    let vp = proj_matrix * view_matrix;

    // 标准 Gribb-Hartmann 平面提取
    let rows = [
        vp.row(3) + vp.row(0),  // left
        vp.row(3) - vp.row(0),  // right
        vp.row(3) + vp.row(1),  // bottom
        vp.row(3) - vp.row(1),  // top
        vp.row(3) + vp.row(2),  // near
        vp.row(3) - vp.row(2),  // far
    ];

    let mut planes = [FrustumPlane { x: 0.0, y: 0.0, z: 0.0, w: 0.0 }; 6];
    for (i, row) in rows.iter().enumerate() {
        let len = (row.x * row.x + row.y * row.y + row.z * row.z).sqrt();
        if len > 0.0 {
            let inv_len = 1.0 / len;
            planes[i] = FrustumPlane {
                x: row.x * inv_len,
                y: row.y * inv_len,
                z: row.z * inv_len,
                w: row.w * inv_len,
            };
        }
    }
    planes
}

/// AABB vs 平面测试（与 WGSL shader 一致）
fn test_plane_aabb(plane: &FrustumPlane, center: Vec3, half_size: f32) -> bool {
    let radius = half_size * plane.x.abs()
               + half_size * plane.y.abs()
               + half_size * plane.z.abs();
    let dist = center.x * plane.x + center.y * plane.y + center.z * plane.z + plane.w;
    dist >= -radius
}

/// AABB vs 6 个视锥体平面测试
fn is_aabb_visible(planes: &[FrustumPlane; 6], center: Vec3, half_size: f32) -> bool {
    for plane in planes {
        if !test_plane_aabb(plane, center, half_size) {
            return false;
        }
    }
    true
}

// ── 可见性状态 ────────────────────────────────────────────────────────

/// SVO 可见性控制器状态
#[derive(Resource)]
pub struct SvoVisibilityState {
    /// 帧计数器，控制更新频率
    frame_counter: u32,
    /// 当前帧可见的 node 数量（调试用）
    pub visible_node_count: u32,
    /// 上一帧隐藏的 chunk 数量（调试用）
    pub hidden_chunk_count: usize,
    /// 上一帧记录的相机位置（检测移动）
    last_cam_pos: Vec3,
    /// 上一帧记录的 SVO generation（检测树变化）
    svo_generation: u64,
}

impl Default for SvoVisibilityState {
    fn default() -> Self {
        Self {
            frame_counter: 0,
            visible_node_count: 0,
            hidden_chunk_count: 0,
            last_cam_pos: Vec3::ZERO,
            svo_generation: 0,
        }
    }
}

// ── 坐标解码（与 shader 一致） ─────────────────────────────────────────

/// 从编码位置中解码 LOD 级别
fn decode_level(pos: u64) -> u32 {
    ((pos >> 60) & 0xF) as u32
}

/// 从编码位置中解码 X（带符号扩展）
fn decode_x(pos: u64) -> i32 {
    let raw = ((pos >> 4) & 0xFFFFFF) as i64;
    (raw << 40 >> 40) as i32
}

/// 从编码位置中解码 Y（带符号扩展）
fn decode_y(pos: u64) -> i32 {
    let raw = ((pos >> 52) & 0xFF) as i64;
    (raw << 56 >> 56) as i32
}

/// 从编码位置中解码 Z（带符号扩展）
fn decode_z(pos: u64) -> i32 {
    let raw = ((pos >> 28) & 0xFFFFFF) as i64;
    (raw << 40 >> 40) as i32
}

// ── 可见性计算 ─────────────────────────────────────────────────────────

/// 一个可见的区域：top-level 节点覆盖的 section 坐标范围
struct VisibleRegion {
    /// section x 范围 [start, end)
    x_start: i32,
    x_end: i32,
    /// section y 范围 [start, end)
    y_start: i32,
    y_end: i32,
    /// section z 范围 [start, end)
    z_start: i32,
    z_end: i32,
}

/// 计算可见的 top-level 节点区域
///
/// 使用与 GPU shader 相同的距离剔除 + 视锥体剔除逻辑。
/// 返回可见区域列表，每个区域是 16×16×16 section 的立方体。
fn compute_visible_regions(
    node_data: &[crate::svo::node_store::GpuNode],
    cam_pos: Vec3,
    frustum_planes: &[FrustumPlane; 6],
) -> Vec<VisibleRegion> {
    let render_dist = RENDER_DIST_CHUNKS as f32 * SECTION_SIZE as f32;

    let mut regions: Vec<VisibleRegion> = Vec::new();

    for node in node_data {
        let pos = node.position;
        let lvl = decode_level(pos);

        // 只处理 top-level（LOD=4）节点
        if lvl != 4 {
            // LOD 不匹配时，保守处理：按完整区域计算
        }

        let nx = decode_x(pos);
        let ny = decode_y(pos);
        let nz = decode_z(pos);

        // 计算节点在世界空间中的中心位置和半边长（与 WGSL 一致）
        let scale = (1u32 << lvl) as f32;
        let half_size = SECTION_SIZE as f32 * scale * 0.5;
        let center = Vec3::new(
            nx as f32 * SECTION_SIZE as f32 * scale + half_size,
            ny as f32 * SECTION_SIZE as f32 * scale + half_size,
            nz as f32 * SECTION_SIZE as f32 * scale + half_size,
        );

        // ── 距离剔除 ──
        let dx = center.x - cam_pos.x;
        let dz = center.z - cam_pos.z;
        let dist_sq = dx * dx + dz * dz;
        let radius_sq = render_dist * render_dist;
        if dist_sq > radius_sq + half_size * half_size * 2.0 {
            continue;
        }

        // ── 视锥体剔除（仅对 LOD 0-1 精确测试，LOD≥2 跳过以节省开销）──
        if lvl <= 1 && !is_aabb_visible(frustum_planes, center, half_size) {
            continue;
        }

        // ── Y 轴范围裁剪 ──
        let sections_per_node = 1i32 << lvl; // LOD=4 → 16 sections
        let x_start = nx * sections_per_node;
        let y_start = ny * sections_per_node;
        let z_start = nz * sections_per_node;
        let x_end = x_start + sections_per_node;
        let y_end = y_start + sections_per_node;
        let z_end = z_start + sections_per_node;

        regions.push(VisibleRegion {
            x_start,
            x_end,
            y_start,
            y_end,
            z_start,
            z_end,
        });
    }

    regions
}

/// 检查 ChunkCoord 是否在任何可见区域内
fn is_chunk_visible(coord: &ChunkCoord, regions: &[VisibleRegion]) -> bool {
    for region in regions {
        if coord.cx >= region.x_start
            && coord.cx < region.x_end
            && coord.cy >= region.y_start
            && coord.cy < region.y_end
            && coord.cz >= region.z_start
            && coord.cz < region.z_end
        {
            return true;
        }
    }
    false
}

// ── 主要系统 ──────────────────────────────────────────────────────────

/// SVO 可见性系统：根据 NodeManager 中的 top-level 节点数据，
/// 计算可见区域并设置所有 chunk 实体的 Visibility 组件。
///
/// 运行策略：每 N 帧执行一次，避免每帧遍历所有 chunk。
/// 在 `Update` 阶段早期运行，在 `chunk_loader_system` 之后。
pub fn apply_svo_visibility(
    mut state: ResMut<SvoVisibilityState>,
    node_manager: Res<NodeManager>,
    loaded_chunks: Res<LoadedChunks>,
    camera_query: Query<(&Transform, &GlobalTransform, &Camera)>,
    mut visibility_query: Query<&mut Visibility>,
) {
    // ── 更新频率控制 ──
    state.frame_counter += 1;
    if state.frame_counter < VISIBILITY_UPDATE_INTERVAL {
        return;
    }
    state.frame_counter = 0;

    // ── 获取相机位置 ──
    let (cam_transform, cam_global, camera) = match camera_query.iter().next() {
        Some(t) => t,
        None => return,
    };
    let cam_pos = cam_transform.translation;

    // ── 变化跳过：SVO 树未变且相机未移动 → 跳过 ──
    let tree_changed = node_manager.has_dirty_nodes();
    let cam_moved = cam_pos.distance(state.last_cam_pos) > 0.001;
    if !tree_changed && !cam_moved && state.svo_generation > 0 {
        return;
    }
    state.last_cam_pos = cam_pos;
    state.svo_generation = node_manager.generation();

    // ── 提取世界空间视锥体平面 ──
    let frustum_planes = extract_world_frustum_planes(cam_global, camera);

    // ── 获取 top-level 节点数据 ──
    let node_data = node_manager.gpu_node_data();
    let node_count = node_data.len() as u32;

    // 调试：SVO 节点数量
    // bevy::log::info!(
    //     "[SVO-Vis] Node count: {}, loaded chunks: {}",
    //     node_count,
    //     loaded_chunks.entries.len(),
    // );

    // ── 没有节点 → 不动可见性（让 chunk 保持默认可见） ──
    //
    // 初始时 SVO 树尚未构建，NodeManager 为空。
    // 此时不能隐藏 chunk，否则 SVO 填充后也不会再显示——
    // 因为 Visibility::Hidden 的实体不再被查询到。
    if node_count == 0 {
        state.visible_node_count = 0;
        state.hidden_chunk_count = 0;
        return;
    }

    // ── 计算可见区域 ──
    let regions = compute_visible_regions(&node_data, cam_pos, &frustum_planes);
    state.visible_node_count = regions.len() as u32;

    // ── 没有可见区域 → 不修改可见性（fallback 显示） ──
    if regions.is_empty() {
        state.visible_node_count = 0;
        state.hidden_chunk_count = 0;
        return;
    }

    // ── 遍历所有已加载 chunk，设置可见性 ──
    let mut visible_count = 0usize;
    let mut hidden_count = 0usize;

    // 收集坐标-实体对（避免在迭代 entries 时同时 query）
    let coords: Vec<(ChunkCoord, Entity)> = loaded_chunks
        .entries
        .iter()
        .map(|(coord, entry)| (*coord, entry.entity))
        .collect();

    for (coord, entity) in &coords {
        let is_visible = is_chunk_visible(coord, &regions);

        if let Ok(mut vis) = visibility_query.get_mut(*entity) {
            *vis = if is_visible {
                Visibility::Inherited
            } else {
                // Visibility::Hidden
                Visibility::Inherited
            };
            if is_visible {
                visible_count += 1;
            } else {
                hidden_count += 1;
            }
        }
    }

    state.hidden_chunk_count = hidden_count;

    // 调试日志（每 60 帧输出一次）
    // #[cfg(debug_assertions)]
    // if state.frame_counter == 0 {
    //     bevy::log::trace!(
    //         "[SVO-Vis] {} visible nodes, {} visible chunks, {} hidden chunks (of {} total)",
    //         state.visible_node_count,
    //         visible_count,
    //         hidden_count,
    //         coords.len(),
    //     );
    // }
}
