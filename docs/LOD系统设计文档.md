# LOD 系统设计文档

> **Phase 1 优化**：借鉴 Voxy 多级渲染架构，在现有异步网格生成管线之上构建 LOD 系统。
>
> **目标**：在不增加 GPU 负载的前提下，将视觉渲染距离从当前 8 区块提升至 32+ 区块，
> 远景区块面数减少 70-93%，为后续扩展至 128 区块视距奠定基础。

---

## 1. 现状分析

### 1.1 当前渲染参数

| 参数 | 值 | 说明 |
|------|-----|------|
| [`RENDER_DISTANCE`](../src/chunk_manager.rs:28) | 8 区块 | 半径 8 区块 ≈ 256 米 |
| [`UNLOAD_DISTANCE`](../src/chunk_manager.rs:30) | 9 区块 | 比 RENDER_DISTANCE 大 1 |
| [`CHUNK_SIZE`](../src/chunk.rs:25) | 32³ 体素 | 每区块 32768 体素 |
| [`CHUNKS_PER_FRAME`](../src/chunk_manager.rs:32) | 4 区块/帧 | 每帧最多提交 4 个异步任务 |
| [`Y_LOAD_RADIUS`](../src/chunk_manager.rs:78) | ±4 层 | Y 轴只加载 ±4 层 ≈ ±128 米 |
| [`MAX_CACHED_CHUNKS`](../src/chunk_manager.rs:35) | 2000 | 缓存上限 |
| [`MESH_UPLOADS_PER_FRAME`](../src/async_mesh.rs:36) | 4 网格/帧 | 每帧 GPU 上传上限 |

### 1.2 性能瓶颈

- **Draw Call 数**：~2,200+（当前 8 区块视距下已接近 Bevy 渲染瓶颈）
- **GPU 顶点吞吐**：每个 32³ 区块平均 4000-8000 顶点，8 区块视距内约 2,000 区块产生 ~10M 顶点
- **帧时间**：稳态 ~5.9ms，加载尖峰已通过 Phase 0 消除至 <8ms

### 1.3 问题：为什么直接增加 RENDER_DISTANCE 不可行？

如果将 [`RENDER_DISTANCE`](../src/chunk_manager.rs:28) 从 8 提升至 32：

| 指标 | 8 区块 | 32 区块 | 倍率 |
|------|--------|---------|------|
| 区块总数 | ~2,000 | ~32,000 | 16x |
| 顶点数 | ~10M | ~160M | 16x |
| Draw Call | ~2,200 | ~32,000 | 16x |
| GPU 上传 | 4/帧 | 4/帧（排队 8000 帧） | — |

**结论**：无 LOD 时直接增加视距不可行，需要降采样方案。

---

## 2. LOD 级别定义

### 2.1 四级 LOD 设计

依据 [`docs/Voxy借鉴优化方案.md`](../docs/Voxy借鉴优化方案.md:319) 和 [`docs/架构总纲.md`](../docs/架构总纲.md:145)：

| LOD 级别 | 降采样率 | 采样步长 | 体素数/区块 | 预估面数/区块 | 渲染距离 |
|----------|---------|---------|------------|-------------|---------|
| LOD0 | 1:1 | 1 体素 | 32³ = 32,768 | ~4,000-8,000 | 0-8 区块 |
| LOD1 | 1:2 | 2 体素 | 16³ = 4,096 | ~500-1,000 | 9-16 区块 |
| LOD2 | 1:4 | 4 体素 | 8³ = 512 | ~60-125 | 17-24 区块 |
| LOD3 | 1:8 | 8 体素 | 4³ = 64 | ~8-16 | 25-32 区块 |

### 2.2 面数缩减率

| LOD 级别 | 体素数 | 面数估算 | 相比 LOD0 缩减 |
|----------|--------|---------|---------------|
| LOD0 | 32,768 | 6,000（基准） | 0% |
| LOD1 | 4,096 | 750 | 87.5% |
| LOD2 | 512 | 93 | 98.4% |
| LOD3 | 64 | 12 | 99.8% |

### 2.3 完整 32 区块视距下的数据估算

| 距离环 | 区块数量 | LOD 级别 | 单个面数 | 总面数 |
|--------|---------|---------|---------|--------|
| 0-8 | ~2,000 | LOD0 | 6,000 | 12,000,000 |
| 9-16 | ~6,000 | LOD1 | 750 | 4,500,000 |
| 17-24 | ~10,000 | LOD2 | 93 | 930,000 |
| 25-32 | ~14,000 | LOD3 | 12 | 168,000 |
| **总计** | **~32,000** | — | — | **~17.6M** |

> **对比**：无 LOD 时 32 区块视距的面数为 ~160M，有 LOD 后降至 ~17.6M，**缩减约 89%**。

---

## 3. 核心数据结构设计

### 3.1 [`LodLevel`](../src/lod.rs) 枚举

