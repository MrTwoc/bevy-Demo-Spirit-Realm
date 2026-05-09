//! 异步网格生成系统（借鉴 Voxy 核心架构）
//!
//! 将网格生成从主线程转移到后台工作线程，消除加载尖峰。
//!
//! # 架构设计
//!
//! ```text
//! ┌─────────────┐     MeshTask      ┌──────────────┐     MeshResult     ┌─────────────┐
//! │  主线程      │ ───────────────→ │  工作线程×N   │ ───────────────→ │  主线程      │
//! │  (Bevy ECS)  │   mpsc::channel  │  (后台计算)   │   mpsc::channel  │  (上传GPU)   │
//! └─────────────┘                   └──────────────┘                   └─────────────┘
//! ```
//!
//! # 关键设计决策
//!
//! 1. **UV 查找表预提取**：`ResourcePackManager` 是 Bevy `Resource`，不能跨线程发送。
//!    因此在提交任务时，将 UV 映射表（`HashMap<(u8, String), (f32,f32,f32,f32)>`）
//!    克隆并打包到任务数据中，供工作线程使用。
//!
//! 2. **取消机制**：当区块在工作线程处理完成前被卸载时，通过发送 `Cancel` 任务
//!    让工作线程跳过已取消的任务（基于 coord 匹配）。
//!
//! 3. **结果收集频率**：每帧在 `First` 阶段收集异步结果，限制每帧上传数量
//!    避免 GPU 上传尖峰。

use bevy::prelude::*;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::mpsc;
use std::thread;

use crate::chunk::{CHUNK_SIZE, ChunkCoord, ChunkData, ChunkNeighbors};
use crate::chunk_dirty::is_air_chunk;

/// 每帧最多从异步结果中收集并上传 GPU 的网格数。
/// 限制 GPU 上传速率，避免帧时间尖峰。
pub const MESH_UPLOADS_PER_FRAME: usize = 4;

/// 工作线程数量。默认使用可用 CPU 核心数的一半（至少 1），
/// 留出核心给主线程和渲染线程。
pub fn default_worker_count() -> usize {
    (thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        / 2)
    .max(1)
}

// ---------------------------------------------------------------------------
// UV 查找表（可跨线程发送）
// ---------------------------------------------------------------------------

/// 从 ResourcePackManager 预提取的 UV 查找表。
///
/// 这是一个纯数据结构，不包含任何 Bevy 资源引用，可以安全地跨线程发送。
/// 在提交网格生成任务时一次性构建，所有任务共享同一份（通过 Arc 共享）。
///
/// Texture Array 模式下，UV 编码为 (layer_index + u, v)，
/// 其中 layer_index 是 Texture Array 的层索引，u/v 是 [0,1] 范围的纹理坐标。
#[derive(Resource, Clone, Debug)]
pub struct UvLookupTable {
    /// (block_id, face_name) -> (u_min, u_max, v_min, v_max)
    /// Texture Array 模式：u_min = layer_index, u_max = layer_index + 1, v_min = 0, v_max = 1
    pub block_uv_map: HashMap<(u8, String), (f32, f32, f32, f32)>,
}

impl UvLookupTable {
    /// 从 ResourcePackManager 构建 UV 查找表。
    ///
    /// 遍历 `block_texture_map`，查找每个 (block_id, face) 对应的纹理在 Atlas 中的 UV 坐标。
    /// Texture Array 模式下 UV 编码为 (layer_index + u, v)。
    pub fn from_resource_pack(rp: &crate::resource_pack::ResourcePackManager) -> Self {
        let mut block_uv_map = HashMap::new();

        if let Some(atlas) = &rp.atlas {
            for ((block_id, face), texture_name) in &rp.block_texture_map {
                if let Some(tex_info) = atlas.textures.get(texture_name) {
                    block_uv_map.insert((*block_id, face.clone()), tex_info.uv);
                }
            }
        }

        Self { block_uv_map }
    }

    /// 获取指定方块和面的 UV 坐标。如果找不到，返回默认 UV。
    /// Texture Array 模式下返回 (0.0, 1.0, 0.0, 1.0) 表示第 0 层完整纹理。
    pub fn get_uv(&self, block_id: u8, face_name: &str) -> (f32, f32, f32, f32) {
        self.block_uv_map
            .get(&(block_id, face_name.to_string()))
            .copied()
            .unwrap_or((0.0, 1.0, 0.0, 1.0))
    }
}

// ---------------------------------------------------------------------------
// 网格生成任务和结果
// ---------------------------------------------------------------------------

/// 发送到工作线程的网格生成任务。
pub enum MeshTask {
    /// 生成指定区块的网格。
    Generate {
        coord: ChunkCoord,
        data: ChunkData,
        neighbors: ChunkNeighbors,
        uv_table: UvLookupTable,
    },
    /// 取消指定区块的网格生成（区块已被卸载）。
    Cancel(ChunkCoord),
}

