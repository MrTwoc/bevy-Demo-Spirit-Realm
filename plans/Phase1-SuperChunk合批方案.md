# Phase 1 实施计划：P0 修复 + SuperChunk 合批系统

> 基于架构总纲 §6.2 的 Phase 1（P1）规划，结合当前代码状态制定。
> 前置条件：先修复 P0 #1 GPU 泄漏，再实现 SuperChunk。

---

## Step 1: 修复 P0 #1 GPU 资源泄漏

### 问题定位

[`src/chunk_manager.rs`](../src/chunk_manager.rs) 中的两个函数：

1. [`unload_distant_chunks`](../src/chunk_manager.rs:277-295) — `commands.entity(entry.entity).despawn()` 直接销毁实体，没有清理 [`ChunkMeshHandle`](../src/chunk_dirty.rs:24-28) 中的 mesh/material
2. [`lru_evict`](../src/chunk_manager.rs:303-341) — 同上

虽然 [`rebuild_dirty_chunks`](../src/chunk_dirty.rs:73-145) 在重建时做了清理，但**卸载/淘汰路径是遗漏的**。长期运行（如 LRU 频繁淘汰）会导致 GPU 内存积累。

### 修复方案

在两个函数中加入资源清理逻辑：

```rust
// 在 despawn 前：
fn cleanup_chunk_entity(
    entity: Entity,
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) {
    let mut entity_mut = commands.entity(entity);
    if let Some(handle) = entity_mut.take::<ChunkMeshHandle>() {
        meshes.remove(&handle.mesh);
        materials.remove(&handle.material);
    }
    entity_mut.despawn();
}
```

需要在 `chunk_loader_system` 中把 `mut meshes` / `mut materials` 传给这两个函数。

### 涉及文件

| 文件 | 修改内容 |
|------|---------|
| [`src/chunk_manager.rs`](../src/chunk_manager.rs) | `unload_distant_chunks` + `lru_evict` 增加资源清理参数和逻辑 |

---

## Step 2: SuperChunk 合批系统

### 2.1 概念回顾

来自 [`docs/体素管理方案.md`](../docs/体素管理方案.md:262)：

| 层级 | 尺寸 | 用途 |
|------|------|------|
| SubChunk | 32³ (1×1×1) | 最小加载/编辑单位 |
| SuperChunk | 8×8×4 SubChunk | 256×256×128 米，近景合批单位 |

一个 SuperChunk 包含 8×8×4 = **256** 个 SubChunk。

### 2.2 新模块结构

```
src/
├── superchunk.rs          # [新增] SuperChunk 定义 + 网格合并逻辑
├── chunk.rs               # [修改] 新增 SuperChunkCoord 类型
├── chunk_manager.rs       # [修改] SuperChunk 调度逻辑
├── chunk_dirty.rs         # [修改] 脏标记扩散到 SuperChunk 级别
└── main.rs                # [修改] 注册新模块
```

### 2.3 新增数据结构

#### [`src/superchunk.rs`] `SuperChunkCoord`

```rust
/// SuperChunk 坐标（将 SubChunk 坐标除以 8/8/4）
///
/// mapping: super_cx = cx.div_euclid(8)
///          super_cy = cy.div_euclid(4)
///          super_cz = cz.div_euclid(8)
#[derive(Clone, Copy, Hash, Eq, PartialEq, Debug)]
pub struct SuperChunkCoord {
    pub sx: i32,
    pub sy: i32,
    pub sz: i32,
}

impl SuperChunkCoord {
    /// 从 SubChunk 坐标计算所属 SuperChunk
    pub fn from_subchunk(cx: i32, cy: i32, cz: i32) -> Self;

    /// 获取此 SuperChunk 包含的所有 SubChunk 坐标
    pub fn subchunk_coords(&self) -> Vec<ChunkCoord>;

    /// 转换为世界原点坐标（AABB 最小值）
    pub fn to_world_origin(&self) -> Vec3;
}
```

#### [`src/superchunk.rs`] `SuperChunkState`

```rust
/// SuperChunk 的生命周期状态
pub enum SuperChunkState {
    /// 正在等待 SubChunk 加载完成
    Pending,
    /// SubChunk 已加载，网格已构建，渲染中
    Active(Entity),
    /// SubChunk 数据还在，但网格已销毁（离开近景范围）
    Dormant,
}
```

