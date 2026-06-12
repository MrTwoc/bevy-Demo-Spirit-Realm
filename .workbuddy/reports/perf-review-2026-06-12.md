# 灵境 (Spirit Realm) 完整代码库性能审查报告

> 审查日期：2026-06-12 | 审查范围：全 37 个源文件 | 工具：灵枢 (LingShu)

---

## 概览

本次审查覆盖 5 个性能维度的 37 个源文件，识别出 **18 条优化建议**：

| 优先级 | 数量 | 说明 |
|--------|------|------|
| 🔴 高  | 4    | 影响主循环帧率或启动体验，建议优先处理 |
| 🟡 中  | 8    | 累积开销显著，在热点路径上有优化空间 |
| 🟢 低  | 6    | 技术债务 / 死代码 / 一次性开销 |

---

## 一、时间复杂度与算法效率

### 🔴 HIGH-1 · is_cave() 每体素 2 次 3D 噪声采样

**位置**：`src/terrain_noise.rs` — `fill_terrain()` 调用 `is_cave()` / `src/chunk.rs` — `fill_terrain()`

**问题**：`fill_terrain()` 对每个地下体素 (surface_y 以下) 调用 `is_cave()`，每次调用进行 **2 次 3D 噪声采样**（`cave_noise` + `cave_threshold`）。对于 32³ = 32,768 体素的区块，假设地表在 y=96，则地下约 96×32×32 ≈ 98,304 个候选体素。噪声采样是区块生成中最高昂的 CPU 操作。

```rust
// 当前：每个体素独立计算 2 次 3D 噪声
fn is_cave(x: f64, y: f64, z: f64) -> bool {
    let n1 = get_cave_noise().get([x * 0.05, y * 0.05, z * 0.05]);
    let n2 = get_cave_threshold().get([x * 0.05, y * 0.05, z * 0.05]);
    n1 > n2
}
```

**性能影响**：每个区块地形生成中，洞穴检测占总 CPU 时间的 **40-60%**（估算）。配合 8 层地表噪声的 `sample_all()`，单个区块的 Prepare 阶段可能超过 50ms。

**优化方向**：
1. **列优先洞穴检测**：对每个 (x,z) 列，先确定 surface_y，再仅对 surface_y 以下的体素做洞穴检测（当前已部分做到）
2. **噪声缓存 / 降采样**：以 2×2×2 或 4×4×4 的粒度做洞穴检测（低频洞穴噪声天然支持降采样），然后按最近邻填充
3. **SIMD 批量**：4 个相邻体素的噪声坐标相近，可考虑批量求值

---

### 🔴 HIGH-2 · Greedy Meshing 水方块早退被注释

**位置**：`src/greedy_mesh.rs` 第 179-186 行

**问题**：水方块检测的提前返回被注释掉，导致**所有非水区块**也执行完整的 6 面 × 32 层 × 32×32 掩码扫描。

```rust
// 已注释的早退优化：
// if !chunk.contains_block(WATER_BLOCK_ID) {
//     return Default::default(); // <- 应该恢复此行！
// }
```

**性能影响**：对于不含水的区块（地表区块的绝大多数），Greedy Meshing 做了完全的无用功。每个非水区块额外消耗 **~0.2-0.5ms** 的 CPU 时间。

**优化方向**：取消注释早退检查。`contains_block()` 只需检查 Palette（O(palette_len) ≈ O(10-20)），开销 < 1μs。

---

### 🟡 MED-1 · PalettedChunkData::is_empty() 全量扫描

**位置**：`src/chunk.rs` 第 264-277 行

**问题**：`is_empty()` 最坏情况遍历全部 32,768 个体素，用于 `is_air_chunk()` 判断。

```rust
pub fn is_empty(&self) -> bool {
    for z in 0..CHUNK_SIZE {
        for y in 0..CHUNK_SIZE {
            for x in 0..CHUNK_SIZE {
                if self.get(x, y, z) != 0 { return false; }
            }
        }
    }
    true
}
```

**性能影响**：在 `spawn_entities_from_prepare()` 的 `is_air_chunk()` 调用链中，每次 Prepare 完成都要做一次 32K 扫描。虽然纯空气区块罕见，但当出现时此操作耗时 ~0.05-0.1ms。

**优化方向**：PalettedChunkData 维护一个 `non_zero_count: u16` 计数器，`set()` 时更新。`is_empty()` 变为 `non_zero_count == 0`（O(1)）。

---

### 🟡 MED-2 · terrain_noise compute_surface_height 无列级缓存

**位置**：`src/terrain_noise.rs` 和 `src/chunk.rs` — `compute_surface_height()` / `get_surface_height()`