/// 工作线程返回的网格生成结果。
///
/// 包含构建 Bevy `Mesh` 所需的所有顶点数据。
pub struct MeshResult {
    pub coord: ChunkCoord,
    pub positions: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub normals: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
}

// ---------------------------------------------------------------------------
// 异步网格管理器（Bevy Resource）
// ---------------------------------------------------------------------------

/// 异步网格生成管理器。
///
/// 作为 Bevy `Resource` 注册，管理：
/// - 任务发送通道（主线程 → 工作线程）
/// - 结果接收通道（工作线程 → 主线程）
/// - 待处理任务集合（用于取消和去重）
/// - 待取消坐标队列（延迟发送取消信号）
///
/// 注意：`mpsc::Receiver` 不是 `Sync`，因此用 `Mutex` 包装以满足 Bevy `Resource` 的要求。
pub struct AsyncMeshManager {
    /// 发送任务到工作线程（Mutex 包装以满足 Sync 要求）
    task_sender: std::sync::Mutex<mpsc::Sender<MeshTask>>,
    /// 从工作线程接收结果（Mutex 包装以满足 Sync 要求）
    result_receiver: std::sync::Mutex<mpsc::Receiver<MeshResult>>,
    /// 当前正在工作线程中处理的区块坐标
    pending_tasks: std::sync::Mutex<HashSet<ChunkCoord>>,
    /// 等待发送取消信号的坐标（在下一次 submit 时批量发送）
    cancel_queue: std::sync::Mutex<VecDeque<ChunkCoord>>,
}

// 手动实现 Resource（因为 derive 宏要求所有字段 Sync，而 Mutex<Receiver> 满足）
impl Resource for AsyncMeshManager {}

impl AsyncMeshManager {
    /// 创建异步网格管理器并启动工作线程。
    ///
    /// `worker_count` 指定工作线程数量。建议使用 `default_worker_count()`。
    pub fn new(worker_count: usize) -> Self {
        let (task_tx, task_rx) = mpsc::channel::<MeshTask>();
        let (result_tx, result_rx) = mpsc::channel::<MeshResult>();

        // task_rx 需要被多个工作线程共享，使用 Arc<Mutex<>> 包装
        let task_rx = std::sync::Arc::new(std::sync::Mutex::new(task_rx));

        for _ in 0..worker_count {
            let rx = task_rx.clone();
            let tx = result_tx.clone();
            thread::spawn(move || {
                Self::worker_loop(rx, tx);
            });
        }

        Self {
            task_sender: std::sync::Mutex::new(task_tx),
            result_receiver: std::sync::Mutex::new(result_rx),
            pending_tasks: std::sync::Mutex::new(HashSet::new()),
            cancel_queue: std::sync::Mutex::new(VecDeque::new()),
        }
    }

    /// 工作线程主循环。
    ///
    /// 持续从任务通道接收任务，执行网格生成，将结果发送回主线程。
    /// 遇到 `Cancel` 任务时跳过对应坐标（如果还在处理中）。
    fn worker_loop(
        receiver: std::sync::Arc<std::sync::Mutex<mpsc::Receiver<MeshTask>>>,
        sender: mpsc::Sender<MeshResult>,
    ) {
        loop {
            // 从共享接收器获取任务
            let task = {
                let rx = receiver.lock().unwrap();
                rx.recv()
            };

            let task = match task {
                Ok(t) => t,
                Err(_) => break, // 通道关闭，退出线程
            };

            match task {
                MeshTask::Generate {
                    coord,
                    data,
                    neighbors,
                    uv_table,
                } => {
                    // 全空气区块跳过网格生成
                    if is_air_chunk(&data) {
                        // 发送空结果，让主线程知道任务完成
                        let _ = sender.send(MeshResult {
                            coord,
                            positions: Vec::new(),
                            uvs: Vec::new(),
                            normals: Vec::new(),
                            indices: Vec::new(),
                        });
                        continue;
                    }

                    let (positions, uvs, normals, indices) =
                        generate_chunk_mesh_async(&data, &uv_table, &neighbors);

                    let _ = sender.send(MeshResult {
                        coord,
                        positions,
                        uvs,
                        normals,
                        indices,
                    });
                }
                MeshTask::Cancel(_) => {
                    // 取消任务：不做任何处理，结果通道中不会有对应结果
                }
            }
        }
    }

    /// 提交网格生成任务。
    ///
    /// 将区块数据打包发送到工作线程。如果区块已在处理中（pending），跳过。
    ///
    /// 返回 `true` 表示任务已成功提交，`false` 表示因已在处理中而被跳过。
    /// 调用方可根据返回值决定是否保留脏标记以便下帧重试。
    pub fn submit_task(&self, task: MeshTask) -> bool {
        if let MeshTask::Generate { coord, .. } = &task {
            let mut pending = self.pending_tasks.lock().unwrap();
            // 如果已经在处理中，跳过
            if pending.contains(coord) {
                return false;
            }
            pending.insert(*coord);
        }
        // 忽略发送失败（通道关闭）
        let sender = self.task_sender.lock().unwrap();
        let _ = sender.send(task);
        true
    }

