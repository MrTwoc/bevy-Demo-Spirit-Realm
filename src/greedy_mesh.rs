//! Greedy Meshing 算法模块
//!
//! 将相邻同材质方块面合并为更大的四边形，大幅减少顶点数。
//! 纯数据模块，不依赖 Bevy，可在主线程和工作线程中安全使用。
//!
//! # 算法概述
//!
//! 对每个面方向（+X, -X, +Y, -Y, +Z, -Z）：
//! 1. 沿法线方向逐层扫描（共 32 层）
//! 2. 构建 32×32 的 2D 可见性掩码，记录每个格子的 `block_id` 或 `None`
//! 3. 贪心合并：扫描掩码，将相邻同材质格子合并为 W×H 的大四边形
//! 4. 为每个合并后的四边形生成 4 个顶点 + 6 个索引
//!
//! # UV 映射
//!
//! 合并后的四边形使用拉伸 UV（将整个四边形映射到同一个纹理槽位）。
//! 对于大多数方块纹理（石头、泥土、沙子等），拉伸效果可接受。
//! 未来可通过纹理数组（Texture Array）实现正确的纹理平铺。
//!
//! # 性能预期
//!
//! | 指标         | 逐面生成（当前） | Greedy Meshing |
//! |-------------|----------------|----------------|
//! | 顶点数/区块  | 4000-8000      | 500-1500       |
//! | CPU 耗时/区块 | ~0.5-1.5ms     | ~0.1-0.3ms     |

use crate::chunk::{BlockId, CHUNK_SIZE, ChunkData, ChunkNeighbors};

// ---------------------------------------------------------------------------
// 公共类型
// ---------------------------------------------------------------------------

/// Greedy Meshing 生成结果。
///
/// 包含构建 Bevy `Mesh` 所需的所有顶点数据。
pub struct GreedyMeshResult {
    pub positions: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub normals: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
}

// ---------------------------------------------------------------------------
// 面方向配置
// ---------------------------------------------------------------------------

/// 面方向配置。
///
/// 定义了面的法线方向、掩码轴映射、邻居索引等信息，
/// 用于通用化的贪心合并处理，避免为 6 个面方向重复编写逻辑。
struct FaceConfig {
    /// 纹理映射用的面名称（"top", "bottom", "side"）
    face_name: &'static str,
    /// 在 `ChunkNeighbors` 中的索引（0=+X, 1=-X, 2=+Y, 3=-Y, 4=+Z, 5=-Z）
    face_index: usize,
    /// 法线轴：0=X, 1=Y, 2=Z
    normal_axis: usize,
    /// 法线方向：+1 或 -1
    normal_sign: i32,
    /// 掩码第一轴（对应合并宽度方向，即 u 方向）
    u_axis: usize,
    /// 掩码第二轴（对应合并高度方向，即 v 方向）
    v_axis: usize,
}

/// 6 个面方向的配置，顺序与 `chunk::FACES` 一致。
const FACE_CONFIGS: [FaceConfig; 6] = [
    // Right (+X)
    FaceConfig {
        face_name: "side",
        face_index: 0,
        normal_axis: 0,
        normal_sign: 1,
        u_axis: 2,
        v_axis: 1,
    },
    // Left (-X)
    FaceConfig {
        face_name: "side",
        face_index: 1,
        normal_axis: 0,
        normal_sign: -1,
        u_axis: 2,
        v_axis: 1,
    },
    // Top (+Y)
    FaceConfig {
        face_name: "top",
        face_index: 2,
        normal_axis: 1,
        normal_sign: 1,
        u_axis: 0,
        v_axis: 2,
    },
    // Bottom (-Y)
    FaceConfig {
        face_name: "bottom",
        face_index: 3,
        normal_axis: 1,
        normal_sign: -1,
        u_axis: 0,
        v_axis: 2,
    },
    // Front (+Z)
    FaceConfig {
        face_name: "side",
        face_index: 4,
        normal_axis: 2,
        normal_sign: 1,
        u_axis: 0,
        v_axis: 1,
    },
    // Back (-Z)
    FaceConfig {
        face_name: "side",
        face_index: 5,
        normal_axis: 2,
        normal_sign: -1,
        u_axis: 0,
        v_axis: 1,
    },
];