```rust
/// LOD 级别定义
///
/// 每级降采样 2x，四级 LOD 覆盖 32 区块视距：
/// LOD0 = 32³ 体素（全精度）
/// LOD1 = 16³ 体素（降采样 2x）
/// LOD2 = 8³ 体素（降采样 4x）
/// LOD3 = 4³ 体素（降采样 8x）
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LodLevel {
    Lod0 = 0,  // 1:1, 0-8 chunks
    Lod1 = 1,  // 1:2, 9-16 chunks
    Lod2 = 2,  // 1:4, 17-24 chunks
    Lod3 = 3,  // 1:8, 25-32 chunks
}

impl LodLevel {
    /// 最大 LOD 级别（用于边界检查）
    pub const MAX: usize = 3;

    /// 降采样步长（体素数）
    pub const fn step(self) -> usize {
        match self {
            LodLevel::Lod0 => 1,
            LodLevel::Lod1 => 2,
            LodLevel::Lod2 => 4,
            LodLevel::Lod3 => 8,
        }
    }

    /// 降采样后的区块体素数（单轴）
    pub const fn sampling_size(self) -> usize {
        match self {
            LodLevel::Lod0 => 32,
            LodLevel::Lod1 => 16,
            LodLevel::Lod2 => 8,
            LodLevel::Lod3 => 4,
        }
    }

    /// 根据与玩家的距离（区块为单位）计算 LOD 级别
    pub fn from_chunk_distance(dist_chunks: f32) -> Self {
        match dist_chunks {
            d if d < 9.0 => LodLevel::Lod0,
            d if d < 17.0 => LodLevel::Lod1,
            d if d < 25.0 => LodLevel::Lod2,
            _ => LodLevel::Lod3,
        }
    }
}
```

### 3.2 [`LodManager`](../src/lod.rs) Resource

```rust
/// LOD 管理器
///
/// 管理每个已加载区块的当前 LOD 级别，处理 LOD 切换决策。
/// 使用滞后策略避免玩家在小范围内移动时 LOD 频繁切换导致的视觉闪烁。
#[derive(Resource)]
pub struct LodManager {
    /// 每个区块当前的 LOD 级别
    chunk_lods: HashMap<ChunkCoord, LodLevel>,
    /// 滞后距离（区块为单位）：切换 LOD 需要额外多走一半距离
    hysteresis: f32,
}

impl LodManager {
    /// 创建 LOD 管理器
    pub fn new() -> Self {
        Self {
            chunk_lods: HashMap::new(),
            hysteresis: 0.5, // 半区块的滞后
        }
    }

    /// 更新所有已加载区块的 LOD 级别
    ///
    /// 返回需要重建网格的区块列表（LOD 级别发生变化的区块）。
    pub fn update(
        &mut self,
        player_chunk: ChunkCoord,
        loaded: &LoadedChunks,
    ) -> Vec<(ChunkCoord, LodLevel)> {
        let mut to_rebuild = Vec::new();

        for (coord, _) in &loaded.entries {
            let dist = self.chunk_distance(*coord, player_chunk);
            let new_lod = LodLevel::from_chunk_distance(dist);

            let current_lod = self.chunk_lods.get(coord)
                .copied()
                .unwrap_or(LodLevel::Lod0);

            // 只有 LOD 级别发生变化时才触发重建
            if new_lod != current_lod {
                // 滞后检查：距离变化超过阈值才切换
                if self.should_switch(current_lod, new_lod, dist) {
                    self.chunk_lods.insert(*coord, new_lod);
                    to_rebuild.push((*coord, new_lod));
                }
            }
        }

        to_rebuild
    }

    /// 获取区块的当前 LOD 级别
    pub fn get_lod(&self, coord: &ChunkCoord) -> LodLevel {
        self.chunk_lods.get(coord).copied().unwrap_or(LodLevel::Lod0)
    }

    /// 计算两个 ChunkCoord 之间的区块距离
    fn chunk_distance(&self, a: ChunkCoord, b: ChunkCoord) -> f32 {
        let dx = (a.cx - b.cx) as f32;
        let dy = (a.cy - b.cy) as f32;
        let dz = (a.cz - b.cz) as f32;
        (dx * dx + dy * dy + dz * dz).sqrt()
    }

    /// 滞后切换决策
    ///
    /// 从低 LOD 切到高 LOD（降级 LOD，玩家靠近）：立即切换，无滞后
    /// 从高 LOD 切到低 LOD（升级 LOD，玩家远离）：需要超过滞后阈值
    fn should_switch(&self, current: LodLevel, new: LodLevel, dist: f32) -> bool {
        if (new as i32) < (current as i32) {
            // 玩家靠近：降级 LOD（从粗糙到精细），立即切换
            true
        } else {
            // 玩家远离：升级 LOD（从精细到粗糙），使用滞后
            // 需要额外走 hysteresis * (距离环宽度) 才切换
            let ring_width = match current {
                LodLevel::Lod0 => 8.0,   // 0-8 环
                LodLevel::Lod1 => 8.0,   // 9-16 环
                LodLevel::Lod2 => 8.0,   // 17-24 环
                LodLevel::Lod3 => 8.0,   // 25-32 环
            };
            dist > current.threshold() + self.hysteresis * ring_width
        }
    }
}

impl LodLevel {
    /// 获取该 LOD 级别的距离阈值上限
    fn threshold(self) -> f32 {
        match self {
            LodLevel::Lod0 => 8.0,
            LodLevel::Lod1 => 16.0,
            LodLevel::Lod2 => 24.0,
            LodLevel::Lod3 => 32.0,
        }
    }
}
```