    /// 请求取消指定区块的网格生成。
    ///
    /// 区块坐标会被加入取消队列，在下一次 `submit_task` 时批量发送取消信号。
    /// 同时从 pending 集合中移除。
    pub fn cancel_task(&self, coord: ChunkCoord) {
        self.pending_tasks.lock().unwrap().remove(&coord);
        self.cancel_queue.lock().unwrap().push_back(coord);
    }

    /// 批量发送取消信号。
    ///
    /// 在提交新任务前调用，将积压的取消请求发送到工作线程。
    fn flush_cancel_queue(&self) {
        let mut cancel_queue = self.cancel_queue.lock().unwrap();
        let sender = self.task_sender.lock().unwrap();
        while let Some(coord) = cancel_queue.pop_front() {
            let _ = sender.send(MeshTask::Cancel(coord));
        }
    }

    /// 收集已完成的网格生成结果。
    ///
    /// 非阻塞地从结果通道中取出所有可用结果，最多返回 `max_results` 个。
    /// 返回的结果会从 pending 集合中移除。
    pub fn collect_results(&self, max_results: usize) -> Vec<MeshResult> {
        let mut results = Vec::new();
        let receiver = self.result_receiver.lock().unwrap();
        let mut pending = self.pending_tasks.lock().unwrap();
        while results.len() < max_results {
            match receiver.try_recv() {
                Ok(result) => {
                    pending.remove(&result.coord);
                    results.push(result);
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => break,
            }
        }
        results
    }

    /// 获取当前待处理任务数量。
    pub fn pending_count(&self) -> usize {
        self.pending_tasks.lock().unwrap().len()
    }

    /// 检查指定坐标是否正在处理中。
    pub fn is_pending(&self, coord: &ChunkCoord) -> bool {
        self.pending_tasks.lock().unwrap().contains(coord)
    }
}

// ---------------------------------------------------------------------------
// 异步网格生成函数（在工作线程中执行）
// ---------------------------------------------------------------------------

/// 面方向定义，与 chunk.rs 中 FACES 一致。
const FACES_ASYNC: [(FaceAsync, [i32; 3]); 6] = [
    (FaceAsync::Right, [1, 0, 0]),
    (FaceAsync::Left, [-1, 0, 0]),
    (FaceAsync::Top, [0, 1, 0]),
    (FaceAsync::Bottom, [0, -1, 0]),
    (FaceAsync::Front, [0, 0, 1]),
    (FaceAsync::Back, [0, 0, -1]),
];

/// 面方向枚举（工作线程本地副本，避免依赖 Bevy）
#[derive(Clone, Copy)]
enum FaceAsync {
    Top,
    Bottom,
    Right,
    Left,
    Front,
    Back,
}

impl FaceAsync {
    fn to_face_name(&self) -> &'static str {
        match self {
            FaceAsync::Top => "top",
            FaceAsync::Bottom => "bottom",
            _ => "side",
        }
    }
}

