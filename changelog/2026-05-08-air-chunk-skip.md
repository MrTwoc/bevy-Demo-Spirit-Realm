# 分支改动：全空气区块跳过优化

> **分支**：`air-chunk-skip`
> **日期**：2026-05-08
> **状态**：✅ 已完成，编译通过

---

## 1. 背景与动机

### 1.1 性能日志分析

基于 `perf_logs/2026-05-08_13-28-21-perf_log.csv` 的分析：

| 指标 | 稳态值 | 说明 |
|------|--------|------|
| 区块数 | ~2200-2300 | 稳定后 |
| CPU 三角面 | ~870-910 万 | 稳定后 |
| FPS | ~200-280 | 稳态表现良好 |
| 帧时间 | ~3.5-5.2ms | 稳态表现良好 |

### 1.2 问题发现

视距 `RENDER_DISTANCE=8`，Y 加载半径 `Y_LOAD_RADIUS=4`，理论最大区块数：

```
XZ 平面：(2×8+1)² = 17² = 289 个区块列（圆形裁剪后约 201 个）
Y 方向：2×4+1 = 9 层
理论最大：289 × 9 = 2601 个区块
```

但地形高度范围仅为 **-16 ~ +48**（世界 Y 坐标），对应 Y 区块层：

| Y 区块 | 世界 Y 范围 | 内容 | 是否需要加载 |
|--------|------------|------|-------------|
| Y = -4 | -128 ~ -96 | 全空气 | ❌ 不需要 |
| Y = -3 | -96 ~ -64 | 全空气 | ❌ 不需要 |
| Y = -2 | -64 ~ -32 | 全空气 | ❌ 不需要 |
| Y = -1 | -32 ~ 0 | 部分石头 | ✅ 需要 |
| Y = 0 | 0 ~ 32 | 地表+泥土+石头 | ✅ 需要 |
| Y = 1 | 32 ~ 64 | 部分山顶 | ✅ 需要 |
| Y = 2 | 64 ~ 96 | 全空气 | ❌ 不需要 |
| Y = 3 | 96 ~ 128 | 全空气 | ❌ 不需要 |
| Y = 4 | 128 ~ 160 | 全空气 | ❌ 不需要 |

**9 层中有 6 层是全空气的**，浪费率约 **67%**。

---

## 2. 改动内容

### 2.1 文件：`src/chunk_manager.rs`

**改动 1：添加导入**（第 22 行）

```diff
- use crate::chunk_dirty::{ChunkAtlasHandle, ChunkCoordComponent, ChunkMeshHandle};
+ use crate::chunk_dirty::{ChunkAtlasHandle, ChunkCoordComponent, ChunkMeshHandle, is_air_chunk};
```

**改动 2：在 `chunk_loader_system` 中跳过全空气区块**（第 310-312 行）

在 `fill_terrain()` 之后、创建实体之前，添加全空气区块检测：

```diff
  // 生成地形数据（轻量操作，保留在主线程）
  let mut chunk = Chunk::filled(0);
  fill_terrain(&mut chunk, &coord);

+ // 跳过全空气区块（高于地形或低于地形的区块），不创建实体和提交任务
+ if is_air_chunk(&chunk) {
+     continue;
+ }

  // 收集邻居数据用于跨区块面剔除
  let neighbors = collect_neighbors(coord, &*loaded);
```

**效果**：全空气区块不再创建 ECS 实体、占位 Mesh/Material、异步任务。

### 2.2 文件：`src/block_interaction.rs`

**改动 1：添加导入**（第 8-13 行）

```diff
+ use bevy::asset::RenderAssetUsages;
  use bevy::prelude::*;

  use crate::chunk::{BlockId, BlockPos, CHUNK_SIZE, ChunkCoord, ChunkData, world_to_chunk};
- use crate::chunk_dirty::{ChunkCoordComponent, mark_chunk_dirty};
- use crate::chunk_manager::LoadedChunks;
+ use crate::chunk_dirty::{ChunkAtlasHandle, ChunkCoordComponent, ChunkMeshHandle, DirtyChunk, mark_chunk_dirty};
+ use crate::chunk_manager::{AtlasTextureHandle, LoadedChunks};
  use crate::raycast::RayHitState;
```

**改动 2：`block_interaction_system` 增加参数**（第 78 行）

```diff
  pub fn block_interaction_system(
      mouse: Res<ButtonInput<MouseButton>>,
      keys: Res<ButtonInput<KeyCode>>,
      hit_state: Res<RayHitState>,
      mut chunk_query: Query<(Entity, &mut ChunkData, &Transform, &ChunkCoordComponent)>,
      mut commands: Commands,
      mut loaded: ResMut<LoadedChunks>,
      cursor_options: Single<&bevy::window::CursorOptions>,
+     atlas_handle: Res<AtlasTextureHandle>,
+     mut meshes: ResMut<Assets<Mesh>>,
+     mut materials: ResMut<Assets<StandardMaterial>>,
  ) {
```