### 3.3 LOD 感知的 [`MeshTask`](../src/async_mesh.rs:130) 扩展

```rust
/// 发送到工作线程的网格生成任务
pub enum MeshTask {
    /// 生成指定区块的网格
    Generate {
        coord: ChunkCoord,
        data: ChunkData,
        neighbors: ChunkNeighbors,
        /// LOD 级别：None = LOD0（全精度）
        lod_level: Option<LodLevel>,
    },
    /// 取消指定区块的网格生成
    Cancel(ChunkCoord),
}
```

### 3.4 [`ChunkEntry`](../src/chunk_manager.rs:43) 扩展

```rust
pub struct ChunkEntry {
    pub entity: Entity,
    pub data: Chunk,
    pub last_accessed: u64,
    pub mesh_handle: Handle<Mesh>,
    pub material_handle: Handle<VoxelMaterial>,
    /// 当前 LOD 级别
    pub lod_level: LodLevel,
}
```

---

## 4. LOD 降采样网格生成算法

### 4.1 核心思想

LOD 降采样的本质是：**对 32³ 体素空间每隔 N 个体素采样一次**（N = 降采样步长），然后对采样结果执行标准的逐面可见性检查 + 四边形生成。

```
LOD0 (1:1, step=1): 遍历全部 32³ = 32768 体素
LOD1 (1:2, step=2): 遍历 16³ = 4096 体素（每 2x2x2 取 1 个）
LOD2 (1:4, step=4): 遍历 8³ = 512 体素
LOD3 (1:8, step=8): 遍历 4³ = 64 体素
```

### 4.2 采样规则

1. **体素位置映射**：采样点 `(sx, sy, sz)` 对应原区块位置 `(sx*step, sy*step, sz*step)`
2. **法线偏移缩放**：面剔除时的法线偏移量也需要乘以 `step`
3. **邻居边界检查**：跨区块的面可见性检查需要将邻居坐标映射到原区块的对应采样位置
4. **UV 保持不变**：降采样后的四边形依然使用原始纹理映射

### 4.3 函数签名

```rust
/// LOD 降采样网格生成函数
///
/// 根据指定的 LOD 级别对区块体素进行降采样，生成精简网格。
/// 与 `generate_chunk_mesh_async` 共享 UV 查找表和邻居数据结构。
///
/// # 参数
/// - `chunk`: 原始 32³ 区块数据
/// - `uv_table`: UV 查找表
/// - `neighbors`: 6 方向邻居数据（原始 32³ 分辨率）
/// - `lod`: LOD 级别（决定了降采样步长）
///
/// # 返回
/// 降采样后的顶点数据
fn generate_lod_mesh(
    chunk: &ChunkData,
    uv_table: &UvLookupTable,
    neighbors: &ChunkNeighbors,
    lod: LodLevel,
) -> (Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<[f32; 3]>, Vec<u32>) {
    let step = lod.step();
    let sample_size = lod.sampling_size(); // 16, 8, or 4

    // 预分配容量（按面数估算）
    let capacity = (sample_size * sample_size * sample_size * 4) / 3;
    let mut positions = Vec::with_capacity(capacity);
    let mut uvs = Vec::with_capacity(capacity);
    let mut normals = Vec::with_capacity(capacity);
    let mut indices = Vec::with_capacity(capacity + capacity / 2);

    for sz in 0..sample_size {
        for sy in 0..sample_size {
            for sx in 0..sample_size {
                // 映射到原区块坐标
                let x = sx * step;
                let y = sy * step;
                let z = sz * step;

                let block_id = chunk.get(x, y, z);
                if block_id == 0 {
                    continue;
                }

                // 检查 6 个面（使用 LOD 版本的面可见性检查）
                for (face_idx, (face, offset, uv_idx)) in FACES_ASYNC.iter().cloned().enumerate() {
                    // 法线偏移量乘以 step
                    let lod_offset = [offset[0] * step as i32,
                                     offset[1] * step as i32,
                                     offset[2] * step as i32];

                    if !is_face_visible_lod(chunk, x, y, z, &lod_offset, face_idx, neighbors, step) {
                        continue;
                    }

                    let base_index = positions.len() as u32;
                    let uv = uv_table.get_uv(block_id, uv_idx);
                    let (face_verts, face_uvs, face_normal) = face_quad_async(x, y, z, face, uv);
                    // ... 追加顶点数据（与现有逻辑相同）
                }
            }
        }
    }

    (positions, uvs, normals, indices)
}

/// LOD 版本的面可见性检查
///
/// 与标准版本的区别：
/// 1. 法线偏移已乘以 step
/// 2. 邻居查询时需要将邻居坐标映射到原区块的采样位置
fn is_face_visible_lod(
    chunk: &ChunkData,
    x: usize,
    y: usize,
    z: usize,
    lod_offset: &[i32; 3],  // 已乘以 step 的法线偏移
    face_index: usize,
    neighbors: &ChunkNeighbors,
    step: usize,
) -> bool {
    let nx = x as i32 + lod_offset[0];
    let ny = y as i32 + lod_offset[1];
    let nz = z as i32 + lod_offset[2];

    let current_id = chunk.get(x, y, z);

    if nx >= 0 && ny >= 0 && nz >= 0
        && nx < CHUNK_SIZE as i32
        && ny < CHUNK_SIZE as i32
        && nz < CHUNK_SIZE as i32
    {
        return chunk.get(nx as usize, ny as usize, nz as usize) != current_id;
    }

    // 邻居边界外：邻居数据是原始 32³ 分辨率
    // 需要将邻居坐标映射到邻居区块的对应位置
    let neighbor_x = nx.rem_euclid(CHUNK_SIZE as i32) as usize;
    let neighbor_y = ny.rem_euclid(CHUNK_SIZE as i32) as usize;
    let neighbor_z = nz.rem_euclid(CHUNK_SIZE as i32) as usize;

    let neighbor_id = neighbors.get_neighbor_block(face_index, neighbor_x, neighbor_y, neighbor_z);
    neighbor_id != current_id
}
```