#### [`src/superchunk.rs`] `SuperChunkManager`

```rust
#[derive(Resource)]
pub struct SuperChunkManager {
    /// SuperChunk 索引
    pub entries: HashMap<SuperChunkCoord, SuperChunkState>,
    /// 脏队列（待重建的 SuperChunk）
    pub rebuild_queue: Vec<SuperChunkCoord>,
}
```

### 2.4 网格合并算法

在 [`src/superchunk.rs`] 中实现：

```rust
/// 将 SuperChunk 内所有 SubChunk 的网格合并为单一 Mesh。
///
/// 核心逻辑：
/// 1. 遍历所有 SubChunk 生成网格（复用现有 generate_chunk_mesh）
/// 2. 将每个 SubChunk 的顶点位置偏移到世界坐标
/// 3. 合并顶点/UV/法线/索引数组
/// 4. 索引需要累加偏移量
pub fn merge_superchunk_mesh(
    subchunks: &[(ChunkData, Vec3)],  // (chunk_data, world_position)
    resource_pack: &ResourcePackManager,
    neighbors_provider: &impl Fn(ChunkCoord) -> ChunkNeighbors,
) -> (Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<[f32; 3]>, Vec<u32>) {
    let mut all_positions = Vec::new();
    let mut all_uvs = Vec::new();
    let mut all_normals = Vec::new();
    let mut all_indices = Vec::new();
    let mut base_index: u32 = 0;

    for (chunk_data, chunk_origin) in subchunks {
        let (positions, uvs, normals, indices) =
            generate_chunk_mesh(chunk_data, resource_pack, &neighbors);

        // 偏移顶点到世界坐标
        let offset = chunk_origin;
        for pos in &positions {
            all_positions.push([
                pos[0] + offset.x,
                pos[1] + offset.y,
                pos[2] + offset.z,
            ]);
        }
        all_uvs.extend(uvs);
        all_normals.extend(normals);
        all_indices.extend(indices.iter().map(|i| i + base_index));
        base_index += positions.len() as u32;
    }

    (all_positions, all_uvs, all_normals, all_indices)
}
```

### 2.5 调度逻辑变更

#### 当前流程（修改前）

```
chunk_loader_system:
  加载 SubChunk → fill_terrain → spawn_chunk_entity (每个 SubChunk 一个实体)
```

#### 新流程（修改后）

```
chunk_loader_system:
  加载 SubChunk → fill_terrain
    ↓
  注册到 SuperChunkManager (不立即生成实体)
    ↓
  检查所属 SuperChunk 是否所有 SubChunk 已加载完成
    ↓
  是 → 合并网格 → spawn 一个 SuperChunk 实体
  否 → 等待后续 SubChunk 加载
```

#### 脏标记扩散

```rust
// 当 SubChunk 被标记为脏时：
fn mark_superchunk_dirty(subchunk_coord: ChunkCoord, manager: &mut SuperChunkManager) {
    let super_coord = SuperChunkCoord::from_subchunk(
        subchunk_coord.cx,
        subchunk_coord.cy,
        subchunk_coord.cz,
    );
    if !manager.rebuild_queue.contains(&super_coord) {
        manager.rebuild_queue.push(super_coord);
    }
}
```

### 2.6 渲染管线变更

| 方面 | 当前行为 | 新行为 |
|------|---------|--------|
| 实体数 | ~600 个 SubChunk 实体 | ~3 个 SuperChunk 实体（RENDER_DISTANCE=8） |
| Draw Call | 每个 SubChunk 一个 | 每个 SuperChunk 一个 |
| 网格更新时间 | 单个 SubChunk 独立更新 | 整个 SuperChunk（256 个）一起更新 |
| LOD 扩展 | 不支持 | 可为后续 LOD 切换留接口 |

### 2.7 与现有系统的协调

1. **`block_interaction_system`** — 修改方块后标记 SubChunk 脏 → `chunk_dirty` 扩散到 SuperChunk → 重建 SuperChunk 网格
2. **`raycast_highlight_system`** — 不变，射线检测基于 `ChunkData` 组件查询
3. **HUD 统计** — 三角形计数改为统计 SuperChunk 而非 SubChunk
4. **线框模式** — 改为绘制 SuperChunk 范围的线框

### 2.8 分阶段实施建议