**改动 3：`place_block` 增加按需创建逻辑**（第 158-260 行）

当目标区块不存在时（被全空气跳过优化跳过），按需创建该区块实体：

```diff
  fn place_block(
      block_pos: &BlockPos,
      normal: &IVec3,
      chunk_query: &mut Query<(Entity, &mut ChunkData, &Transform, &ChunkCoordComponent)>,
      commands: &mut Commands,
      loaded: &mut LoadedChunks,
+     atlas_handle: &AtlasTextureHandle,
+     meshes: &mut Assets<Mesh>,
+     materials: &mut Assets<StandardMaterial>,
  ) {
      // ... 计算 place_pos、coord、lx、ly、lz ...

+     // 查找目标区块实体
+     let mut found = false;
      for (entity, mut chunk_data, _transform, coord_comp) in chunk_query.iter_mut() {
          if coord_comp.0 != coord {
              continue;
          }
+         found = true;
          // ... 正常放置逻辑 ...
          break;
      }

+     // 目标区块不存在（被全空气跳过优化跳过），按需创建
+     if !found {
+         let mut chunk = ChunkData::filled(0);
+         chunk.set(lx, ly, lz, PLACE_BLOCK_ID);
+
+         let position = coord.to_world_origin();
+         let placeholder_mesh = meshes.add(Mesh::new(...));
+         let placeholder_mat = materials.add(StandardMaterial { ... });
+
+         let entity = commands.spawn((
+             chunk.clone(),
+             Transform::from_translation(position),
+             Visibility::default(),
+             ChunkAtlasHandle(atlas_handle.handle.clone()),
+             ChunkCoordComponent(coord),
+             Mesh3d(placeholder_mesh.clone()),
+             MeshMaterial3d(placeholder_mat.clone()),
+             ChunkMeshHandle { mesh: placeholder_mesh.clone(), material: placeholder_mat.clone() },
+             DirtyChunk,
+         )).id();
+
+         // 注册到 LoadedChunks
+         loaded.entries.insert(coord, ChunkEntry {
+             entity,
+             data: chunk,
+             last_accessed: loaded.frame_counter,
+             mesh_handle: placeholder_mesh,
+             material_handle: placeholder_mat,
+         });
+     }

      // 标记边界邻居为脏 ...
  }
```

---

## 3. 预期效果

| 指标 | 优化前 | 优化后 | 改善 |
|------|--------|--------|------|
| 区块数 | ~2200 | ~600-800 | **减少 60-70%** |
| ECS 实体数 | ~2200 | ~600-800 | **减少 60-70%** |
| 占位 Mesh/Material | ~2200 套 | ~600-800 套 | **减少 60-70%** |
| 异步任务数 | ~2200 个 | ~600-800 个 | **减少 60-70%** |
| Draw Call | ~2200 | ~600-800 | **减少 60-70%** |

---

## 4. 副作用处理

### 4.1 方块放置到未加载区块

**问题**：全空气区块被跳过后没有实体，玩家无法在其中放置方块。

**解决**：在 `place_block()` 中检测目标区块是否存在，不存在时按需创建：
- 创建全空气区块实体
- 设置放置的方块数据
- 注册到 `LoadedChunks`
- 标记 `DirtyChunk` 触发异步网格重建

### 4.2 邻居数据缺失

全空气区块不加载后，相邻区块的跨区块面剔除会将缺失邻居视为空气（保留边界面）。这是正确行为，不会导致视觉问题。

---

## 5. 编译状态

```
cargo check → ✅ 通过
新增警告：0
新增错误：0
```

---

## 6. 验证清单

- [ ] 运行程序，观察 `perf_log.csv` 中 `chunk_count` 是否从 ~2200 降至 ~600-800
- [ ] 地形视觉效果不受影响（全空气区块本来就没有可见内容）
- [ ] 在地表顶部向上叠方块，可以正常放置到全空气区块范围
- [ ] FPS 是否有提升（Draw Call 减少）
- [ ] 加载速度是否加快（异步任务减少）

---

## 7. 后续优化方向

| 优化方案 | 预期效果 | 优先级 |
|----------|---------|--------|
| Greedy Meshing | 顶点数减少 70-80% | P0 |
| LOD 系统 | GPU 负载降低 50-70% | P1 |
| SuperChunk 合批 | Draw Call 2000+ → ~4 | P1 |
| 预过滤 Y 范围（方案 3） | 避免生成地形数据后才跳过 | P2 |
| 全实心区块检测（方案 2） | 跳过地下完全被包围的区块 | P2 |
