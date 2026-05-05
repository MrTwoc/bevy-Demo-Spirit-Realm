//! 体素射线检测 — DDA 算法
//!
//! 从摄像机位置向视线方向发射射线，遍历射线经过的体素，
//! 找到第一个非空气方块后，在该位置显示一个半透明高亮方块。

use bevy::prelude::*;

use crate::chunk::{BlockPos, CHUNK_SIZE, ChunkData};
use crate::chunk_wire_frame::WireframeMode;

// ---------------------------------------------------------------------------
// 常量
// ---------------------------------------------------------------------------

/// 射线最大检测距离（方块数）
const MAX_RAY_DISTANCE: f32 = 64.0;

/// 高亮方块颜色（半透明黄色）
const HIGHLIGHT_COLOR: Color = Color::srgba(1.0, 1.0, 0.0, 0.3);

// ---------------------------------------------------------------------------
// 组件
// ---------------------------------------------------------------------------

/// 标记高亮方块实体，用于在射线未命中时移除
#[derive(Component)]
pub struct HighlightBlock;

/// 存储当前射线命中的方块位置
#[derive(Resource, Default)]
pub struct RayHitState {
    /// 当前命中的方块世界坐标（None 表示未命中）
    pub hit_pos: Option<BlockPos>,
}

// ---------------------------------------------------------------------------
// DDA 射线检测算法
// ---------------------------------------------------------------------------

/// 射线定义
pub struct Ray {
    pub origin: Vec3,
    pub direction: Vec3,
}

/// 射线命中结果
pub struct VoxelHit {
    /// 命中的方块位置
    pub block_pos: BlockPos,
    /// 命中的面法线（用于确定放置/破坏方块的位置）
    pub normal: IVec3,
    /// 距离射线起点的距离
    pub distance: f32,
}

/// DDA 体素射线遍历算法
///
/// 从射线起点出发，沿射线方向逐个体素遍历，
/// 返回第一个非空气方块的命中信息。
///
/// 参考: "A Fast Voxel Traversal Algorithm for Ray Tracing" - Amanatides & Woo
pub fn cast_ray(ray: &Ray, chunks: &[(&ChunkData, Vec3)]) -> Option<VoxelHit> {
    let dir = ray.direction.normalize();

    // 当前所在的体素坐标
    let mut x = ray.origin.x.floor() as i32;
    let mut y = ray.origin.y.floor() as i32;
    let mut z = ray.origin.z.floor() as i32;

    // 射线方向的步进符号（+1 或 -1）
    let step_x = if dir.x >= 0.0 { 1 } else { -1 };
    let step_y = if dir.y >= 0.0 { 1 } else { -1 };
    let step_z = if dir.z >= 0.0 { 1 } else { -1 };

    // 计算射线到达下一个体素边界的 t 值
    // t_delta: 射线穿过一个完整体素所需的 t 增量
    let t_delta_x = if dir.x.abs() > 1e-10 {
        (1.0 / dir.x).abs()
    } else {
        f32::MAX
    };
    let t_delta_y = if dir.y.abs() > 1e-10 {
        (1.0 / dir.y).abs()
    } else {
        f32::MAX
    };
    let t_delta_z = if dir.z.abs() > 1e-10 {
        (1.0 / dir.z).abs()
    } else {
        f32::MAX
    };

    // 初始 t_max: 射线到达第一个体素边界的 t 值
    let mut t_max_x = if dir.x.abs() > 1e-10 {
        let next_boundary = if dir.x > 0.0 {
            ray.origin.x.floor() + 1.0
        } else {
            ray.origin.x.floor()
        };
        (next_boundary - ray.origin.x) / dir.x
    } else {
        f32::MAX
    };
    let mut t_max_y = if dir.y.abs() > 1e-10 {
        let next_boundary = if dir.y > 0.0 {
            ray.origin.y.floor() + 1.0
        } else {
            ray.origin.y.floor()
        };
        (next_boundary - ray.origin.y) / dir.y
    } else {
        f32::MAX
    };
    let mut t_max_z = if dir.z.abs() > 1e-10 {
        let next_boundary = if dir.z > 0.0 {
            ray.origin.z.floor() + 1.0
        } else {
            ray.origin.z.floor()
        };
        (next_boundary - ray.origin.z) / dir.z
    } else {
        f32::MAX
    };

    // 记录上一步的命中面法线
    let mut face_normal = IVec3::ZERO;
    let mut t = 0.0;

    // 遍历体素
    for _ in 0..(MAX_RAY_DISTANCE * 3.0) as i32 {
        // 检查当前体素是否为非空气
        if let Some(block_id) = get_block_at(x, y, z, chunks) {
            if block_id != 0 {
                return Some(VoxelHit {
                    block_pos: BlockPos { x, y, z },
                    normal: face_normal,
                    distance: t,
                });
            }
        }

        // 步进到下一个体素（选择 t_max 最小的轴）
        if t_max_x < t_max_y {
            if t_max_x < t_max_z {
                x += step_x;
                t = t_max_x;
                t_max_x += t_delta_x;
                face_normal = IVec3::new(-step_x, 0, 0);
            } else {
                z += step_z;
                t = t_max_z;
                t_max_z += t_delta_z;
                face_normal = IVec3::new(0, 0, -step_z);
            }
        } else {
            if t_max_y < t_max_z {
                y += step_y;
                t = t_max_y;
                t_max_y += t_delta_y;
                face_normal = IVec3::new(0, -step_y, 0);
            } else {
                z += step_z;
                t = t_max_z;
                t_max_z += t_delta_z;
                face_normal = IVec3::new(0, 0, -step_z);
            }
        }

        // 超出最大距离
        if t > MAX_RAY_DISTANCE {
            break;
        }
    }

    None
}