| 子步骤 | 说明 | 涉及文件 |
|--------|------|---------|
| 2.8.1 | 定义 `SuperChunkCoord` 和基础方法 | `src/superchunk.rs` [新增] |
| 2.8.2 | 实现 `SuperChunkManager` Resource | `src/superchunk.rs` |
| 2.8.3 | 实现网格合并 `merge_superchunk_mesh` | `src/superchunk.rs` |
| 2.8.4 | 修改 `chunk_loader_system`：SubChunk → SuperChunk 实体 | `src/chunk_manager.rs` |
| 2.8.5 | 脏标记扩散到 SuperChunk 级别 | `src/chunk_dirty.rs` |
| 2.8.6 | 区块卸载时同步清理 SuperChunk | `src/chunk_manager.rs` |
| 2.8.7 | `main.rs` 注册新模块 + 系统 | `src/main.rs` |
| 2.8.8 | 测试：验证合批后渲染正确性 | 手动测试 |

---

## Step 3: 预期收益

| 指标 | 当前（RENDER_DISTANCE=8） | SuperChunk 后 |
|------|--------------------------|---------------|
| 实体数 | ~600 SubChunk 实体 | ~3 SuperChunk 实体 + ~600 SubChunk 数据 |
| Draw Call | ~600 | ~3 |
| 网格重建 | 单 SubChunk 独立，约 0.1ms | 整 SuperChunk 重建，约 25ms（但频率更低） |
| 内存 | 600 个独立 mesh 对象 | 3 个合并 mesh 对象 |

---

## 涉及文件清单（汇总）

| 文件 | 操作 | 修改概要 |
|------|------|---------|
| [`src/chunk_manager.rs`](../src/chunk_manager.rs) | 修改 | P0 泄漏修复 + SuperChunk 调度 |
| [`src/superchunk.rs`](../src/superchunk.rs) | **新增** | SuperChunkCoord, SuperChunkManager, merge_superchunk_mesh |
| [`src/chunk_dirty.rs`](../src/chunk_dirty.rs) | 修改 | 脏标记扩散到 SuperChunk |
| [`src/main.rs`](../src/main.rs) | 修改 | 注册 `mod superchunk` + 系统 |
| [`src/chunk.rs`](../src/chunk.rs) | 可能微调 | 导出 `ChunkCoord` 相关方法 |