// ---------------------------------------------------------------------------
// UV 收缩常量
// ---------------------------------------------------------------------------

/// UV 收缩量，防止双线性插值导致的纹理边缘渗色（约 0.5px / 32px）。
/// 与 `chunk::face_quad` 中的 `eps` 保持一致。
const UV_EPS: f32 = 0.016;

// ---------------------------------------------------------------------------
// 公共接口
// ---------------------------------------------------------------------------

/// 生成 Greedy Mesh。
///
/// 将相邻同材质方块面合并为更大的四边形，减少顶点数。
///
/// # 参数
/// - `chunk`: 区块数据（三态存储）
/// - `neighbors`: 6 个方向的邻居区块数据，用于跨区块面剔除
/// - `get_uv`: UV 查找回调，给定 `(block_id, face_name)` 返回 `(u_min, u_max, v_min, v_max)`
///
/// # 返回
/// 包含 `positions`, `uvs`, `normals`, `indices` 的结果结构体。
pub fn generate_greedy_mesh<F>(
    chunk: &ChunkData,
    neighbors: &ChunkNeighbors,
    get_uv: F,
) -> GreedyMeshResult
where
    F: Fn(u8, &str) -> (f32, f32, f32, f32),
{
    // 全空气区块提前返回
    if matches!(chunk, ChunkData::Empty | ChunkData::Uniform(0)) {
        return GreedyMeshResult {
            positions: Vec::new(),
            uvs: Vec::new(),
            normals: Vec::new(),
            indices: Vec::new(),
        };
    }

    // 预分配容量（Greedy Meshing 后顶点数大幅减少）
    let mut positions = Vec::with_capacity(8000);
    let mut uvs = Vec::with_capacity(8000);
    let mut normals = Vec::with_capacity(8000);
    let mut indices = Vec::with_capacity(12000);

    // 掩码和已消费标记（每层复用，避免重复分配）
    let mut mask = [[None::<BlockId>; CHUNK_SIZE]; CHUNK_SIZE];
    let mut consumed = [[false; CHUNK_SIZE]; CHUNK_SIZE];

    for face_config in &FACE_CONFIGS {
        process_face(
            chunk,
            neighbors,
            face_config,
            &get_uv,
            &mut mask,
            &mut consumed,
            &mut positions,
            &mut uvs,
            &mut normals,
            &mut indices,
        );
    }

    GreedyMeshResult {
        positions,
        uvs,
        normals,
        indices,
    }
}

// ---------------------------------------------------------------------------
// 内部实现
// ---------------------------------------------------------------------------

/// 处理单个面方向的所有层。
///
/// 沿法线方向逐层扫描，对每层构建掩码并执行贪心合并。
fn process_face<F>(
    chunk: &ChunkData,
    neighbors: &ChunkNeighbors,
    face: &FaceConfig,
    get_uv: &F,
    mask: &mut [[Option<BlockId>; CHUNK_SIZE]; CHUNK_SIZE],
    consumed: &mut [[bool; CHUNK_SIZE]; CHUNK_SIZE],
    positions: &mut Vec<[f32; 3]>,
    uvs: &mut Vec<[f32; 2]>,
    normals: &mut Vec<[f32; 3]>,
    indices: &mut Vec<u32>,
) where
    F: Fn(u8, &str) -> (f32, f32, f32, f32),
{
    // 法线方向的偏移向量
    let normal_offset = {
        let mut n = [0i32; 3];
        n[face.normal_axis] = face.normal_sign;
        n
    };

    // 沿法线方向逐层扫描
    for layer in 0..CHUNK_SIZE {
        // 1. 构建 2D 可见性掩码
        build_mask(chunk, neighbors, face, layer, &normal_offset, mask);

        // 2. 重置已消费标记
        for row in consumed.iter_mut() {
            row.fill(false);
        }

        // 3. 贪心合并：扫描掩码，合并同材质连续区域
        for v in 0..CHUNK_SIZE {
            for u in 0..CHUNK_SIZE {
                if consumed[v][u] || mask[v][u].is_none() {
                    continue;
                }

                let block_id = mask[v][u].unwrap();

                // 找宽度：沿 u 方向延伸，直到遇到不同材质或已消费格子
                let mut width = 1usize;
                while u + width < CHUNK_SIZE
                    && !consumed[v][u + width]
                    && mask[v][u + width] == Some(block_id)
                {
                    width += 1;
                }

                // 找高度：沿 v 方向延伸，检查整行是否都能匹配
                let mut height = 1usize;
                'extend_v: while v + height < CHUNK_SIZE {
                    for du in 0..width {
                        if consumed[v + height][u + du]
                            || mask[v + height][u + du] != Some(block_id)
                        {
                            break 'extend_v;
                        }
                    }
                    height += 1;
                }

                // 标记已消费区域
                for dv in 0..height {
                    for du in 0..width {
                        consumed[v + dv][u + du] = true;
                    }
                }

                // 4. 生成合并后的四边形顶点
                emit_quad(
                    layer, u, v, width, height, block_id, face, get_uv, positions, uvs, normals,
                    indices,
                );
            }
        }
    }
}

