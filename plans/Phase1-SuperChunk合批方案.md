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