潜在问题和意见
```markdown
# SuperChunk 合批方案：潜在问题与改进建议

> 基于《Phase 1 实施计划：P0 修复 + SuperChunk 合批系统》的架构、Rust 实现及图形学专家评审，提炼以下核心意见。

---

## 1. 游戏架构风险与建议

### 1.1 刚性合并触发条件（“所有 SubChunk 就绪才合并”）
- **风险**：若某些 SubChunk 长期未被加载（世界边缘、加载失败），对应 SuperChunk 将永远卡在 `Pending` 状态，不会生成任何渲染实体。
- **建议**：引入 **部分合并策略**，当加载完成率达到可配置阈值（如 80%）时即进行合批，缺失 SubChunk 的位置渲染为透明或填充空网格；或实现 **超时回退**，避免无限等待。

### 1.2 SubChunk 数据生命周期与所有权模糊
- **风险**：方案提到“~600 SubChunk 数据仍在内存”，但未说明这些数据由谁持有（`ChunkData` 组件？`SuperChunkManager`？）。若 SubChunk 实体被销毁而数据未被正确管理，LRU 淘汰等操作可能再次引发内存泄漏。
- **建议**：明确定义 SubChunk 数据的所有权归属，推荐由 `SuperChunkManager` 集中持有已加载的 `ChunkData`，并配套设计 **对称的卸载路径**（清除数据 + 清理 `SuperChunkState`）。

### 1.3 卸载顺序与悬垂引用
- **风险**：`SuperChunkState::Active(Entity)` 直接持有实体 ID，若该实体因卸载被 despawn 而状态未同步更新，将留下悬垂引用，导致后续访问崩溃。
- **建议**：卸载时严格遵守 **先移除 `SuperChunkManager` 记录 → 再销毁实体** 的顺序；或使用独立 `HashMap<SuperChunkCoord, Entity>` 存储实体关系，并配合 Bevy 的 `EntityHashMap` 自动清理。

---

## 2. Rust 实现问题与改进

### 2.1 `subchunk_coords()` 不必要的堆分配
- **问题**：返回 `Vec<ChunkCoord>` 会在每次调用时动态分配 256 个元素的堆内存，若在帧循环内高频调用将产生显著分配开销。
- **改进**：改为返回固定大小数组 `[ChunkCoord; 256]` 或返回 `impl Iterator<Item = ChunkCoord>`，采用确定性计算避免堆分配。

### 2.2 索引累加潜在溢出
- **问题**：`merge_superchunk_mesh` 中使用 `u32` 累加 `base_index`，虽单个 SuperChunk 远低于上限，但缺乏防护。
- **改进**：添加调试断言 `debug_assert!(base_index + positions.len() <= u32::MAX as usize)`；或内部使用 `u64` 累加后再截断，并明确注释安全边界。

### 2.3 `SuperChunkState::Active(Entity)` 的设计缺陷
- **问题**：将实体 ID 直接嵌入状态枚举，导致状态与实体生命周期强耦合，卸载后易产生无效 Entity。
- **改进**：
  - 将实体映射独立存储（如 `HashMap<SuperChunkCoord, Entity>`）；
  - 或使用新类型 `Active { entity: Entity }` 并配合 `Component` 双向标记，在实体销毁时通过 `RemovedComponents` 自动回写状态。

### 2.4 清理函数可能引发借用冲突
- **问题**：`cleanup_chunk_entity` 同时接收 `&mut Commands`、`&mut Assets<Mesh>` 等，若系统签名中已持有这些资源的可变借用，直接调用将违反 Rust 借用规则。
- **改进**：将清理逻辑封装为独立 **Bevy 系统**（如 `cleanup_removed_chunks`），通过 `RemovedComponents<ChunkMeshHandle>` 监听或利用 **命令缓冲**（`Commands`）延迟执行，避免函数级参数传递冲突。

---

## 3. 图形学性能与视觉风险

### 3.1 SuperChunk 一次性重建导致明显卡顿
- **风险**：单次合并 256 个 SubChunk 网格约需 25ms，远超出 16.67ms（60fps）帧预算，玩家放置/破坏方块时会感知到可察觉的停顿。
- **建议**：
  - **分帧合并**：每帧合并固定数量 SubChunk（如 16 个），16 帧内完成整个 SuperChunk 重建，重建期间保留旧网格；
  - **异步计算**：将网格合并移至后台线程（需评估与 Bevy ECS 的数据交互开销）。

### 3.2 材质与纹理同质化假设
- **风险**：方案假设所有 SubChunk 使用同一 `StandardMaterial`，若引入不同纹理图集或特殊材质（透明、自发光），单一合并网格将丢失材质多样性。
- **建议**：在当前阶段明确约束 “SuperChunk 内仅使用统一纹理图集和同一 Shader”；为未来扩展预留 **材质分层合批** 或 **多段合并网格** 的接口。

### 3.3 包围盒与剔除精度劣化
- **风险**：单个 SuperChunk 网格尺寸最大可达 256×256×128 米，可能导致视锥体边缘出现“应渲染部分被整体剔除”或“大量不可见区域被绘制”的情况，浪费带宽和算力。
- **建议**：Phase 1 可接受此精度损失（因近景距离内 SuperChunk 通常几乎完全可见）；长期可考虑为合并网格生成更紧凑的包围体（如 OBB），或使用基于 SubChunk 的次级剔除。

### 3.4 法线与背面剔除正确性验证
- **提醒**：顶点在世界空间偏移后进行合并，法线方向不受平移影响，背面剔除仍能正常工作。但若后续引入非均匀缩放或复杂变换，需重新归一化法线。当前方案无此风险，仅作备忘。

---

## 4. 综合优先级建议

| 优先级 | 问题 | 理由 |
|-------|------|------|
| P0（编码前必解决） | 刚性合并触发条件（1.1）<br>SuperChunk 重建卡顿（3.1） | 直接影响可玩性与交互体验 |
| P1（实现阶段改进） | `subchunk_coords` 分配（2.1）<br>`Active(Entity)` 设计（2.3）<br>清理函数借用（2.4） | 避免技术债务与潜在崩溃 |
| P2（后续迭代关注） | 所有权定义（1.2）<br>包围盒精度（3.3）<br>材质同质化（3.2） | 架构健壮性与未来扩展 |

> 整体方案方向正确、收益显著，在落实上述改进后可作为稳定的基础优化里程碑。
```