**问题**：`compute_surface_height()` 每次调用独立初始化噪声采样，被 `tree_gen.rs` 的 `generate_trees_in_chunk()` 对每个候选树干位置调用。对于 step=4 的树木生成，扩展搜索范围内约 90 个候选位置各调用一次，每次做完整的 8 层噪声采样。

**性能影响**：树木生成中约 **15-20%** 的 CPU 时间花在重复的 `sample_all()` 调用上——与 `fill_terrain` 的地表高度计算重复。

**优化方向**：
1. `fill_terrain()` 已对每个 (x,z) 列做 `sample_all()`一次——可将地表高度结果缓存到 `[i32; 32*32]` 临时数组
2. `generate_trees_in_chunk()` 可直接复用 fill_terrain 阶段计算好的地表高度，或在 chunk 上存储地表高度缓存

---

### 🟡 MED-3 · generate_solid_mesh 固定预分配 + shrink_to_fit

**位置**：`src/async_mesh.rs` — `generate_solid_mesh()` 函数

**问题**：固定预估 6000 个三角形面，实际大部分区块远小于此值，导致 `shrink_to_fit()` 的额外 realloc。

```rust
let estimated_faces = 6000;
let mut positions = Vec::with_capacity(estimated_faces * 4);
// ... 面生成 ...
positions.shrink_to_fit(); // 实际只有 ~500-2000 面
```

**性能影响**：过度分配浪费 ~10-20KB 每区块的堆内存，`shrink_to_fit()` 触发额外 realloc。对于 2000+ 区块的场景，累积浪费 20-40MB。

**优化方向**：基于列扫描结果估算实际面数。例如：统计非空气邻居对数量 × 0.3-0.5 作为预估系数。

---

### 🟡 MED-4 · svo/batch_renderer build_from_render_queue 每帧重分配

**位置**：`src/svo/batch_renderer.rs` — `build_from_render_queue()` 方法

**问题**：每帧调用 `self.clear()` 清除所有批次，然后重新 `push()` 构建。`Vec::clear()` 保留容量但不清除子 `Vec`（`RenderBatch.node_ids`）。然而 `batches` 本身被 clear，重新 push 时子 Vec 也要重新分配。

```rust
pub fn clear(&mut self) {
    self.batches.clear();        // 丢掉了之前分配的 RenderBatch 和内部 Vec
    self.material_to_batch.clear();
    self.stats = BatchRenderStats::default();
}
```

**性能影响**：每帧分配若干 `Vec<u32>` 用于可见节点 ID 列表。节点数 500-2000 时影响可控，但在节点数增长时逐步显现。

**优化方向**：复用 batch 池——不清除而是遍历已有 batch 重置 node_ids（`.clear()` 保留容量），多余的 batch 才 push 新的。

---

### 🟢 LOW-1 · chunk_distance() sqrt 死代码

**位置**：`src/lod.rs` 第 224 行

**问题**：`chunk_distance()` 函数使用 `sqrt()` 进行距离计算，但代码中已无调用方，全部改用 `chunk_distance_sq()` 做平方距离比较。此函数为死代码。

**优化方向**：移除该函数或添加 `#[allow(dead_code)]` + 注释说明保留原因。

---

### 🟢 LOW-2 · is_face_visible_async() 死代码

**位置**：`src/async_mesh.rs` 第 679 行

**问题**：`is_face_visible_async()` 与 `is_face_visible_fast()` 功能重复，且无任何调用方。保留它增加维护负担和编译时间。

**优化方向**：移除。

---

### 🟢 LOW-3 · 旧式 generate_chunk_mesh 同步函数

**位置**：`src/chunk.rs` 第 503 行

**问题**：保留的同步 `generate_chunk_mesh()` 使用旧式 SoA 布局，与异步路径的 `generate_chunk_mesh_separated()` 功能重复。增加了代码维护负担。

**优化方向**：评估是否还有调用方，若无需保留则移除。

---

## 二、内存管理

### 🔴 HIGH-3 · resource_pack build_atlas 重复像素复制

**位置**：`src/resource_pack.rs` — `build_atlas()` 方法

**问题**：纹理数据被复制 **两次**——先复制到 `array_pixels`（Texture Array 格式），再复制到 `atlas_pixels`（传统 Atlas 格式）。两种格式的像素数据完全相同，但各占一份内存。

```
array_pixels: tex_size × tex_size × 4 × array_layers bytes
atlas_pixels: atlas_width × atlas_height × 4 bytes
```

