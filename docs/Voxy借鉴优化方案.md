# 借鉴 Voxy 的优化方案（主方案）

> 基于 [MCRcortex/voxy](https://github.com/MCRcortex/voxy) 项目的核心技术，为 Spirit Realm 制定的统一优化路线图。
> 
> **目标**：实现 128+ 区块视距，保持 60+ FPS，支持 Y=±20480 超大垂直范围。
>
> **整合来源**：
> - [GPU噪声迁移方案.md](GPU噪声迁移方案.md) — GPU 噪声计算 + 网格优化分析
> - [优化建议总结.md](优化建议总结.md) — 性能基线与优化建议
>
> **已移除方案**：
> - ~~SuperChunk 合批~~ — 结构性缺陷（Y 尺寸不匹配、刚性合并条件、重建卡顿），实现复杂度高，收益可通过 LOD 替代
>
> **备选方案**：
> - Greedy Meshing — Voxy 项目实际未使用此技术，其核心是 GPU Compute Shader 面剔除 + MultiDrawIndirect。Greedy Meshing 作为 CPU 端优化手段保留为备选，当 LOD + GPU 面剔除仍无法满足性能需求时可考虑启用

---

## 一、Voxy 核心技术分析

### 1.1 Voxy 架构概览

```
┌─────────────────────────────────────────────────────────────┐
│                    Voxy Architecture                         │
├─────────────────────────────────────────────────────────────┤
│  Chunk Storage      │  LOD System      │  Render Pipeline   │
│  ─────────────      │  ──────────      │  ──────────────    │
│  · 16³ chunks       │  · 4 LOD levels  │  · Compute Shader  │
│  · Palette encoding │  · Distance-based│  · Frustum culling │
│  · Sparse storage   │  · Async meshing │  · Hi-Z occlusion  │
│                     │                  │  · Multi-draw      │
└─────────────────────────────────────────────────────────────┘
```

### 1.2 Voxy 关键技术特性

| 特性 | Voxy 实现 | 当前项目状态 | 优化优先级 |
|------|-----------|--------------|------------|
| **异步网格生成** | 后台线程 + 通道回传 | 同步生成（阻塞主线程） | P0 |
| ~~Greedy Meshing~~ | ~~面合并减少顶点~~ | ~~逐面生成~~ | **备选**（Voxy 未使用） |
| **LOD 系统** | 4 级 LOD，距离阈值 | 无 LOD | P1 |
| **GPU 面剔除** | Compute Shader | CPU 逐面检查 | P1 |
| **多级空间索引** | 层次化 Chunk 管理 | 单层 HashMap | P2 |
| **流式加载** | 按需加载 + 优先级队列 | 分帧加载队列 | 已实现 |
| **内存池** | 复用 Mesh 缓冲区 | 每次新建 Vec | P2 |

---

## 二、当前系统性能基线

待补充

## 三、优化方案详细设计

### 3.1 Phase 0：异步网格生成（借鉴 Voxy 核心）

#### 3.1.1 问题分析

待补充

#### 3.1.2 Voxy 方案

Voxy 使用 Rust 的 `std::sync::mpsc` 通道实现异步网格生成：

```rust
// Voxy 的异步网格生成模式
enum MeshTask {
    Generate { coord: ChunkCoord, data: ChunkData },
    Cancel { coord: ChunkCoord },
}

struct MeshWorker {
    receiver: Receiver<MeshTask>,
    sender: Sender<(ChunkCoord, MeshResult)>,
}

impl MeshWorker {
    fn run(&self) {
        while let Ok(task) = self.receiver.recv() {
            match task {
                MeshTask::Generate { coord, data } => {
                    let mesh = generate_mesh(data);
                    self.sender.send((coord, mesh)).ok();
                }
                MeshTask::Cancel(_) => { /* 跳过 */ }
            }
        }
    }
}
```

#### 3.1.3 Spirit Realm 实现方案

```rust
// src/async_mesh.rs

use std::sync::mpsc;
use std::thread;

/// 网格生成任务
pub enum MeshTask {
    Generate {
        coord: ChunkCoord,
        data: ChunkData,
        neighbors: ChunkNeighbors,
    },
    Cancel(ChunkCoord),
}

/// 网格生成结果
pub struct MeshResult {
    pub coord: ChunkCoord,
    pub positions: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub normals: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
}

/// 异步网格生成管理器
#[derive(Resource)]
pub struct AsyncMeshManager {
    task_sender: mpsc::Sender<MeshTask>,
    result_receiver: mpsc::Receiver<MeshResult>,
    pending_tasks: HashSet<ChunkCoord>,
}

impl AsyncMeshManager {
    pub fn new(worker_count: usize) -> Self {
        let (task_tx, task_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        
        // 启动工作线程
        for _ in 0..worker_count {
            let rx = task_rx.clone();
            let tx = result_tx.clone();
            thread::spawn(move || {
                Self::worker_loop(rx, tx);
            });
        }
        
        Self {
            task_sender: task_tx,
            result_receiver: result_rx,
            pending_tasks: HashSet::new(),
        }
    }
    
    fn worker_loop(
        receiver: mpsc::Receiver<MeshTask>,
        sender: mpsc::Sender<MeshResult>,
    ) {
        while let Ok(task) = receiver.recv() {
            match task {
                MeshTask::Generate { coord, data, neighbors } => {
                    let (positions, uvs, normals, indices) = 
                        generate_chunk_mesh(&data, &neighbors);
                    
                    sender.send(MeshResult {
                        coord,
                        positions,
                        uvs,
                        normals,
                        indices,
                    }).ok();
                }
                MeshTask::Cancel(_) => { /* 跳过 */ }
            }
        }
    }
    
    pub fn submit_task(&mut self, task: MeshTask) {
        if let MeshTask::Generate { coord, .. } = &task {
            self.pending_tasks.insert(*coord);
        }
        self.task_sender.send(task).ok();
    }
    
    pub fn collect_results(&mut self) -> Vec<MeshResult> {
        let mut results = Vec::new();
        while let Ok(result) = self.result_receiver.try_recv() {
            self.pending_tasks.remove(&result.coord);
            results.push(result);
        }
        results
    }
}
```

#### 3.1.4 集成到现有系统

修改 [`chunk_loader_system()`](../src/chunk_manager.rs:168)：

```rust
pub fn chunk_loader_system(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut loaded: ResMut<LoadedChunks>,
    mut async_mesh: ResMut<AsyncMeshManager>,
    // ... 其他参数
) {
    // 1. 收集异步结果
    let results = async_mesh.collect_results();
    for result in results {
        // 创建 Mesh 并上传到 GPU
        let mesh_handle = meshes.add(/* ... */);
        // 更新实体组件
    }
    
    // 2. 提交新任务（不阻塞主线程）
    let drain_count = CHUNKS_PER_FRAME.min(loaded.load_queue.len());
    for coord in loaded.load_queue.drain(..drain_count) {
        let chunk = Chunk::filled(0);
        fill_terrain(&mut chunk, &coord);
        let neighbors = collect_neighbors(coord, &loaded);
        
        async_mesh.submit_task(MeshTask::Generate {
            coord,
            data: chunk,
            neighbors,
        });
    }
}
```

#### 3.1.5 预期收益

| 指标 | 当前 | 优化后 |
|------|------|--------|
| 加载尖峰 | ~21.8ms | <8ms |
| FPS 谷值 | ~58 | >100 |
| 主线程阻塞 | 是 | 否 |

---

### 3.2 ~~Phase 1~~：Greedy Meshing + 网格优化（备选方案）

> 来源：[GPU噪声迁移方案.md](GPU噪声迁移方案.md) §10.2

#### 3.2.1 当前网格生成瓶颈

| 瓶颈 | 当前实现 | 分析 |
|------|---------|------|
| **三重嵌套循环** | `for z → for y → for x` 遍历 32³=32768 个体素 | 每次迭代都调用 `chunk.get()` 和 `is_face_visible()` |
| **面剔除检查** | 每个非空气方块检查 6 个面（最多 196608 次检查） | `is_face_visible()` 每次都要查邻居数据 |
| **Vec 动态扩容** | 使用 `Vec::new()` + `Vec::extend()` | 多次 reallocation，浪费 CPU 周期 |
| **无法合并相邻面** | 每个面生成 4 个独立顶点 | 相邻同材质方块的面可以被合并为更大的四边形 |

#### 3.2.2 Greedy Meshing 算法（备选）

**原理**：将相邻的同材质方块面合并为更大的四边形，大幅减少顶点数。

```rust
pub fn generate_greedy_mesh(chunk: &Chunk, ...) -> Mesh {
    for face_direction in [Top, Bottom, Right, Left, Front, Back] {
        // 1. 创建可见性掩码（32×32 的 bool 数组）
        let mut mask = [[false; CHUNK_SIZE]; CHUNK_SIZE];
        for ... { mask[y][x] = is_face_visible(...); }

        // 2. 贪心合并：扫描掩码，合并连续的行
        let mut quads = Vec::new();
        for y in 0..CHUNK_SIZE {
            let mut x = 0;
            while x < CHUNK_SIZE {
                if !mask[y][x] { x += 1; continue; }
                // 找到最大连续宽度
                let w = find_width(&mask, y, x);
                let h = find_height(&mask, y, x, w);
                quads.push(Quad { x, y, w, h });
                // 标记已处理
                for dy in 0..h {
                    for dx in 0..w { mask[y+dy][x+dx] = false; }
                }
                x += w;
            }
        }
        // 3. 为每个合并后的四边形生成 4 个顶点
    }
}
```

#### 3.2.3 快速优化清单（低投入高回报）

| 优化项 | 修改位置 | 预期收益 | 工作量 |
|--------|---------|---------|-------|
| 预分配 Vec 容量 | `generate_chunk_mesh()` | ~10-20% 加速 | 极低 |
| 提前计算可见面数量 | `generate_chunk_mesh()` | 减少 reallocation | 低 |
| 内联 `is_face_visible()` | `chunk.rs` | ~5-10% 加速 | 低 |
| 全空气区块跳过网格生成 | `spawn_chunk_entity()` | 空气区块节省 100% | 极低 |

#### 3.2.4 预期收益

| 指标 | 当前（逐面生成） | Greedy Meshing |
|------|----------------|----------------|
| 顶点数/区块 | 平均 4000-8000 | 平均 500-1500 |
| CPU 耗时/区块 | ~0.5-1.5ms | ~0.1-0.3ms |
| GPU 渲染负载 | 高 | 低 |

---

### 3.3 Phase 2：LOD 系统（借鉴 Voxy 多级渲染）

#### 3.3.1 Voxy LOD 策略

Voxy 使用 4 级 LOD，每级降采样 2x：

| LOD 级别 | 距离 | 采样率 | 面数减少 |
|----------|------|--------|----------|
| LOD0 | 0-16 chunks | 1:1 | 基准 |
| LOD1 | 17-32 chunks | 1:2 | 75% |
| LOD2 | 33-64 chunks | 1:4 | 93.75% |
| LOD3 | 65-128 chunks | 1:8 | 98.4% |

#### 3.3.2 Spirit Realm LOD 设计

```rust
// src/lod.rs

/// LOD 级别定义
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LodLevel {
    Lod0 = 0,  // 全精度（1:1）
    Lod1 = 1,  // 1:2 降采样
    Lod2 = 2,  // 1:4 降采样
    Lod3 = 3,  // 1:8 降采样
}

impl LodLevel {
    /// 根据距离计算 LOD 级别
    pub fn from_distance(distance: f32) -> Self {
        match distance {
            d if d < 16.0 * 32.0 => LodLevel::Lod0,
            d if d < 32.0 * 32.0 => LodLevel::Lod1,
            d if d < 64.0 * 32.0 => LodLevel::Lod2,
            _ => LodLevel::Lod3,
        }
    }
    
    /// 降采样步长
    pub fn step(&self) -> usize {
        match self {
            LodLevel::Lod0 => 1,
            LodLevel::Lod1 => 2,
            LodLevel::Lod2 => 4,
            LodLevel::Lod3 => 8,
        }
    }
}
```

#### 3.3.3 LOD 切换策略（滞后防抖）

```rust
/// LOD 管理器
#[derive(Resource)]
pub struct LodManager {
    chunk_lods: HashMap<ChunkCoord, LodLevel>,
    hysteresis: f32,  // 滞后距离，避免边界抖动
}

impl LodManager {
    pub fn update(&mut self, player_pos: Vec3, loaded: &LoadedChunks) {
        for (coord, _) in &loaded.entries {
            let chunk_center = coord.to_world_origin() + Vec3::splat(16.0);
            let distance = player_pos.distance(chunk_center);
            
            let new_lod = LodLevel::from_distance(distance);
            let current_lod = self.chunk_lods.get(coord).copied()
                .unwrap_or(LodLevel::Lod0);
            
            // 滞后策略：只有距离变化超过阈值才切换
            if self.should_switch(current_lod, new_lod, distance) {
                self.chunk_lods.insert(*coord, new_lod);
                // 标记需要重建网格
            }
        }
    }
}
```

#### 3.3.4 预期收益

| 指标 | 当前 | LOD 后 |
|------|------|--------|
| 远景面数 | 100% | 20-30% |
| GPU 负载 | 基准 | 降低 50-70% |
| 内存占用 | 基准 | 降低 30-40% |

---

### 3.4 Phase 3：GPU 面剔除（借鉴 Voxy Compute Shader）

#### 3.4.1 Voxy GPU 方案

Voxy 使用 Compute Shader 进行面剔除，将 CPU 密集型计算转移到 GPU：

```wgsl
// WGSL Compute Shader 面剔除
@compute @workgroup_size(4, 4, 4)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let pos = id;
    let block = getBlock(pos.x, pos.y, pos.z);
    
    if (block == 0u) { return; }
    
    // 检查 6 个面
    for (var face = 0u; face < 6u; face++) {
        let neighbor_pos = vec3<i32>(pos) + FACE_OFFSETS[face];
        let neighbor = getBlockSafe(neighbor_pos);
        
        if (neighbor == 0u) {
            emitFace(pos, face, block);
        }
    }
}
```

#### 3.4.2 渐进式迁移策略

由于 Bevy 的 Compute Shader 支持仍在发展中，建议分阶段实施：

1. **阶段 A**：使用 `bevy::render::experimental::compute` API
2. **阶段 B**：如果 Bevy 支持不足，考虑使用 `wgpu` 直接调用
3. **阶段 C**：回退到 CPU 多线程方案（使用 Rayon）

#### 3.4.3 预期收益

| 指标 | CPU 面剔除 | GPU 面剔除 |
|------|-----------|-----------|
| 面剔除耗时 | ~2ms/chunk | <0.1ms/chunk |
| 主线程阻塞 | 是 | 否 |

---

### 3.5 Phase 4：GPU 噪声计算（整合自 GPU噪声迁移方案）

> 来源：[GPU噪声迁移方案.md](GPU噪声迁移方案.md)

#### 3.5.1 当前噪声性能分析

| 场景 | 采样量 | CPU 耗时估计 | 是否瓶颈 |
|------|--------|-------------|---------|
| 2D 高度图（当前） | 1024 次/区块 | ~0.01ms/区块 | ❌ 不是瓶颈 |
| 3D 噪声（未来洞穴） | 32768 次/区块 | ~0.3ms/区块 | ⚠️ 可能成为瓶颈 |

**结论**：当前 2D 噪声的 CPU 计算开销可以忽略不计，**不必急于 GPU 化**。

#### 3.5.2 建议时机

- **短期**：保持 CPU `noise` crate 不变，优先优化网格生成
- **中期**：当引入 3D 噪声（洞穴生成）时，启动 GPU 噪声方案
- **长期**：全 GPU 管线（噪声 → 方块填充 → 网格生成）

#### 3.5.3 推荐方案

方案 A：GPU 噪声 + CPU 方块填充（平衡复杂度和收益）

```
GPU 输出高度图 → CPU 根据高度决定方块类型 → 保留调色板压缩逻辑在 CPU
```

---

### 3.6 Phase 5：多级空间索引（借鉴 Voxy 层次化管理）

#### 3.6.1 三级空间索引设计

```
WorldColumn → MegaColumn → SubChunk
```

适应 Y=±20480 的超大垂直范围。

```rust
/// 世界列坐标（XZ 平面）
#[derive(Clone, Copy, Hash, Eq, PartialEq)]
pub struct WorldColumn {
    pub cx: i32,
    pub cz: i32,
}

/// 垂直大列（包含多个 SubChunk）
pub struct MegaColumn {
    pub y_min: i32,
    pub y_max: i32,
    pub chunks: HashMap<i32, SubChunkEntry>,
    pub terrain_loaded: bool,
}

/// 多级空间索引管理器
#[derive(Resource)]
pub struct SpatialIndex {
    columns: HashMap<WorldColumn, MegaColumn>,
    lru_queue: VecDeque<WorldColumn>,
    max_columns: usize,
}
```

#### 3.6.2 优势

1. **内存效率**：只加载玩家附近的列，每列只加载需要的 Y 范围
2. **快速查找**：O(1) 查找列，O(1) 查找 SubChunk
3. **批量操作**：可以按列批量加载/卸载
4. **LOD 友好**：可以按列生成 LOD 网格

---

### 3.7 Phase 6：内存池优化（借鉴 Voxy 缓冲区复用）

#### 3.7.1 Mesh 缓冲区池

```rust
#[derive(Resource)]
pub struct MeshBufferPool {
    positions: Vec<Vec<[f32; 3]>>,
    uvs: Vec<Vec<[f32; 2]>>,
    normals: Vec<Vec<[f32; 3]>>,
    indices: Vec<Vec<u32>>,
    max_pool_size: usize,
}

impl MeshBufferPool {
    pub fn acquire(&mut self) -> MeshBuffers {
        MeshBuffers {
            positions: self.positions.pop().unwrap_or_else(|| Vec::with_capacity(4096)),
            uvs: self.uvs.pop().unwrap_or_else(|| Vec::with_capacity(4096)),
            normals: self.normals.pop().unwrap_or_else(|| Vec::with_capacity(4096)),
            indices: self.indices.pop().unwrap_or_else(|| Vec::with_capacity(6144)),
        }
    }
    
    pub fn release(&mut self, mut buffers: MeshBuffers) {
        if self.positions.len() < self.max_pool_size {
            buffers.positions.clear();
            self.positions.push(buffers.positions);
        }
        // ... 其他缓冲区
    }
}
```

#### 3.7.2 预期收益

| 指标 | 当前 | 内存池后 |
|------|------|----------|
| 内存分配次数 | 每次重建 4 次 | 0 次（复用） |
| 内存碎片 | 高 | 低 |

---

## 四、统一实施路线图

### 4.1 优先级排序

```
Phase 0: 异步网格生成 (P0) ← 最高优先级，消除加载尖峰
    ↓
Phase 1: LOD 系统 (P1) ← 大幅降低远景 GPU 负载
    ↓
Phase 2: GPU 面剔除 (P1) ← 将 CPU 密集计算转移到 GPU（Voxy 核心）
    ↓
Phase 3: GPU 噪声计算 (P2) ← 等待 3D 噪声需求
    ↓
Phase 4: 多级空间索引 (P2) ← 优化内存和查找效率
    ↓
Phase 5: 内存池优化 (P2) ← 减少内存分配开销
    ↓
[备选] Greedy Meshing + 网格优化 ← Voxy 未使用，当 LOD + GPU 面剔除不足时启用
```

### 4.2 时间估算

| Phase | 任务 | 预估时间 | 依赖 |
|-------|------|----------|------|
| 0 | 异步网格生成 | 2-3 天 | 无 |
| 1 | LOD 系统 | 3-4 天 | Phase 0 |
| 2 | GPU 面剔除 | 5-7 天 | Phase 0 |
| 3 | GPU 噪声计算 | 3-5 天 | 等待需求 |
| 4 | 多级空间索引 | 2-3 天 | 无 |
| 5 | 内存池优化 | 1-2 天 | Phase 0 |
| 备选 | Greedy Meshing + 网格优化 | 3-4 天 | 无 |

### 4.3 里程碑

| 里程碑 | 目标 | 验收标准 |
|--------|------|----------|
| M0 | 消除加载尖峰 | 加载时 FPS > 100 |
| M1 | 网格优化 | 顶点数减少 70%+，FPS > 200 |
| M2 | 支持 64 区块视距 | 64 区块视距下 FPS > 60 |
| M3 | 支持 128 区块视距 | 128 区块视距下 FPS > 60 |

---

## 五、与现有架构的兼容性

### 5.1 需要修改的模块

| 模块 | 修改内容 | 影响范围 |
|------|----------|----------|
| [`chunk_manager.rs`](../src/chunk_manager.rs) | 集成异步网格生成 | 高 |
| [`chunk.rs`](../src/chunk.rs) | 添加 LOD 网格生成（Greedy Meshing 备选） | 中 |
| [`chunk_dirty.rs`](../src/chunk_dirty.rs) | 支持异步重建 | 中 |
| [`main.rs`](../src/main.rs) | 注册新系统和资源 | 低 |

### 5.2 新增模块

| 模块 | 功能 |
|------|------|
| `src/async_mesh.rs` | 异步网格生成管理 |
| `src/lod.rs` | LOD 系统 |
| `src/spatial_index.rs` | 多级空间索引 |
| `src/buffer_pool.rs` | 内存池 |
| `src/gpu_culling.rs` | GPU 面剔除（可选） |
| `src/gpu_noise.rs` | GPU 噪声计算（可选） |

---

## 六、风险评估

### 6.1 技术风险

| 风险 | 概率 | 影响 | 缓解措施 |
|------|------|------|----------|
| Bevy Compute Shader 支持不足 | 中 | 高 | 回退到 CPU 多线程 |
| 异步网格生成导致数据竞争 | 低 | 高 | 使用通道（Channel）同步 |
| LOD 切换产生视觉伪影 | 中 | 中 | 使用滞后策略和过渡动画 |
| GPU 内存不足 | 低 | 高 | 动态调整 LOD 距离 |

### 6.2 性能风险

| 风险 | 概率 | 影响 | 缓解措施 |
|------|------|------|----------|
| 异步任务堆积 | 中 | 中 | 限制队列大小，丢弃旧任务 |
| LOD 网格质量下降 | 低 | 中 | 调整降采样算法 |

---

## 七、总结

### 7.1 核心优化点

1. **异步网格生成**：消除加载尖峰，提升用户体验
2. **LOD 系统**：大幅降低远景 GPU 负载
3. **GPU 面剔除**：将 CPU 密集型计算转移到 GPU（Voxy 核心）
4. **GPU 噪声**：等待 3D 噪声需求时启动
5. **多级空间索引**：优化内存使用和查找效率
6. **内存池**：减少内存分配开销
7. **[备选] Greedy Meshing**：大幅减少顶点数，Voxy 未使用，保留为后备方案

### 7.2 预期最终效果

| 指标 | 当前 | 优化后 |
|------|------|--------|
| 视距 | 8 chunks | 128+ chunks |
| FPS（稳态） | ~170 | >60 |
| FPS（加载时） | ~58 | >100 |
| Draw Call | ~2,200+ | <100（LOD + GPU 面剔除自然减少） |
| 内存占用 | ~200MB | ~500MB（更大视距） |
| 加载尖峰 | ~21.8ms | <8ms |

### 7.3 下一步行动

1. **立即开始**：Phase 0（异步网格生成）已完成，下一步 Phase 1（LOD 系统）
2. **并行探索**：Phase 4（多级空间索引）
3. **持续评估**：Phase 2（GPU 面剔除）的 Bevy 支持情况

---

## 八、方案索引

| 方案文档 | 状态 | 说明 |
|----------|------|------|
| [GPU噪声迁移方案.md](GPU噪声迁移方案.md) | 已整合 | GPU 噪声 + 网格优化分析 |
| [优化建议总结.md](优化建议总结.md) | 已整合 | 性能基线与优化建议 |
| [Phase1-SuperChunk合批方案.md](Phase1-SuperChunk合批方案.md) | **已废弃** | 结构性缺陷，已被 LOD 替代 |
| [架构总纲.md](../docs/架构总纲.md) | 参考 | 项目整体架构设计 |

---

> **参考资源**
> - [Voxy 源码](https://github.com/MCRcortex/voxy)
> - [Greedy Meshing 算法](https://0fps.net/2012/06/30/meshing-in-a-minecraft-game/)
> - [Bevy Compute Shader 文档](https://bevyengine.org/examples/shaders/compute-shader/)
> - [wgpu 文档](https://docs.rs/wgpu/)