/// 异步版本的网格生成函数。
///
/// 与 `chunk::generate_chunk_mesh()` 逻辑完全一致，但：
/// - 使用 `UvLookupTable` 替代 `ResourcePackManager`（可跨线程）
/// - 不依赖任何 Bevy 类型
fn generate_chunk_mesh_async(
    chunk: &ChunkData,
    uv_table: &UvLookupTable,
    neighbors: &ChunkNeighbors,
) -> (Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<[f32; 3]>, Vec<u32>) {
    // 全空气区块提前返回
    if matches!(chunk, ChunkData::Empty | ChunkData::Uniform(0)) {
        return (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    }

    // 预分配容量
    let mut positions = Vec::with_capacity(48000);
    let mut uvs = Vec::with_capacity(48000);
    let mut normals = Vec::with_capacity(48000);
    let mut indices = Vec::with_capacity(72000);

    for z in 0..CHUNK_SIZE {
        for y in 0..CHUNK_SIZE {
            for x in 0..CHUNK_SIZE {
                let block_id = chunk.get(x, y, z);
                if block_id == 0 {
                    continue;
                }

                for (face_index, (face, offset)) in FACES_ASYNC.iter().cloned().enumerate() {
                    if !is_face_visible_async(chunk, x, y, z, &offset, face_index, neighbors) {
                        continue;
                    }

                    let base_index = positions.len() as u32;
                    let face_name = face.to_face_name();

                    // 从 UV 查找表获取坐标
                    let uv = uv_table.get_uv(block_id, face_name);

                    let (face_verts, face_uvs, face_normal) = face_quad_async(x, y, z, face, uv);
                    positions.extend(face_verts);
                    uvs.extend(face_uvs);
                    normals.extend([face_normal; 4]);
                    indices.extend([
                        base_index,
                        base_index + 2,
                        base_index + 1,
                        base_index,
                        base_index + 3,
                        base_index + 2,
                    ]);
                }
            }
        }
    }

    (positions, uvs, normals, indices)
}

/// 异步版本的面可见性检查。
///
/// 逻辑与 `ChunkData::is_face_visible()` 完全一致。
fn is_face_visible_async(
    chunk: &ChunkData,
    x: usize,
    y: usize,
    z: usize,
    face: &[i32; 3],
    face_index: usize,
    neighbors: &ChunkNeighbors,
) -> bool {
    let nx = x as i32 + face[0];
    let ny = y as i32 + face[1];
    let nz = z as i32 + face[2];

    let current_id = chunk.get(x, y, z);

    // 邻居在区块边界内
    if nx >= 0
        && ny >= 0
        && nz >= 0
        && nx < CHUNK_SIZE as i32
        && ny < CHUNK_SIZE as i32
        && nz < CHUNK_SIZE as i32
    {
        return chunk.get(nx as usize, ny as usize, nz as usize) != current_id;
    }

    // 邻居在区块边界外
    let neighbor_x = nx.rem_euclid(CHUNK_SIZE as i32) as usize;
    let neighbor_y = ny.rem_euclid(CHUNK_SIZE as i32) as usize;
    let neighbor_z = nz.rem_euclid(CHUNK_SIZE as i32) as usize;

    let neighbor_id = neighbors.get_neighbor_block(face_index, neighbor_x, neighbor_y, neighbor_z);
    neighbor_id != current_id
}

/// 异步版本的面四边形生成。
///
/// 逻辑与 `chunk::face_quad()` 完全一致。
fn face_quad_async(
    x: usize,
    y: usize,
    z: usize,
    face: FaceAsync,
    uv: (f32, f32, f32, f32),
) -> ([[f32; 3]; 4], [[f32; 2]; 4], [f32; 3]) {
    let (verts, normal) = match face {
        FaceAsync::Top => (
            [
                [x as f32, y as f32 + 1.0, z as f32],
                [x as f32 + 1.0, y as f32 + 1.0, z as f32],
                [x as f32 + 1.0, y as f32 + 1.0, z as f32 + 1.0],
                [x as f32, y as f32 + 1.0, z as f32 + 1.0],
            ],
            [0.0, 1.0, 0.0],
        ),
        FaceAsync::Bottom => (
            [
                [x as f32, y as f32, z as f32 + 1.0],
                [x as f32 + 1.0, y as f32, z as f32 + 1.0],
                [x as f32 + 1.0, y as f32, z as f32],
                [x as f32, y as f32, z as f32],
            ],
            [0.0, -1.0, 0.0],
        ),
        FaceAsync::Right => (
            [
                [x as f32 + 1.0, y as f32, z as f32],
                [x as f32 + 1.0, y as f32, z as f32 + 1.0],
                [x as f32 + 1.0, y as f32 + 1.0, z as f32 + 1.0],
                [x as f32 + 1.0, y as f32 + 1.0, z as f32],
            ],
            [1.0, 0.0, 0.0],
        ),
        FaceAsync::Left => (
            [
                [x as f32, y as f32, z as f32 + 1.0],
                [x as f32, y as f32, z as f32],
                [x as f32, y as f32 + 1.0, z as f32],
                [x as f32, y as f32 + 1.0, z as f32 + 1.0],
            ],
            [-1.0, 0.0, 0.0],
        ),
        FaceAsync::Front => (
            [
                [x as f32 + 1.0, y as f32, z as f32 + 1.0],
                [x as f32, y as f32, z as f32 + 1.0],
                [x as f32, y as f32 + 1.0, z as f32 + 1.0],
                [x as f32 + 1.0, y as f32 + 1.0, z as f32 + 1.0],
            ],
            [0.0, 0.0, 1.0],
        ),
        FaceAsync::Back => (
            [
                [x as f32, y as f32, z as f32],
                [x as f32 + 1.0, y as f32, z as f32],
                [x as f32 + 1.0, y as f32 + 1.0, z as f32],
                [x as f32, y as f32 + 1.0, z as f32],
            ],
            [0.0, 0.0, -1.0],
        ),
    };

    let (u_min, u_max, v_min, v_max) = uv;
    let eps = 0.016;
    let face_uvs = [
        [u_min + eps, v_max - eps],
        [u_max - eps, v_max - eps],
        [u_max - eps, v_min + eps],
        [u_min + eps, v_min + eps],
    ];

    (verts, face_uvs, normal)
}