/// 根据世界坐标查询方块 ID
fn get_block_at(x: i32, y: i32, z: i32, chunks: &[(&ChunkData, Vec3)]) -> Option<u8> {
    for (chunk_data, chunk_origin) in chunks {
        // 计算局部坐标
        let local_x = x - chunk_origin.x as i32;
        let local_y = y - chunk_origin.y as i32;
        let local_z = z - chunk_origin.z as i32;

        // 检查是否在 chunk 范围内
        if local_x >= 0
            && local_x < CHUNK_SIZE as i32
            && local_y >= 0
            && local_y < CHUNK_SIZE as i32
            && local_z >= 0
            && local_z < CHUNK_SIZE as i32
        {
            return Some(chunk_data.get(local_x as usize, local_y as usize, local_z as usize));
        }
    }
    None
}

// ---------------------------------------------------------------------------
// 系统
// ---------------------------------------------------------------------------

/// 每帧执行射线检测，更新高亮方块位置
pub fn raycast_highlight_system(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    camera_query: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    chunk_query: Query<(&ChunkData, &Transform)>,
    highlight_query: Query<Entity, With<HighlightBlock>>,
    mut hit_state: ResMut<RayHitState>,
    wireframe_mode: Res<WireframeMode>,
) {
    // 线框模式下不显示高亮（避免干扰）
    if wireframe_mode.0 {
        // 移除已有的高亮方块
        for entity in &highlight_query {
            commands.entity(entity).despawn();
        }
        hit_state.hit_pos = None;
        return;
    }

    let Ok((camera, cam_transform)) = camera_query.single() else {
        return;
    };

    // 获取屏幕中心点（准星位置）
    let viewport_size = camera
        .logical_viewport_size()
        .unwrap_or(Vec2::new(800.0, 600.0));
    let screen_center = viewport_size * 0.5;

    // 从屏幕中心生成射线
    let Ok(ray3d) = camera.viewport_to_world(cam_transform, screen_center) else {
        return;
    };

    let ray = Ray {
        origin: ray3d.origin,
        direction: *ray3d.direction,
    };

    // 收集所有 chunk 数据
    let chunk_data: Vec<(&ChunkData, Vec3)> = chunk_query
        .iter()
        .map(|(data, transform)| (data, transform.translation))
        .collect();

    // 执行射线检测
    let hit = cast_ray(&ray, &chunk_data);

    // 移除旧的高亮方块
    for entity in &highlight_query {
        commands.entity(entity).despawn();
    }

    match hit {
        Some(hit) => {
            hit_state.hit_pos = Some(hit.block_pos);

            // 在命中位置放置半透明高亮方块
            let highlight_pos = Vec3::new(
                hit.block_pos.x as f32 + 0.5,
                hit.block_pos.y as f32 + 0.5,
                hit.block_pos.z as f32 + 0.5,
            );

            // 创建一个略大的线框盒子来表示选中
            let mesh = meshes.add(Mesh::from(Cuboid::new(1.01, 1.01, 1.01)));
            let mat = materials.add(StandardMaterial {
                base_color: HIGHLIGHT_COLOR,
                alpha_mode: AlphaMode::Blend,
                unlit: true,
                ..default()
            });

            commands.spawn((
                HighlightBlock,
                Mesh3d(mesh),
                MeshMaterial3d(mat),
                Transform::from_translation(highlight_pos),
            ));
        }
        None => {
            hit_state.hit_pos = None;
        }
    }
}