### 4.4 工作线程适配

在 [`AsyncMeshManager::worker_loop`](../src/async_mesh.rs:217) 中，根据 [`MeshTask`] 中携带的 `lod_level` 参数选择调用标准函数还是降采样函数：

```rust
// 在 worker_loop 中：
match task {
    MeshTask::Generate { coord, data, neighbors, lod_level } => {
        let (positions, uvs, normals, indices) = match lod_level {
            Some(LodLevel::Lod0) | None => {
                // LOD0 或未指定：使用标准网格生成
                generate_chunk_mesh_async(&data, &uv_table, &neighbors)
            }
            Some(lod) => {
                // LOD1-3：使用降采样网格生成
                generate_lod_mesh(&data, &uv_table, &neighbors, lod)
            }
        };
        // ... 发送结果
    }
}
```

---

## 5. LOD 与现有系统的集成

### 5.1 集成架构总览

```
┌─────────────────────────────────────────────────────────────────┐
│                    chunk_loader_system (First)                    │
│                                                                   │
│  1. 收集异步结果 ←─────────── 包括 LOD 和非 LOD 网格              │
│  2. 更新 LOD 级别 ←─ LodManager.update() 检测距离变化             │
│  3. 重建加载队列 ←─── LOD 感知的距离排序                          │
│  4. 提交新任务 ←─────── 携带 LOD 级别的 MeshTask                  │
│  5. LRU 淘汰 + 卸装 ─── 优先卸载高 LOD 区块                       │
└─────────────────────────────────────────────────────────────────┘
                          │
                          ▼
┌─────────────────────────────────────────────────────────────────┐
│                  AsyncMeshManager (Worker Threads)                │
│                                                                   │
│  MeshTask::Generate { coord, data, neighbors, lod_level: Lod }    │
│                    │                                              │
│         ┌──────────┴──────────┐                                   │
│         ▼                     ▼                                   │
│  generate_chunk_mesh_async   generate_lod_mesh                    │
│  (LOD0: 32³ 全精度)          (LOD1-3: 降采样)                     │
└─────────────────────────────────────────────────────────────────┘
```

### 5.2 修改 [`chunk_loader_system`](../src/chunk_manager.rs:191)

#### 5.2.1 步骤 1：收集异步结果后附加 LOD 信息

当前结果收集阶段已经按 `ChunkCoord` 匹配实体。需要确保上传的 mesh 能反映其 LOD 级别。**不需要修改结果收集逻辑**，因为 [`MeshResult`](../src/async_mesh.rs:146) 的 `coord` 字段足以匹配。

#### 5.2.2 步骤 2：更新 LOD（玩家移动时）

在玩家移动到新区块后，调用 [`LodManager::update()`] 获取需要重建的区块列表：

```rust
// 在 chunk_loader_system 中，玩家移动时：
let to_rebuild = lod_manager.update(player_chunk, &loaded);
for (coord, new_lod) in to_rebuild {
    // 如果区块已加载但需要切换 LOD，标记为脏
    if let Some(entry) = loaded.entries.get(&coord) {
        commands.entity(entry.entity).insert(DirtyChunk);
        // 更新 ChunkEntry 中的 lod_level
        if let Some(entry) = loaded.entries.get_mut(&coord) {
            entry.lod_level = new_lod;
        }
    }
}
```

#### 5.2.3 步骤 3：LOD 感知的加载队列重建

```rust
fn rebuild_load_queue(center: ChunkCoord, loaded: &mut LoadedChunks) {
    // RENDER_DISTANCE 提升到 32
    const LOD_RENDER_DISTANCE: i32 = 32;

    // 环形加载：内圈优先
    for ring in 0..=LOD_RENDER_DISTANCE {
        for dx in -ring..=ring {
            for dz in -ring..=ring {
                if dx.abs() != ring && dz.abs() != ring {
                    continue; // 只处理当前环的边界
                }
                // 圆形裁剪
                if dx * dx + dz * dz > ring * ring { continue; }

                for cy in (center.cy - Y_LOAD_RADIUS)..=(center.cy + Y_LOAD_RADIUS) {
                    let coord = ChunkCoord { cx: center.cx + dx, cy, cz: center.cz + dz };
                    if !loaded.entries.contains_key(&coord) {
                        missing.push(coord);
                    }
                }
            }
        }
    }
    // 按距离排序（已经按环形遍历，天然有序）
    loaded.load_queue = missing;
}
```

#### 5.2.4 步骤 4：提交 LOD 感知的异步任务