/// 构建单层的 2D 可见性掩码。
///
/// 遍历掩码的每个格子 `(u, v)`，映射到区块坐标 `(x, y, z)`，
/// 检查该位置的方块面是否需要渲染。
///
/// - `mask[v][u] = Some(block_id)` 表示该面可见，需要渲染
/// - `mask[v][u] = None` 表示该面被遮挡或是空气
fn build_mask(
    chunk: &ChunkData,
    neighbors: &ChunkNeighbors,
    face: &FaceConfig,
    layer: usize,
    normal_offset: &[i32; 3],
    mask: &mut [[Option<BlockId>; CHUNK_SIZE]; CHUNK_SIZE],
) {
    for v in 0..CHUNK_SIZE {
        for u in 0..CHUNK_SIZE {
            // 将 (layer, u, v) 映射到区块局部坐标 (x, y, z)
            let mut pos = [0usize; 3];
            pos[face.normal_axis] = layer;
            pos[face.u_axis] = u;
            pos[face.v_axis] = v;
            let (x, y, z) = (pos[0], pos[1], pos[2]);

            let block_id = chunk.get(x, y, z);
            if block_id == 0 {
                // 空气方块，不需要渲染任何面
                mask[v][u] = None;
                continue;
            }

            // 计算邻居位置（沿法线方向偏移一格）
            let nx = x as i32 + normal_offset[0];
            let ny = y as i32 + normal_offset[1];
            let nz = z as i32 + normal_offset[2];

            // 检查邻居是否在当前区块内
            if nx >= 0
                && ny >= 0
                && nz >= 0
                && nx < CHUNK_SIZE as i32
                && ny < CHUNK_SIZE as i32
                && nz < CHUNK_SIZE as i32
            {
                // 邻居在区块内，直接查询
                let neighbor_id = chunk.get(nx as usize, ny as usize, nz as usize);
                mask[v][u] = if neighbor_id != block_id {
                    Some(block_id)
                } else {
                    None
                };
            } else {
                // 邻居在区块外，查询邻居区块数据
                let neighbor_x = nx.rem_euclid(CHUNK_SIZE as i32) as usize;
                let neighbor_y = ny.rem_euclid(CHUNK_SIZE as i32) as usize;
                let neighbor_z = nz.rem_euclid(CHUNK_SIZE as i32) as usize;
                let neighbor_id = neighbors.get_neighbor_block(
                    face.face_index,
                    neighbor_x,
                    neighbor_y,
                    neighbor_z,
                );
                mask[v][u] = if neighbor_id != block_id {
                    Some(block_id)
                } else {
                    None
                };
            }
        }
    }
}

