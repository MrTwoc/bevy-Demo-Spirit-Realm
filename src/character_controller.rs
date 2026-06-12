//! 自定义角色控制器：重力、跳跃、移动、AABB 碰撞检测。
//!
//! 不依赖外部物理引擎，直接查询 `LoadedChunks` 中的方块数据进行碰撞解算。
//! 使用逐轴分离（Separating Axis）算法，逐帧对 X/Y/Z 三个轴分别处理碰撞。

use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions};

use crate::block_definition::is_block_solid_from_table;
use crate::camera::CameraController;
use crate::chunk::{BlockId, CHUNK_SIZE, ChunkCoord};
use crate::chunk_manager::LoadedChunks;
use crate::player::Player;

// ── 物理常量 ────────────────────────────────────────────────────────

/// 重力加速度（m/s²），比现实值大以获得 Minecraft 风格的手感。
const GRAVITY: f32 = -28.0;
/// 跳跃初速度（m/s）。
const JUMP_VELOCITY: f32 = 9.0;
/// 行走速度（m/s）。
const WALK_SPEED: f32 = 4.317;
/// 冲刺速度（m/s）。
const SPRINT_SPEED: f32 = 5.612;
/// 终端下落速度（m/s），防止无限加速。
const TERMINAL_VELOCITY: f32 = -50.0;
/// 地面摩擦系数（每帧速度衰减）。
const GROUND_FRICTION: f32 = 0.0;
/// 空气阻力系数（每帧速度衰减）。
const AIR_FRICTION: f32 = 0.0;

// ── 组件 ────────────────────────────────────────────────────────────

/// 角色控制器组件，附加到玩家实体上。
#[derive(Component)]
pub struct CharacterController {
    /// 当前速度（米/秒）。
    pub velocity: Vec3,
    /// 是否站在实心方块上。
    pub grounded: bool,
    /// 是否在冲刺。
    pub sprinting: bool,
    /// 玩家碰撞箱水平半宽（0.3 对应 0.6 宽，与 Minecraft 一致）。
    pub half_width: f32,
    /// 玩家碰撞箱总高度（1.8，与 Minecraft 一致）。
    pub height: f32,
}

impl Default for CharacterController {
    fn default() -> Self {
        Self {
            velocity: Vec3::ZERO,
            grounded: false,
            sprinting: false,
            half_width: 0.3,
            height: 1.8,
        }
    }
}

// ── 方块查询 ────────────────────────────────────────────────────────

/// 查询世界坐标 (x, y, z) 处的方块 ID。未加载的区块视为空气。
#[inline]
fn get_block_at(loaded: &LoadedChunks, x: i32, y: i32, z: i32) -> BlockId {
    let coord = ChunkCoord {
        cx: x.div_euclid(CHUNK_SIZE as i32),
        cy: y.div_euclid(CHUNK_SIZE as i32),
        cz: z.div_euclid(CHUNK_SIZE as i32),
    };
    if let Some(entry) = loaded.entries.get(&coord) {
        let lx = x.rem_euclid(CHUNK_SIZE as i32) as usize;
        let ly = y.rem_euclid(CHUNK_SIZE as i32) as usize;
        let lz = z.rem_euclid(CHUNK_SIZE as i32) as usize;
        entry.data.get(lx, ly, lz)
    } else {
        0
    }
}

/// 检查世界坐标处的方块是否为实心碰撞体。
#[inline]
fn is_solid(loaded: &LoadedChunks, x: i32, y: i32, z: i32) -> bool {
    let block_id = get_block_at(loaded, x, y, z);
    is_block_solid_from_table(block_id)
}

// ── 系统 1：输入处理 ────────────────────────────────────────────────