---

## 社区反馈评估与采纳决策

> 基于对方案正文（§1-§3）和现有代码（`chunk_manager.rs`、`chunk_dirty.rs`、`chunk.rs`）的逐条审阅，形成以下评估。

### 评估结论总览

| 类别 | 完全合理 | 部分合理 | 不合理 |
|------|---------|---------|--------|
| 架构风险（§1） | 3 条 | 0 条 | 0 条 |
| Rust 实现（§2） | 2 条 | 2 条 | 0 条 |
| 图形学性能（§3） | 3 条 | 1 条 | 0 条 |
| **合计** | **8 条** | **3 条** | **0 条** |

---

### 逐条评估

#### 1.1 刚性合并触发条件 — ✅ 完全合理，采纳

- **问题确认**：方案 §2.5 第192行要求"所有 SubChunk 已加载完成才合并"。在世界边缘，部分 SubChunk 永远不会加载，对应 SuperChunk 将永久卡在 `Pending` 状态，**整个区域无法渲染**。
- **采纳方案**：实现**超时回退机制**——SubChunk 加载后启动计时器，超时后以已加载的 SubChunk 进行部分合并，缺失位置渲染为空。
- **实施阶段**：P0（编码前确定设计）

#### 1.2 SubChunk 数据生命周期与所有权 — ✅ 完全合理，采纳

- **问题确认**：方案 §2.6 提到"~600 SubChunk 数据仍在内存"，但未说明数据归属。当前代码中 [`ChunkEntry`](src/chunk_manager.rs:33) 在 [`LoadedChunks`](src/chunk_manager.rs:45) 中持有 `data: Chunk`，方案新流程说"注册到 SuperChunkManager"但未明确迁移路径。
- **采纳方案**：在方案中补充**数据所有权图**——`ChunkData` 继续由 `LoadedChunks` 持有（因为 raycast、block_interaction 等系统仍需访问），`SuperChunkManager` 仅持有 `SuperChunkCoord → Entity` 映射和状态。
- **实施阶段**：P1（实现阶段明确）

#### 1.3 卸载顺序与悬垂引用 — ✅ 完全合理，采纳

- **问题确认**：[`SuperChunkState::Active(Entity)`](plans/Phase1-SuperChunk合批方案.md:111) 直接持有 Entity ID。当前 [`unload_distant_chunks`](src/chunk_manager.rs:303) 和 [`lru_evict`](src/chunk_manager.rs:340) 直接 despawn 实体，无 SuperChunk 层面清理，会导致悬垂引用。
- **采纳方案**：
  1. 将实体映射从枚举中分离，使用独立 `HashMap<SuperChunkCoord, Entity>` 存储
  2. 卸载时严格遵守**先移除 SuperChunkManager 记录 → 再销毁实体**的顺序
  3. 配合 Bevy 的 `RemovedComponents` 自动清理作为兜底
- **实施阶段**：P0（编码前确定设计）

#### 2.1 `subchunk_coords()` 堆分配 — ✅ 完全合理，采纳

- **问题确认**：返回 `Vec<ChunkCoord>` 每次分配 256 个元素堆内存。
- **采纳方案**：改为返回 `impl Iterator<Item = ChunkCoord>`，避免堆分配。
- **实施阶段**：P1（实现阶段改进）

#### 2.2 索引累加溢出 — ⚠️ 部分合理，低优先级采纳

- **分析**：单个 SubChunk 32³ 面剔除后通常几千到几万顶点，256 个 SubChunk 合并后远低于 `u32::MAX`（42.9亿），实际溢出风险极低。
- **采纳方案**：添加 `debug_assert!` 作为防御性编程，不做架构变更。
- **实施阶段**：P2（后续迭代）

#### 2.3 `Active(Entity)` 设计缺陷 — ✅ 完全合理，与 1.3 合并处理

- **分析**：与 1.3 本质相同，从 Rust 类型设计角度强调。将实体映射从枚举中分离是正确的 Bevy 实践。
- **采纳方案**：与 1.3 一并处理，使用 `HashMap<SuperChunkCoord, Entity>` + `RemovedComponents` 模式。
- **实施阶段**：P0（编码前确定设计）

#### 2.4 清理函数借用冲突 — ⚠️ 部分合理，降级处理