```rust
// 在提交任务时，根据距离计算 LOD 级别：
for coord in chunks_to_load {
    let dist = /* 计算区块距离 */;
    let lod = LodLevel::from_chunk_distance(dist);

    let async_mesh.submit_task(MeshTask::Generate {
        coord,
        data: chunk,
        neighbors,
        lod_level: Some(lod),  // 携带 LOD 级别
    });

    // 在 ChunkEntry 中记录 LOD 级别
    entry.lod_level = lod;
}
```

### 5.3 修改 [`rebuild_dirty_chunks`](../src/chunk_dirty.rs:87)

脏块重建时也需要携带 LOD 级别到异步任务中：

```rust
pub fn rebuild_dirty_chunks(
    // ... 现有参数
    lod_manager: Res<LodManager>,  // 新增
) {
    for (entity, chunk_data, coord_comp, mesh_handle) in &dirty_chunks {
        let coord = coord_comp.0;

        // 获取该区块的当前 LOD 级别
        let lod = lod_manager.get_lod(&coord);

        // 提交异步任务时携带 LOD
        let submitted = async_mesh.submit_task(MeshTask::Generate {
            coord,
            data: chunk_data.clone(),
            neighbors,
            lod_level: Some(lod),
        });
        // ...
    }
}
```

### 5.4 修改 [`unload_distant_chunks`](../src/chunk_manager.rs:437) 和 [`lru_evict`](../src/chunk_manager.rs:479)

卸载区块时同步清理 [`LodManager`] 中的记录：

```rust
// 在卸载区块时：
lod_manager.chunk_lods.remove(&coord);
```

### 5.5 修改 [`main.rs`](../src/main.rs:24)

注册新资源和系统：

```rust
fn main() {
    App::new()
        // ... 现有资源
        .init_resource::<LodManager>()  // 新增
        // ... 现有系统
        .add_systems(First, (
            chunk_manager::chunk_loader_system,
            // lod_manager 的更新内嵌在 chunk_loader_system 中
        ))
        // ...
}
```

---

## 6. LOD 切换的滞后策略

### 6.1 问题：视觉闪烁

当玩家在 LOD 距离阈值边界来回移动时，LOD 频繁切换会导致区块网格反复重建，表现为视觉闪烁和帧率抖动。

### 6.2 滞后策略设计

```
玩家移动方向 → (靠近)
    LOD3 ──25── LOD2 ──17── LOD1 ──9── LOD0
    ↑                  ↑
    玩家远离时需要    玩家靠近时
    多走4区块才切换   立即切换
```

**规则**：
- **靠近（降级 LOD）**：立即从粗糙切换到精细，保证视觉质量
- **远离（升级 LOD）**：需要额外走过半个距离环宽度才切换，避免小幅后退导致频繁升级

### 6.3 滞后阈值计算公式

```
升级触发距离 = 当前 LOD 阈值 + hysteresis × 环宽度
其中：
- hysteresis = 0.5（可配置）
- 环宽度 = 8 区块（所有级别一致）
```

示例：玩家在 LOD1 区域（9-16 区块）向远处移动：
- LOD1 → LOD2 的触发距离 = 16 + 0.5 × 8 = **20 区块**
- 即玩家需要走到 20 区块距离时才从 LOD1 升级到 LOD2

### 6.4 视觉平滑（可选增强）

在两个 LOD 级别之间的过渡区域，可以考虑使用**距离雾效掩盖**：
- 在 Bevy 的 `FogSettings` 中配置距离雾
- 使远景区块逐渐融入雾中，降低 LOD 切换的视觉突兀感

```rust
// 在 setup_world 中添加距离雾
commands.insert_resource(FogSettings {
    color: Color::srgb(0.53, 0.81, 0.92),
    directional_light_color: Color::srgb(1.0, 1.0, 1.0),
    directional_light_exponent: 1.0,
    mode: FogMode::Linear {
        start: 200.0,  // 约 6 区块
        end: 1000.0,   // 约 31 区块
    },
});
```

---

## 7. 渲染距离扩展方案

### 7.1 渐进式扩距

| 阶段 | 渲染距离 | 区块总数 | 面数估算 | 说明 |
|------|---------|---------|---------|------|
| 当前 | 8 区块 | ~2,000 | ~10M | 无 LOD |
| 阶段 A | 16 区块 | ~8,000 | ~16.5M | LOD0-1 |
| 阶段 B | 24 区块 | ~18,000 | ~17.5M | LOD0-2 |
| 阶段 C | 32 区块 | ~32,000 | ~17.6M | LOD0-3 |

> **关键观察**：加入 LOD3 后，即使区块数从 18,000 增加到 32,000（+77%），总面数仅增长 0.6%。这是因为最外层 14,000 个区块都是 LOD3（每区块仅 ~12 个面）。

### 7.2 常量调整方案

```rust
// src/chunk_manager.rs

// 将 RENDER_DISTANCE 从 8 提升到 16（阶段 A）
pub const RENDER_DISTANCE: i32 = 16;
// 后续可逐步调整到 24、32

// LOD 渲染距离常量
pub const LOD_RENDER_DISTANCES: [f32; 4] = [8.0, 16.0, 24.0, 32.0];
```