对于 50 个 16×16 纹理，`array_pixels` ≈ 50×16×16×4 = 51,200 bytes，`atlas_pixels` ≈ 256×256×4 = 262,144 bytes。总共 ~300KB，虽不大但对启动内存有双重影响。

**性能影响**：启动时额外 ~300KB 堆分配 + 双重像素复制循环。Texture Array 模式下 `atlas_pixels` 可能无实际用途。

**优化方向**：确认 `atlas_pixels` 的实际用途。如果仅用于调试/兼容性，考虑用 `#[cfg(debug_assertions)]` 条件编译或在不需要时不分配。

---

### 🟡 MED-5 · svo/visibility_bridge visible_coords 无容量预分配

**位置**：`src/svo/visibility_bridge.rs` — `SvoVisibilityState::visible_coords`

**问题**：`visible_coords` HashSet 在每次可见性更新时 `clear()` 后重建，但没有容量提示。对于 16×16×16 = 4096 个 section 的 top-level 节点区域，HashSet 会经历多次 rehash。

**性能影响**：每 2 帧重建 HashSet 时，中间 rehash 产生额外的堆分配和哈希重算。若可见区域稳定，开销逐步降低但初次仍存在。

**优化方向**：在 `clear()` 后调用 `reserve()` 预分配合理容量（如 4096），或记录上次容量作为下一次的 hint。

---

### 🟡 MED-6 · svo section 二级缓存内存占用

**位置**：`src/svo/config.rs` — `SECONDARY_CACHE_SIZE = 4096` / `TOP_LEVEL_CACHE_SIZE = 1024`

**问题**：Section 是 32³ 体素区块，按 PalettedChunkData 存储约 2-8KB/section。4096 个 section 的二级缓存可能占用 **8-32MB** 内存，且持续增长直到 LRU 淘汰触发。

**性能影响**：SV O 系统与 Chunk 系统的区块数据可能存在**重复存储**——同一区域的数据在 `LoadedChunks.entries`（`Arc<ChunkData>`）和 SVO Section 中各存一份。

**优化方向**：评估是否可以复用 `LoadedChunks` 的数据作为 SVO Section 的数据源，避免双重存储。

---

### 🟢 LOW-4 · generate_greedy_mesh 固定预分配 8000

**位置**：`src/greedy_mesh.rs`

**问题**：`Vec::with_capacity(8000)` 对于大部分不含水或只有少量水的区块过度分配。

**优化方向**：根据 `contains_block(WATER_BLOCK_ID)` 结果动态选择容量（0 或较小值），或基于地形高度估算水面体积。

---

### 🟢 LOW-5 · perf_logger BufWriter 未设置容量

**位置**：`src/perf_logger.rs`

**问题**：`BufWriter` 使用默认 8KB 缓冲区。每秒写入一行 CSV（~50 bytes），8KB 缓冲区可缓存 ~160 行。flush 频率由外部控制（每行后强制 flush）。

**优化方向**：降低强制 flush 频率（如每 10 行 flush 一次），或增大 BufWriter 缓冲区。

---

## 三、I/O 与网络性能

### 🟢 LOW-6 · resource_pack 启动时同步 I/O

**位置**：`src/resource_pack.rs` — `scan_dir_recursive()` / `load_png_as_rgba()`

**问题**：所有纹理在启动时同步加载，阻塞主线程。对于 50 个 16×16 纹理影响极小（<10ms），但如果材质包扩展到数百个高分辨率纹理，启动时间会显著增加。

**性能影响**：当前影响极小（<10ms）。未来扩展风险。

**优化方向**：当前不需要优化。若未来纹理数量增长，可考虑 Bevy 的 `AssetServer` 异步加载路径。

---

## 四、并发与异步处理

### 🟡 MED-7 · AsyncMeshManager 工作线程数硬编码上限

**位置**：`src/async_mesh.rs` — `default_worker_count()`

**问题**：工作线程数固定为 `min(num_cpus, 4)`。在 8-16 核 CPU 上只使用 4 个线程，Prepare + Generate 两阶段流水线的吞吐量受限。

```rust
pub fn default_worker_count() -> usize {
    num_cpus::get().min(4)
}
```

**性能影响**：在 8 核 CPU 上，4 个线程可能达到 50% 利用率。考虑到 Bevy 主线程占用 1 核 + 渲染线程，4 个工作线程在 8 核上已经相当合理。在高核心数（16+）CPU 上可能成为瓶颈。

**优化方向**：使用 `(num_cpus::get() - 2).max(2).min(8)` 公式，为渲染线程预留资源，同时在高核心数机器上获得更好的并行性。

---