/// 读取键盘输入，计算水平移动速度和跳跃请求。
///
/// 水平速度直接基于相机朝向投影到水平面，不累积加速度（Minecraft 风格）。
pub fn character_controller_input(
    keys: Res<ButtonInput<KeyCode>>,
    cursor_options: Single<&CursorOptions>,
    camera_query: Query<&GlobalTransform, With<CameraController>>,
    mut query: Query<&mut CharacterController, With<Player>>,
) {
    if cursor_options.grab_mode != CursorGrabMode::Locked {
        return;
    }

    let Ok(mut ctrl) = query.single_mut() else {
        return;
    };

    // ── 水平移动方向 ──
    let mut input_dir = Vec2::ZERO;
    if keys.pressed(KeyCode::KeyW) {
        input_dir.y += 1.0;
    }
    if keys.pressed(KeyCode::KeyS) {
        input_dir.y -= 1.0;
    }
    if keys.pressed(KeyCode::KeyA) {
        input_dir.x -= 1.0;
    }
    if keys.pressed(KeyCode::KeyD) {
        input_dir.x += 1.0;
    }

    // 冲刺判定
    ctrl.sprinting =
        keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);

    let speed = if ctrl.sprinting {
        SPRINT_SPEED
    } else {
        WALK_SPEED
    };

    // 将输入方向转换到世界空间（基于相机水平朝向）
    if input_dir != Vec2::ZERO {
        let Ok(cam_gt) = camera_query.single() else {
            return;
        };
        let forward = cam_gt.forward();
        let right = cam_gt.right();
        let horizontal_forward = Vec3::new(forward.x, 0.0, forward.z).normalize_or_zero();
        let horizontal_right = Vec3::new(right.x, 0.0, right.z).normalize_or_zero();

        let dir = (horizontal_forward * input_dir.y + horizontal_right * input_dir.x).normalize();

        ctrl.velocity.x = dir.x * speed;
        ctrl.velocity.z = dir.z * speed;
    } else {
        // 无输入时水平速度归零（Minecraft 风格：无惯性滑行）
        ctrl.velocity.x = 0.0;
        ctrl.velocity.z = 0.0;
    }

    // ── 跳跃 ──
    if keys.pressed(KeyCode::Space) && ctrl.grounded {
        ctrl.velocity.y = JUMP_VELOCITY;
        ctrl.grounded = false;
    }
}

// ── 系统 2：物理解算 ────────────────────────────────────────────────

/// 应用重力、逐轴移动并执行 AABB 碰撞解算。
///
/// 算法：
/// 1. 重力修改 velocity.y
/// 2. 对每个轴（X → Y → Z）：
///    a. 计算位移
///    b. 移动玩家位置
///    c. 查询 AABB 覆盖的所有方块
///    d. 对每个实心方块计算穿透并推出
///    e. 碰撞时将该轴速度归零
///    f. Y 轴向下碰撞 → grounded = true
pub fn character_controller_physics(
    time: Res<Time>,
    loaded: Res<LoadedChunks>,
    mut query: Query<(&mut CharacterController, &mut Transform), With<Player>>,
) {
    let Ok((mut ctrl, mut transform)) = query.single_mut() else {
        return;
    };

    let dt = time.delta_secs();
    if dt <= 0.0 || dt > 0.1 {
        // 防止极端帧时间导致穿墙
        return;
    }

    // ── 1. 重力 ──
    ctrl.velocity.y += GRAVITY * dt;
    if ctrl.velocity.y < TERMINAL_VELOCITY {
        ctrl.velocity.y = TERMINAL_VELOCITY;
    }

    // ── 2. 逐轴移动 + 碰撞解算 ──
    // 重置 grounded，Y 轴碰撞时重新设置
    ctrl.grounded = false;

    // 轴顺序：X → Y → Z（Y 在中间以确保地面检测正确）
    let axes = [
        Vec3::X,
        Vec3::Y,
        Vec3::Z,
    ];

    for axis in &axes {
        let delta = *axis * (ctrl.velocity.dot(*axis) * dt);
        if delta == Vec3::ZERO {
            continue;
        }

        // 移动位置
        transform.translation += delta;

        // ── AABB 碰撞检测与解算 ──
        resolve_collisions(
            &loaded,
            &mut transform.translation,
            &mut ctrl,
            *axis,
        );
    }

    // ── 3. 摩擦 / 阻力 ──
    let friction = if ctrl.grounded {
        GROUND_FRICTION
    } else {
        AIR_FRICTION
    };
    ctrl.velocity.x *= friction;
    ctrl.velocity.z *= friction;
}

// ── 碰撞解算核心 ────────────────────────────────────────────────────

/// 皮肤厚度：重叠检测用的微小收缩量。
/// 使得"刚好接触"（玩家脚底 = 方块顶面）不被判定为重叠。
const SKIN: f32 = 0.002;