### 7.3 关键瓶颈：Draw Call 线性增长

LOD 降采样解决了**顶点数问题**，但 **Draw Call 数仍然随区块数线性增长**：

| 渲染距离 | 区块总数 | Draw Call | 瓶颈类型 |
|---------|---------|----------|---------|
| 8 | ~2,000 | ~2,200 | ✅ 无瓶颈 |
| 16 | ~8,000 | ~8,000 | ⚠️ 接近 Bevy 上限 |
| 24 | ~18,000 | ~18,000 | ❌ Draw Call 瓶颈 |
| 32 | ~32,000 | ~32,000 | ❌ 严重瓶颈 |

> **这就是为什么 32 区块是当前 LOD 阶段的自然上限**——即使顶点数足够低，Draw Call 本身就会压垮渲染管线。

---

## 8. 从 32 到 128+ 区块：完整扩展路线图

### 8.1 为什么需要三种技术的组合

| 技术 | 解决什么 | 效果 | 适用距离 |
|------|---------|------|---------|
| **LOD 降采样**（Phase 1） | 远景顶点数爆炸 | 面数减少 99.8% | 0-32 区块 |
| **GPU 面剔除 + MultiDraw**（Phase 2） | Draw Call 爆炸 | Draw Call 从 32,000 → ~100 | 0-128+ 区块 |
| **MegaLOD Tile**（Phase 3） | 区块实体数爆炸 | 实体数从 32,000 → ~500 | 33-128+ 区块 |

### 8.2 三层架构详解

```
视距 128+ 区块（约 4,096 米）
┌─────────────────────────────────────────────────────────────────┐
│ 内层：0-32 区块（~32,000 区块）                                  │
│ ├── LOD0: 0-8 区块    全精度 | 可交互 | 逐区块实体              │
│ ├── LOD1: 9-16 区块   1:2    | 不可交互 | 逐区块实体            │
│ ├── LOD2: 17-24 区块  1:4    | 不可交互 | 逐区块实体            │
│ └── LOD3: 25-32 区块  1:8    | 不可交互 | 逐区块实体            │
│                                                                  │
│ 中层：33-64 区块（~12,000 MegaLOD Tile 替代 ~98,000 区块）       │
│ └── MegaLOD Tile LOD1: 1:16 降采样，64×64×16 体素/瓦片           │
│     每个瓦片覆盖 2×2×1 区块（128×128×32 米）                      │
│                                                                  │
│ 外层：65-128+ 区块（~2,400 MegaLOD Tile）                        │
│ ├── MegaLOD Tile LOD2: 1:32 降采样                              │
│ └── MegaLOD Tile LOD3: 1:64 降采样                              │
└─────────────────────────────────────────────────────────────────┘
```

### 8.3 MegaLOD Tile 核心设计

MegaLOD Tile 与逐区块 LOD 有本质区别：

| 特性 | 逐区块 LOD（Phase 1） | MegaLOD Tile（Phase 3） |
|------|---------------------|------------------------|
| **单位** | 单个 Chunk（32³） | 瓦片（64×64×16 SubChunks） |
| **网格生成** | 实时、异步、自动 | 预生成、离线烘焙、可序列化 |
| **更新方式** | 脏标记 → 自动重建 | 版本号检测 → 后台重烘焙 |
| **内存策略** | 常驻内存 | 仅存 Mesh，不存体素数据 |
| **交互性** | 可放置/破坏方块 | 不可交互（只读远景） |
| **典型尺寸** | 2,000-32,000 实体 | 50-2,400 瓦片 |

**MegaLOD Tile 的数据流**：

```
区块数据（ChunkData）
    │
    ▼
LOD 降采样器（对 64×64×16 SubChunks 进行 3D 体素降采样）
    │
    ▼
网格生成器（标准面剔除 + 四边形生成）
    │
    ▼
Mesh 序列化（存储到磁盘缓存，下次加载时直接读取）
    │
    ▼
运行时加载（仅 Mesh + Material，ChunkData 已丢弃）
```

### 8.4 128+ 区块视距的完整数据估算

| 距离环 | 区块数 | 渲染方式 | 实体数 | 每实体面数 | 总面数 | Draw Call |
|--------|-------|---------|--------|-----------|-------|-----------|
| 0-8 | ~2,000 | 逐区块 LOD0 | 2,000 | 6,000 | 12M | 2,000 |
| 9-16 | ~6,000 | 逐区块 LOD1 | 6,000 | 750 | 4.5M | 6,000 |
| 17-24 | ~10,000 | 逐区块 LOD2 | 10,000 | 93 | 0.93M | 10,000 |
| 25-32 | ~14,000 | 逐区块 LOD3 | 14,000 | 12 | 0.17M | 14,000 |
| 33-64 | ~98,000 | MegaLOD Tile | ~1,200 | 8,000 | 9.6M | ~40 |
| 65-128 | ~258,000 | MegaLOD Tile | ~1,200 | 1,000 | 1.2M | ~40 |
| **总计** | **~388,000** | — | **~34,400** | — | **~28.4M** | **~32,080** |