/// 为合并后的四边形生成顶点数据。
///
/// 将 `(layer, u_start, v_start, width, height)` 映射回区块局部坐标，
/// 生成 4 个顶点位置、UV 坐标、法线和索引。
///
/// 顶点缠绕顺序与 `chunk::face_quad()` 保持一致，确保面朝向正确。
fn emit_quad<F>(
    layer: usize,
    u_start: usize,
    v_start: usize,
    width: usize,
    height: usize,
    block_id: BlockId,
    face: &FaceConfig,
    get_uv: &F,
    positions: &mut Vec<[f32; 3]>,
    uvs: &mut Vec<[f32; 2]>,
    normals: &mut Vec<[f32; 3]>,
    indices: &mut Vec<u32>,
) where
    F: Fn(u8, &str) -> (f32, f32, f32, f32),
{
    let base_index = positions.len() as u32;
    let w = width as f32;
    let h = height as f32;

    // 将 (layer, u_start, v_start) 映射到区块局部坐标 (x, y, z)
    let mut origin = [0usize; 3];
    origin[face.normal_axis] = layer;
    origin[face.u_axis] = u_start;
    origin[face.v_axis] = v_start;
    let (ox, oy, oz) = (origin[0] as f32, origin[1] as f32, origin[2] as f32);

    // 面位于 layer + max(0, normal_sign) 的位置
    // +法线方向：面在 layer+1（如 Top 面在 y+1）
    // -法线方向：面在 layer（如 Bottom 面在 y）
    let face_offset = if face.normal_sign > 0 { 1.0 } else { 0.0 };

    // 生成 4 个顶点，缠绕顺序与 chunk::face_quad() 一致
    let verts = match (face.normal_axis, face.normal_sign) {
        // +X (Right): 宽度沿 Z 轴，高度沿 Y 轴
        (0, 1) => [
            [ox + face_offset, oy, oz],
            [ox + face_offset, oy, oz + w],
            [ox + face_offset, oy + h, oz + w],
            [ox + face_offset, oy + h, oz],
        ],
        // -X (Left): 宽度沿 Z 轴，高度沿 Y 轴
        (0, -1) => [
            [ox + face_offset, oy, oz + w],
            [ox + face_offset, oy, oz],
            [ox + face_offset, oy + h, oz],
            [ox + face_offset, oy + h, oz + w],
        ],
        // +Y (Top): 宽度沿 X 轴，高度沿 Z 轴
        (1, 1) => [
            [ox, oy + face_offset, oz],
            [ox + w, oy + face_offset, oz],
            [ox + w, oy + face_offset, oz + h],
            [ox, oy + face_offset, oz + h],
        ],
        // -Y (Bottom): 宽度沿 X 轴，高度沿 Z 轴
        (1, -1) => [
            [ox, oy + face_offset, oz + h],
            [ox + w, oy + face_offset, oz + h],
            [ox + w, oy + face_offset, oz],
            [ox, oy + face_offset, oz],
        ],
        // +Z (Front): 宽度沿 X 轴，高度沿 Y 轴
        (2, 1) => [
            [ox + w, oy, oz + face_offset],
            [ox, oy, oz + face_offset],
            [ox, oy + h, oz + face_offset],
            [ox + w, oy + h, oz + face_offset],
        ],
        // -Z (Back): 宽度沿 X 轴，高度沿 Y 轴
        (2, -1) => [
            [ox, oy, oz + face_offset],
            [ox + w, oy, oz + face_offset],
            [ox + w, oy + h, oz + face_offset],
            [ox, oy + h, oz + face_offset],
        ],
        _ => unreachable!(
            "invalid face config: axis={}, sign={}",
            face.normal_axis, face.normal_sign
        ),
    };

    // 法线向量
    let normal = {
        let mut n = [0.0f32; 3];
        n[face.normal_axis] = face.normal_sign as f32;
        n
    };

    // UV 坐标（拉伸模式：整个合并四边形映射到同一个纹理槽位）
    let (u_min, u_max, v_min, v_max) = get_uv(block_id, face.face_name);
    let face_uvs = [
        [u_min + UV_EPS, v_max - UV_EPS],
        [u_max - UV_EPS, v_max - UV_EPS],
        [u_max - UV_EPS, v_min + UV_EPS],
        [u_min + UV_EPS, v_min + UV_EPS],
    ];

    // 追加顶点数据
    positions.extend_from_slice(&verts);
    uvs.extend_from_slice(&face_uvs);
    normals.extend([normal; 4]);

    // 追加索引（两个三角形，缠绕顺序与 chunk.rs 一致）
    indices.extend([
        base_index,
        base_index + 2,
        base_index + 1,
        base_index,
        base_index + 3,
        base_index + 2,
    ]);
}