- **分析**：查看现有代码，[`unload_distant_chunks`](src/chunk_manager.rs:303) 和 [`lru_evict`](src/chunk_manager.rs:340) 已以相同方式接收 `&mut Commands`、`&mut Assets<Mesh>` 等参数并正常工作。方案中的 `cleanup_chunk_entity` 与现有模式一致，**在当前架构下不会产生借用冲突**。
- **采纳方案**：P0 修复阶段沿用现有参数传递模式；SuperChunk 实现阶段再考虑封装为独立系统配合 `RemovedComponents` 监听。
- **实施阶段**：P2（后续迭代优化）

#### 3.1 一次性重建卡顿 — ✅ 完全合理，采纳（最关键问题）

- **问题确认**：方案 §3 第251行已承认"整 SuperChunk 重建，约 25ms"，远超 16.67ms 帧预算。玩家放置/破坏方块时会感知到明显卡顿。
- **采纳方案**：实现**分帧合并机制**：
  1. 每帧合并固定数量 SubChunk（如 16 个），16 帧内完成整个 SuperChunk 重建
  2. 重建期间**保留旧网格**继续渲染，新网格完成后替换
  3. 重建过程中标记 SuperChunk 为 `Rebuilding` 状态，避免重复触发
- **实施阶段**：P0（编码前确定设计）

#### 3.2 材质同质化假设 — ✅ 完全合理，记录约束

- **分析**：当前代码使用单一 [`ChunkAtlasHandle`](src/chunk_dirty.rs:17) 纹理图集，Phase 1 假设成立。
- **采纳方案**：在方案中**明确记录此约束**——"Phase 1 限定 SuperChunk 内使用统一纹理图集和同一 Shader"，为未来多材质场景预留接口。
- **实施阶段**：P2（后续迭代）

#### 3.3 包围盒剔除精度 — ✅ 完全合理，接受精度损失

- **分析**：RENDER_DISTANCE=8 时 SuperChunk 通常大部分可见，影响有限。
- **采纳方案**：Phase 1 接受此精度损失；长期可考虑为合并网格生成更紧凑的包围体（如 OBB）或基于 SubChunk 的次级剔除。
- **实施阶段**：P2（后续迭代）

#### 3.4 法线正确性 — ✅ 正确提醒，无需修改

- **分析**：纯平移不影响法线方向，背面剔除正常工作。仅作备忘。
- **采纳方案**：无需修改方案，记录为已知约束。

---

### 修正后的优先级矩阵

> 对比社区原始优先级，标注调整及理由。

| 优先级 | 问题 | 社区原始 | 调整 | 理由 |
|-------|------|---------|------|------|
| **P0（编码前必解决）** | 刚性合并触发条件（1.1） | P0 ✅ | 不变 | 阻塞世界边缘渲染 |
| | SuperChunk 重建卡顿（3.1） | P0 ✅ | 不变 | 影响可玩性 |
| | 实体生命周期管理（1.3 + 2.3） | P1 ⬆️ | **升至 P0** | 悬垂引用导致崩溃，编码前必须确定设计 |
| **P1（实现阶段改进）** | `subchunk_coords` 分配（2.1） | P1 ✅ | 不变 | 非热路径，实现时优化 |
| | 所有权定义（1.2） | P2 ⬆️ | **升至 P1** | 模糊所有权会导致实现阶段反复修改 |
| **P2（后续迭代关注）** | 清理函数借用（2.4） | P1 ⬇️ | **降至 P2** | 现有代码已用相同模式且正常工作 |
| | 索引累加溢出（2.2） | — | P2 | 风险极低，添加 debug_assert 即可 |
| | 材质同质化（3.2） | P2 ✅ | 不变 | 当前阶段可接受 |
| | 包围盒精度（3.3） | P2 ✅ | 不变 | Phase 1 可接受精度损失 |

---

### 编码前必须确定的三个设计决策

1. **分帧合并机制**（3.1）— 确定每帧合并的 SubChunk 数量、`Rebuilding` 状态定义、旧网格保留策略
2. **部分合并/超时回退**（1.1）— 确定超时时长、缺失 SubChunk 的渲染策略（透明/空网格）
3. **实体生命周期管理**（1.3 + 2.3）— 确定 `HashMap<SuperChunkCoord, Entity>` 存储方案、卸载顺序、`RemovedComponents` 清理路径