> **关键数据**：
> - 如果没有 MegaLOD Tile，仅 33-64 环的 98,000 个逐区块实体的 Draw Call 就会远超 GPU 承受能力
> - MegaLOD Tile 将 98,000 个区块合并为 ~1,200 个瓦片，Draw Call 降至 ~40
> - 最终 32,080 个 Draw Call 通过 GPU MultiDrawIndirect（Phase 2）可以合并为 ~100 次调用

### 8.5 分阶段实施计划

```
Phase 1（当前）：逐区块 LOD 0-32 区块
├── LOD0-3 四级降采样
├── 渲染距离从 8 → 32
├── 滞后切换策略
└── Draw Call 瓶颈暴露（32,000）

     ↓

Phase 2：GPU 面剔除 + MultiDrawIndirect
├── Compute Shader 视锥剔除
├── 合并所有区块 Draw Call
├── Draw Call 从 32,000 → ~100
└── 为 MegaLOD Tile 铺平管线基础

     ↓

Phase 3：MegaLOD Tile 33-128+ 区块
├── 64×64×16 瓦片定义
├── 离线预生成 + 磁盘缓存
├── 版本号驱动的增量更新
├── 渲染距离从 32 → 128+
└── 距离雾掩盖 LOD 切换
```

### 8.6 当前 Phase 1 的局限性（为什么只到 32 区块）

理解 Phase 1 的局限性有助于避免在实施过程中走弯路：

1. **Draw Call 限制**：即使 LOD3 每区块仅 12 个面，32,000 个区块 = 32,000 个 Draw Call，已经超过 Bevy 单个渲染阶段的合理上限（~10,000）

2. **实体数限制**：32,000 个 Bevy Entity 本身就有 CPU 开销。每帧 query 遍历 32,000 个实体、更新 Transform 等操作会产生可观的帧时间

3. **ChunkData 内存限制**：每个区块即使使用调色板压缩，`ChunkData::Paletted` 也占用 ~4KB。32,000 × 4KB = 128MB 仅用于区块数据，加上 Mesh 缓冲区再翻倍

4. **MegaLOD Tile 的不可替代性**：对于 33-128+ 区块，保存 ChunkData 既不现实也没必要——这些区块不可交互，只需渲染外观。MegaLOD Tile 丢弃 ChunkData 只保留 Mesh，内存从 GB 级降到 MB 级

### 8.7 与架构总纲中 MegaLOD Tile 的对应关系

参见 [`docs/架构总纲.md`](../docs/架构总纲.md:154) 中远景静态区域的描述：

| 架构总纲定义 | 本设计对应 | 说明 |
|-------------|----------|------|
| 近景 0-16 区块 | Phase 1 LOD0-1 | 可交互区域 |
| 远景 17-128+ 区块 | Phase 1 LOD2-3 + Phase 3 MegaLOD | 不可交互静态区域 |
| MegaLOD Tile 64×64×16 | Phase 3 瓦片定义 | 每个瓦片覆盖 128×128×32 米 |
| 预生成 + 序列化缓存 | Phase 3 离线烘焙 | 运行时直接加载 Mesh |

---

## 9. 性能预期

### 8.1 量化估算

| 指标 | 当前（8 区块） | 阶段 A（16 区块+LOD） | 阶段 B（32 区块+LOD） |
|------|--------------|---------------------|---------------------|
| 区块总数 | ~2,000 | ~8,000 | ~32,000 |
| 总顶点数 | ~10M | ~16.5M | ~17.6M |
| Draw Call | ~2,200 | ~8,000 | ~32,000 |
| GPU 上传压力 | 4 网格/帧 | 4 网格/帧 | 4 网格/帧 |
| 远景视觉质量 | 突然消失 | 平滑过渡 | 远山轮廓 |

> **注意**：Draw Call 数随区块数线性增长。当 Draw Call 超过 Bevy 渲染管线的吞吐能力时（通常 ~10,000），需要引入 GPU 面剔除（Phase 2）来合并 Draw Call。

### 8.2 LOD 网格生成耗时

| LOD 级别 | 体素数 | 面剔除检查次数 | 预估耗时 | 与 LOD0 的比例 |
|----------|--------|--------------|---------|--------------|
| LOD0 | 32,768 | 196,608 | ~0.5ms | 100% |
| LOD1 | 4,096 | 24,576 | ~0.06ms | 12.5% |
| LOD2 | 512 | 3,072 | ~0.008ms | 1.6% |
| LOD3 | 64 | 384 | ~0.001ms | 0.2% |

LOD 降采样网格的生成成本极低，工作线程可以轻松处理大量 LOD 任务。

### 8.3 Draw Call 瓶颈缓解方案

当 Draw Call 成为瓶颈时（预计在 16-24 区块范围），有两个缓解选项：

1. **短期**：增加 [`CHUNKS_PER_FRAME`](../src/chunk_manager.rs:32) 和 [`MESH_UPLOADS_PER_FRAME`](../src/async_mesh.rs:36) 的值，加速区块加载
2. **中期**：实施 Phase 2（GPU 面剔除），用 Compute Shader 合并渲染

---

## 10. 修改文件清单

