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
//!    因此在提交任务时，将 UV 映射表（`HashMap<(u8, String), (f32,f32,f32,f32)`）克隆并打包到任务数据中。
//!
//! 2. **取消机制**：当区块在工作线程处理完成前被卸载时，通过发送 `Cancel` 任务让工作线程跳过已取消的任务。
//!
//! 3. **结果收集频率**：每帧在 `First` 阶段收集异步结果，限制每帧上传数量避免 GPU 上传尖峰。

use bevy::prelude::*;
use std::collections::{HashSet, VecDeque};
use std::sync::{mpsc, Arc};
use std::thread;

use crate::chunk::{ChunkCoord, ChunkData, ChunkNeighbors, fill_terrain, is_block_solid, CHUNK_SIZE};
use crate::chunk_dirty::is_air_chunk;
use crate::lod::{generate_lod_mesh, LodLevel};
use crate::tree_gen::{generate_trees_in_chunk, TreeConfig, TreeNoise};

/// 每帧最多从异步结果中收集并上传 GPU 的网格数。
/// 限制 GPU 上传速率，避免帧时间尖峰。
pub const MESH_UPLOADS_PER_FRAME: usize = 32;

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

/// UV 类型别名：(u_min, u_max, v_min, v_max)
pub type UvCoord = (f32, f32, f32, f32);

/// 默认 UV 坐标
const DEFAULT_UV: UvCoord = (0.0, 1.0, 0.0, 1.0);

/// 面名称到索引的映射。
pub const fn face_name_to_index(face_name: &str) -> usize {
    let bytes = face_name.as_bytes();
    if bytes.len() == 3 && bytes[0] == b't' && bytes[1] == b'o' && bytes[2] == b'p' {
        0
    } else if bytes.len() == 6 && bytes[0] == b'b' && bytes[1] == b'o' {
        1
    } else {
        2
    }
}

/// 从 ResourcePackManager 预提取的 UV 查找表。
#[derive(Resource, Clone, Debug)]
pub struct UvLookupTable {
    uv_array: [[Option<UvCoord>; 3]; 256],
}

impl UvLookupTable {
    pub fn from_resource_pack(rp: &crate::resource_pack::ResourcePackManager) -> Self {
        let mut uv_array = [[None; 3]; 256];

        if let Some(atlas) = &rp.atlas {
            for ((block_id, face), texture_name) in &rp.block_texture_map {
                if let Some(tex_info) = atlas.textures.get(texture_name) {
                    let fi = face_name_to_index(face);
                    uv_array[*block_id as usize][fi] = Some(tex_info.uv);
                }
            }
        }

        Self { uv_array }
    }

    #[inline]
    pub fn get_uv(&self, block_id: u8, face_index: usize) -> UvCoord {
        self.uv_array[block_id as usize][face_index].unwrap_or(DEFAULT_UV)
    }
}

// ---------------------------------------------------------------------------
// 网格生成任务和结果
// ---------------------------------------------------------------------------

/// 区块数据准备结果（地形+树木生成完成后返回）。
pub struct PrepareResult {
    pub coord: ChunkCoord,
    pub data: ChunkData,
}

/// 发送到工作线程的网格生成任务。
pub enum MeshTask {
    /// 准备区块数据：地形生成 + 树木生成（CPU 密集型，移至工作线程）。
    Prepare {
        coord: ChunkCoord,
    },
    /// 网格生成：在准备好的区块数据上执行面剔除 + 网格构建。
    Generate {
        coord: ChunkCoord,
        data: Arc<ChunkData>,
        neighbors: ChunkNeighbors,
        lod_level: Option<LodLevel>,
    },
    Cancel(ChunkCoord),
}