/// 检查玩家 AABB 是否与方块 AABB 重叠（三个轴全部检查）。
///
/// 使用 SKIN 收缩玩家 AABB，避免精确边界处的误判。
/// 例如脚底 Y=66.0 对上方块顶面 Y=66.0 时，收缩后不重叠。
#[inline]
fn aabb_overlaps(
    px: f32, py: f32, pz: f32,
    half_width: f32, height: f32,
    bx: i32, by: i32, bz: i32,
) -> bool {
    let p_min_x = px - half_width + SKIN;
    let p_max_x = px + half_width - SKIN;
    let p_min_y = py + SKIN;
    let p_max_y = py + height - SKIN;
    let p_min_z = pz - half_width + SKIN;
    let p_max_z = pz + half_width - SKIN;

    p_min_x < (bx + 1) as f32
        && p_max_x > bx as f32
        && p_min_y < (by + 1) as f32
        && p_max_y > by as f32
        && p_min_z < (bz + 1) as f32
        && p_max_z > bz as f32
}

/// 对单个轴执行 AABB vs 方块碰撞解算。
///
/// 算法：
/// 1. 遍历玩家 AABB 覆盖的所有方块
/// 2. 用收缩后的 AABB（SKIN margin）检查三轴是否都重叠
/// 3. 对活跃轴，仅在速度方向指向方块内部时计算穿透并推出
/// 4. 速度 == 0 时跳过（玩家不移动，无需解算）
fn resolve_collisions(
    loaded: &LoadedChunks,
    position: &mut Vec3,
    ctrl: &mut CharacterController,
    axis: Vec3,
) {
    let hw = ctrl.half_width;
    let h = ctrl.height;

    let min_x = (position.x - hw).floor() as i32;
    let max_x = (position.x + hw).floor() as i32;
    let min_y = position.y.floor() as i32;
    let max_y = (position.y + h).ceil() as i32;
    let min_z = (position.z - hw).floor() as i32;
    let max_z = (position.z + hw).floor() as i32;

    for bx in min_x..=max_x {
        for by in min_y..=max_y {
            for bz in min_z..=max_z {
                if !is_solid(loaded, bx, by, bz) {
                    continue;
                }

                // 三轴重叠检查（SKIN 收缩后）
                if !aabb_overlaps(position.x, position.y, position.z, hw, h, bx, by, bz) {
                    continue;
                }

                // ── 活跃轴碰撞解算 ──
                // 只在速度指向方块内部时处理，velocity == 0 跳过
                if axis.x != 0.0 && ctrl.velocity.x != 0.0 {
                    let pen = if ctrl.velocity.x > 0.0 {
                        (position.x + hw) - bx as f32
                    } else {
                        (bx + 1) as f32 - (position.x - hw)
                    };
                    if pen > 0.0 {
                        if ctrl.velocity.x > 0.0 {
                            position.x -= pen;
                        } else {
                            position.x += pen;
                        }
                        ctrl.velocity.x = 0.0;
                    }
                }

                if axis.y != 0.0 && ctrl.velocity.y != 0.0 {
                    let pen = if ctrl.velocity.y > 0.0 {
                        (position.y + h) - by as f32
                    } else {
                        (by + 1) as f32 - position.y
                    };
                    if pen > 0.0 {
                        if ctrl.velocity.y > 0.0 {
                            position.y -= pen;
                        } else {
                            position.y += pen;
                            ctrl.grounded = true;
                        }
                        ctrl.velocity.y = 0.0;
                    }
                }

                if axis.z != 0.0 && ctrl.velocity.z != 0.0 {
                    let pen = if ctrl.velocity.z > 0.0 {
                        (position.z + hw) - bz as f32
                    } else {
                        (bz + 1) as f32 - (position.z - hw)
                    };
                    if pen > 0.0 {
                        if ctrl.velocity.z > 0.0 {
                            position.z -= pen;
                        } else {
                            position.z += pen;
                        }
                        ctrl.velocity.z = 0.0;
                    }
                }
            }
        }
    }
}

// ── 系统 3：同步到 HUD / 调试 ──────────────────────────────────────

// 此系统预留，未来可用于同步物理状态到 HUD 显示。
// 当前不需要额外同步，因为 Transform 已在 physics 系统中直接修改。