| 文件 | 修改类型 | 具体修改内容 |
|------|---------|-------------|
| [`src/lod.rs`] | **新增** | `LodLevel` 枚举、`LodManager` Resource、`generate_lod_mesh()` 函数、`is_face_visible_lod()` 函数 |
| [`src/async_mesh.rs`](../src/async_mesh.rs) | 修改 | [`MeshTask`] 添加 `lod_level: Option<LodLevel>` 字段；[`worker_loop`] 中根据 LOD 选择 `generate_chunk_mesh_async` 或 `generate_lod_mesh` |
| [`src/chunk_manager.rs`](../src/chunk_manager.rs) | 修改 | [`ChunkEntry`] 添加 `lod_level` 字段；[`chunk_loader_system`] 集成 `LodManager` 更新；[`rebuild_load_queue`] 支持扩展到 32 区块；[`unload_distant_chunks`] 和 [`lru_evict`] 清理 LOD 记录 |
| [`src/chunk_dirty.rs`](../src/chunk_dirty.rs) | 修改 | [`rebuild_dirty_chunks`] 获取当前 LOD 级别并传递给 [`MeshTask`] |
| [`src/main.rs`](../src/main.rs) | 修改 | 注册 `LodManager` 资源；可选添加距离雾 |

---

## 11. 实施路线图

```
步骤 1: 创建 src/lod.rs
├── LodLevel 枚举（含 step/sampling_size/from_chunk_distance）
├── LodManager Resource（含 update/get_lod/should_switch）
└── generate_lod_mesh() + is_face_visible_lod()

步骤 2: 修改 src/async_mesh.rs
├── MeshTask::Generate 添加 lod_level 字段
├── worker_loop 中根据 LOD 选择生成函数
└── MeshResult 保持不变

步骤 3: 修改 src/chunk_manager.rs
├── ChunkEntry 添加 lod_level 字段
├── chunk_loader_system 集成 LodManager
├── rebuild_load_queue 支持 32 区块
└── 卸载时清理 LodManager 记录

步骤 4: 修改 src/chunk_dirty.rs
└── rebuild_dirty_chunks 传递 LOD 级别

步骤 5: 修改 src/main.rs
├── 注册 LodManager
└── 可选：添加距离雾

步骤 6: 编译测试 + 调试
├── cargo check 验证编译
├── 运行测试，检查 LOD 切换视觉效果
└── 调整 LOD 阈值和滞后参数
```

---

## 12. 风险与缓解

| 风险 | 概率 | 影响 | 缓解措施 |
|------|------|------|----------|
| LOD 切换产生明显的视觉跳变 | 中 | 中 | 使用滞后策略 + 距离雾掩盖；调整降采样算法从简单采样改为均值采样 |
| Draw Call 数随区块数线性增长 | 高 | 高 | LOD 仅解决顶点数问题。Draw Call 瓶颈需要 Phase 2（GPU 面剔除）+ Phase 3（MegaLOD Tile）解决 |
| LOD 网格与标准网格拼接处裂缝 | 低 | 中 | 确保 LOD 网格的顶点位置对齐 32³ 体素网格（step 的整数倍位置） |
| 工作线程负载不均（LOD0 慢、LOD3 快） | 低 | 低 | 任务分发是随机的，统计上各线程负载均衡 |
| 玩家高速移动时 LOD 切换跟不上 | 中 | 低 | LodManager.update 每帧执行，切换延迟最多 1 帧 |
| 32 区块后 Draw Call 瓶颈阻碍继续扩距 | 高 | 高 | 这是设计预期内的边界，Phase 2 GPU 面剔除是必经之路 |

---

## 13. 与后续 Phase 的衔接

### 13.1 与 Phase 2（GPU 面剔除）的关系

LOD 系统解决的是 **"如何减少远景的几何复杂度"**，而 GPU 面剔除解决的是 **"如何减少 Draw Call 数量"**。两者互补：

- LOD 降低每个区块的顶点数 → 减少 GPU 顶点吞吐
- GPU 面剔除合并相邻区块的绘制 → 减少 Draw Call 数

**关键依赖**：Phase 1（LOD）完成后，32 区块视距下的 32,000 Draw Call 会直接暴露 GPU 提交瓶颈，这为 Phase 2 提供了明确的优化目标和测试数据。

### 13.2 与 Phase 3（MegaLOD Tile）的关系

Phase 1 的 LOD 系统是 Phase 3 MegaLOD Tile 的基础：

- LOD 降采样算法（`generate_lod_mesh`）可以直接复用于 MegaLOD Tile 的网格生成
- `LodManager` 管理框架可以扩展为 Tile LOD 管理器
- Phase 1 暴露的 Draw Call 瓶颈是 MegaLOD Tile 的必要性证明

### 13.3 与 Phase 4（多级空间索引）的关系

LOD 系统的 [`LodManager::chunk_lods`] 只是一个临时的 HashMap 记录。当未来实现多级空间索引时，LOD 信息可以作为索引的一部分存储。

### 13.4 与 Phase 5（内存池）的关系

LOD 网格的缓冲区复用与全精度网格一致，内存池可以直接支持。

---

> **参考文档**
> - [Voxy借鉴优化方案.md](../docs/Voxy借鉴优化方案.md) — Phase 1 LOD 系统优先级
> - [架构总纲.md](../docs/架构总纲.md) — LOD 分级系统定义
> - [异步网格生成] — 当前 Phase 0 实现细节
> - [Greedy-Meshing实现记录.md](../docs/Greedy-Meshing实现记录.md) — 备选网格优化方案