/// 工作线程返回的网格生成结果。
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
#[derive(Resource)]
pub struct AsyncMeshManager {
    task_sender: std::sync::Mutex<mpsc::Sender<MeshTask>>,
    mesh_receiver: std::sync::Mutex<mpsc::Receiver<MeshResult>>,
    prepare_receiver: std::sync::Mutex<mpsc::Receiver<PrepareResult>>,
    pending_tasks: std::sync::Mutex<HashSet<ChunkCoord>>,
    prepare_pending: std::sync::Mutex<HashSet<ChunkCoord>>,
    cancel_queue: std::sync::Mutex<VecDeque<ChunkCoord>>,
    uv_table: Arc<UvLookupTable>,
    tree_config: Arc<TreeConfig>,
    tree_noise: Arc<TreeNoise>,
}

impl AsyncMeshManager {
    pub fn new(
        worker_count: usize,
        uv_table: UvLookupTable,
        tree_config: TreeConfig,
        tree_noise: TreeNoise,
    ) -> Self {
        let (task_tx, task_rx) = mpsc::channel::<MeshTask>();
        let (mesh_tx, mesh_rx) = mpsc::channel::<MeshResult>();
        let (prepare_tx, prepare_rx) = mpsc::channel::<PrepareResult>();

        let task_rx = Arc::new(std::sync::Mutex::new(task_rx));
        let uv_table = Arc::new(uv_table);
        let tree_config = Arc::new(tree_config);
        let tree_noise = Arc::new(tree_noise);

        for _ in 0..worker_count {
            let rx = task_rx.clone();
            let mesh_tx = mesh_tx.clone();
            let prepare_tx = prepare_tx.clone();
            let uv = uv_table.clone();
            let tc = tree_config.clone();
            let tn = tree_noise.clone();
            thread::spawn(move || {
                Self::worker_loop(rx, mesh_tx, prepare_tx, uv, tc, tn);
            });
        }

        Self {
            task_sender: std::sync::Mutex::new(task_tx),
            mesh_receiver: std::sync::Mutex::new(mesh_rx),
            prepare_receiver: std::sync::Mutex::new(prepare_rx),
            pending_tasks: std::sync::Mutex::new(HashSet::new()),
            prepare_pending: std::sync::Mutex::new(HashSet::new()),
            cancel_queue: std::sync::Mutex::new(VecDeque::new()),
            uv_table,
            tree_config,
            tree_noise,
        }
    }

    fn worker_loop(
        receiver: Arc<std::sync::Mutex<mpsc::Receiver<MeshTask>>>,
        mesh_sender: mpsc::Sender<MeshResult>,
        prepare_sender: mpsc::Sender<PrepareResult>,
        uv_table: Arc<UvLookupTable>,
        tree_config: Arc<TreeConfig>,
        tree_noise: Arc<TreeNoise>,
    ) {
        loop {
            let task = {
                let rx = receiver.lock().unwrap();
                rx.recv()
            };

            let task = match task {
                Ok(t) => t,
                Err(_) => break,
            };

            match task {
                MeshTask::Prepare { coord } => {
                    let mut chunk = ChunkData::filled(0);
                    fill_terrain(&mut chunk, &coord);
                    generate_trees_in_chunk(
                        &mut chunk,
                        &coord,
                        tree_config.as_ref(),
                        tree_noise.as_ref(),
                    );

                    // 始终发送结果（包括空区块），确保 prepare_pending 能被正确清除。
                    // 空区块的过滤在 collect_prepare_results 的消费者端进行。
                    let _ = prepare_sender.send(PrepareResult {
                        coord,
                        data: chunk,
                    });
                }
                MeshTask::Generate {
                    coord,
                    data,
                    neighbors,
                    lod_level,
                } => {
                    // data 是 Arc<ChunkData>，通过 as_ref() 获取 &ChunkData
                    if is_air_chunk(data.as_ref()) {
                        let _ = mesh_sender.send(MeshResult {
                            coord,
                            positions: Vec::new(),
                            uvs: Vec::new(),
                            normals: Vec::new(),
                            indices: Vec::new(),
                        });
                        continue;
                    }

                    let (positions, uvs, normals, indices) = match lod_level {
                        Some(LodLevel::Lod0) | None => {
                            generate_chunk_mesh_async(data.as_ref(), uv_table.as_ref(), &neighbors)
                        }
                        Some(lod) => {
                            generate_lod_mesh(data.as_ref(), uv_table.as_ref(), &neighbors, lod)
                        }
                    };

                    let _ = mesh_sender.send(MeshResult {
                        coord,
                        positions,
                        uvs,
                        normals,
                        indices,
                    });
                }
                MeshTask::Cancel(_) => {}
            }
        }
    }