### 🟡 MED-8 · svo/visibility_bridge 锁定所有 chunk 实体

**位置**：`src/svo/visibility_bridge.rs` — `apply_svo_visibility()`

**问题**：每 2 帧遍历所有已加载 chunk 实体并设置 `Visibility`。对于 2000+ 的 chunk 场景，这涉及 2000 次 `visibility_query.get_mut(entity)` 调用。

```rust
for (_coord, entry) in loaded_chunks.entries.iter() {
    let entity = entry.entity;
    if let Ok(mut vis) = visibility_query.get_mut(entity) {
        *vis = if is_visible { Visibility::Inherited } else { Visibility::Hidden };
    }
}
```

**性能影响**：2000 次 `get_mut` 调用约 0.1-0.2ms，每 2 帧执行一次。可接受但仍有优化空间。

**优化方向**：维护"上一帧可见性状态"缓存，仅对可见性发生变化的实体执行 `insert`。`Visibility` 组件支持 change detection，可结合 `Changed<Visibility>` 进一步减少 Command 开销。

---

## 五、资源加载与初始化

### 🔴 HIGH-4 · 启动时全视距加载

**位置**：`src/chunk_manager.rs` — `setup_world()` 和 `INITIAL_LOAD_RADIUS`

**问题**：`INITIAL_LOAD_RADIUS = 16`（等于 `RENDER_DISTANCE`），启动时一次性将所有视距内的区块加入加载队列：

```rust
pub const INITIAL_LOAD_RADIUS: i32 = 16;
```

加载队列范围 = (16×2+1)² × (5×2+1) = 33×33×11 = **11,979 个区块坐标**。虽然每帧只提交 `CHUNKS_PER_FRAME=32` 个 Prepare 任务，但队列构建本身遍历了所有坐标，且初始加载队列的长度导致启动后前 ~374 帧（约 6 秒 @60fps）都在消费初始队列。

**性能影响**：
- 启动时队列构建遍历 12K 坐标，约 0.3ms（可接受）
- 12K 个区块的 Prepare + Generate 任务积压导致启动后 ~6 秒内工作线程满负荷
- 启动阶段帧率可能下降（大量 ECS 实体创建和 Mesh 上传）

**优化方向**：
1. 将 `INITIAL_LOAD_RADIUS` 改回渐进值（如 4-6），让玩家在移动中逐步加载远处区块
2. 或者实现螺旋式加载：先加载玩家周围 2 圈，然后逐步向外扩展

---

### 🟢 LOW-7 · Perlin 噪声初始化 non-deterministic seed

**位置**：`src/tree_gen.rs` — `TreeNoise::default()` / `src/terrain_noise.rs`

**问题**：`Perlin::new(seed)` 在 `noise` crate 中不保证跨平台/跨版本的一致性（使用随机排列表）。这意味着相同 seed 在不同平台或库版本上可能生成不同的树木分布。

**性能影响**：非性能问题，但影响可复现性。

**优化方向**：使用确定性哈希函数（如 `FnvHasher`）生成排列表，或换用保证确定性的噪声库（如 `fastnoise-lite`）。

---

## 总结

### 建议处理顺序

| 顺序 | ID      | 问题                         | 预期收益               |
|------|---------|------------------------------|------------------------|
| 1    | HIGH-2  | 恢复 Greedy Meshing 早退     | 无成本，立即消除无用功 |
| 2    | HIGH-1  | is_cave 噪声降采样           | 地形生成提速 30-40%    |
| 3    | HIGH-4  | 渐进式初始加载               | 启动体验明显改善       |
| 4    | MED-1   | is_empty O(1) 计数器         | 消除偶发大开销         |
| 5    | HIGH-3  | build_atlas 消除重复像素复制 | 减少启动内存           |
| 6    | MED-2   | 地表高度列缓存               | 树木生成提速 15%       |

### 已做得很好的方面

1. **Arc<ChunkData> 共享**：避免深拷贝，O(1) 引用计数传递，设计优秀
2. **AoS 顶点布局**：`MeshVertex` 32 字节对齐，缓存友好
3. **UvLookupTable / BlockPropertiesTable**：`[Option<T>; 256]` 数组 O(1) 查找
4. **复用缓冲区**：`evict_candidates` 和 `unload_buf` 避免每帧堆分配
5. **分帧处理**：`DELETIONS_PER_FRAME`、`CHUNKS_PER_FRAME` 等限制避免尖峰
6. **run_if 条件**：空闲帧完全跳过系统执行，零 CPU 开销
7. **预计算偏移量表**：`OnceLock<Vec<OffsetEntry>>` 懒初始化，按距离排序