    /// 提交网格生成任务（区块数据已准备好）。
    pub fn submit_task(&self, task: MeshTask) -> bool {
        if let MeshTask::Generate { coord, .. } = &task {
            let mut pending = self.pending_tasks.lock().unwrap();
            if pending.contains(coord) {
                return false;
            }
            pending.insert(*coord);
        }
        let sender = self.task_sender.lock().unwrap();
        let _ = sender.send(task);
        true
    }

    /// 提交区块数据准备任务（地形+树木生成，在工作线程中执行）。
    pub fn submit_prepare_task(&self, coord: ChunkCoord) -> bool {
        let mut pending = self.prepare_pending.lock().unwrap();
        if pending.contains(&coord) {
            return false;
        }
        pending.insert(coord);
        let sender = self.task_sender.lock().unwrap();
        let _ = sender.send(MeshTask::Prepare { coord });
        true
    }

    /// 取消指定区块的所有任务（准备 + 网格生成）。
    pub fn cancel_task(&self, coord: ChunkCoord) {
        self.pending_tasks.lock().unwrap().remove(&coord);
        self.prepare_pending.lock().unwrap().remove(&coord);
        self.cancel_queue.lock().unwrap().push_back(coord);
    }

    fn flush_cancel_queue(&self) {
        let mut cancel_queue = self.cancel_queue.lock().unwrap();
        let sender = self.task_sender.lock().unwrap();
        while let Some(coord) = cancel_queue.pop_front() {
            let _ = sender.send(MeshTask::Cancel(coord));
        }
    }

    /// 收集完成的网格生成结果。
    pub fn collect_results(&self, max_results: usize) -> Vec<MeshResult> {
        let mut results = Vec::new();
        let receiver = self.mesh_receiver.lock().unwrap();
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

    /// 收集完成的区块数据准备结果。
    pub fn collect_prepare_results(&self, max_results: usize) -> Vec<PrepareResult> {
        let mut results = Vec::new();
        let receiver = self.prepare_receiver.lock().unwrap();
        let mut prepare_pending = self.prepare_pending.lock().unwrap();
        while results.len() < max_results {
            match receiver.try_recv() {
                Ok(result) => {
                    // 如果 coord 已不在 prepare_pending 中（已被取消），丢弃结果
                    if prepare_pending.remove(&result.coord) {
                        results.push(result);
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => break,
            }
        }
        results
    }

    /// 总待处理任务数（准备 + 网格生成）。
    pub fn pending_count(&self) -> usize {
        self.pending_tasks.lock().unwrap().len()
            + self.prepare_pending.lock().unwrap().len()
    }

    /// 指定区块是否有网格生成任务待处理。
    pub fn is_pending(&self, coord: &ChunkCoord) -> bool {
        self.pending_tasks.lock().unwrap().contains(coord)
    }

    /// 指定区块是否有数据准备任务待处理。
    pub fn is_prepare_pending(&self, coord: &ChunkCoord) -> bool {
        self.prepare_pending.lock().unwrap().contains(coord)
    }
}

// ---------------------------------------------------------------------------
// 异步网格生成函数（在工作线程中执行）
// ---------------------------------------------------------------------------

/// 面方向定义
const FACES_ASYNC: [(FaceAsync, [i32; 3], usize); 6] = [
    (FaceAsync::Right, [1, 0, 0], 2),
    (FaceAsync::Left, [-1, 0, 0], 2),
    (FaceAsync::Top, [0, 1, 0], 0),
    (FaceAsync::Bottom, [0, -1, 0], 1),
    (FaceAsync::Front, [0, 0, 1], 2),
    (FaceAsync::Back, [0, 0, -1], 2),
];

#[derive(Clone, Copy)]
enum FaceAsync {
    Top,
    Bottom,
    Right,
    Left,
    Front,
    Back,
}

/// 预估区块的顶点数量
///
/// 根据区块类型和地表高度估算，避免过度预分配。
fn estimate_vertex_capacity(chunk: &ChunkData) -> usize {
    match chunk {
        ChunkData::Empty | ChunkData::Uniform(0) => 0,
        ChunkData::Uniform(_) => 2000,  // 全填充区块，预计有较多面
        ChunkData::Paletted(data) => {
            // 根据调色板大小估算
            // 2-3 种类型：地表区块，预计 1000-3000 顶点
            // 4+ 种类型：复杂区块，预计 2000-5000 顶点
            let palette_len = data.palette_len();
            if palette_len <= 2 {
                1000
            } else if palette_len <= 4 {
                2000
            } else {
                3000
            }
        }
    }
}

/// 异步版本的网格生成函数。
fn generate_chunk_mesh_async(
    chunk: &ChunkData,
    uv_table: &UvLookupTable,
    neighbors: &ChunkNeighbors,
) -> (Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<[f32; 3]>, Vec<u32>) {
    if matches!(chunk, ChunkData::Empty | ChunkData::Uniform(0)) {
        return (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    }

    // 动态预分配：根据区块类型估算顶点数量
    let capacity = estimate_vertex_capacity(chunk);
    let mut positions = Vec::with_capacity(capacity);
    let mut uvs = Vec::with_capacity(capacity);
    let mut normals = Vec::with_capacity(capacity);
    let mut indices = Vec::with_capacity(capacity * 3 / 2);

    for z in 0..CHUNK_SIZE {
        for y in 0..CHUNK_SIZE {
            for x in 0..CHUNK_SIZE {
                let block_id = chunk.get(x, y, z);
                if block_id == 0 {
                    continue;
                }

                for (face_index, (face, offset, uv_idx)) in FACES_ASYNC.iter().cloned().enumerate()
                {
                    if !is_face_visible_async(chunk, x, y, z, &offset, face_index, neighbors) {
                        continue;
                    }

                    let base_index = positions.len() as u32;
                    let uv = uv_table.get_uv(block_id, uv_idx);

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

    let neighbor_id = if nx >= 0
        && ny >= 0
        && nz >= 0
        && nx < CHUNK_SIZE as i32
        && ny < CHUNK_SIZE as i32
        && nz < CHUNK_SIZE as i32
    {
        chunk.get(nx as usize, ny as usize, nz as usize)
    } else {
        let neighbor_x = nx.rem_euclid(CHUNK_SIZE as i32) as usize;
        let neighbor_y = ny.rem_euclid(CHUNK_SIZE as i32) as usize;
        let neighbor_z = nz.rem_euclid(CHUNK_SIZE as i32) as usize;
        neighbors.get_neighbor_block(face_index, neighbor_x, neighbor_y, neighbor_z)
    };

    let current_id = chunk.get(x, y, z);

    // 优化1：相同类型的相邻方块（包括水）之间的面完全剔除
    if neighbor_id == current_id && neighbor_id != 0 {
        return false;
    }

    // 优化2：实体方块（草地、石头、泥土、沙）完全遮挡相邻面
    if is_block_solid(neighbor_id) {
        return false;
    }

    // 邻居是空气或不同类型的非实体方块时，渲染面
    true
}

/// 异步版本的面四边形生成。
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

    let u_min = uv.0;
    let u_max = uv.1;
    let v_min = uv.2;
    let v_max = uv.3;

    let face_uvs = [
        [u_min, v_max],
        [u_max, v_max],
        [u_max, v_min],
        [u_min, v_min],
    ];

    (verts, face_uvs, normal)
}
