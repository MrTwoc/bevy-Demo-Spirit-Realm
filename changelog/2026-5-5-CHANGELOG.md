# Spirit Realm Changelog

## [未发布] - 2025-XX-XX

### 新增

- **src/chunk_dirty.rs**（新建）
  - 实现脏标记 + 重建机制，为方块放置/破坏功能铺垫
  - `DirtyChunk` 组件：零数据标记组件，加在哪个实体上哪个实体就重建
  - `mark_chunk_dirty(commands, entity)`：公开 API，标记脏区块待重建
  - `is_air_chunk(chunk_data)`：辅助函数，判断区块是否全空气（Empty || Uniform(0)）
  - `rebuild_dirty_chunks` 系统：每帧检测 dirty 实体 → 重建 mesh → 清除标记
  - `set_block_dirty(commands, entity, chunk_data, pos, new_id)`：一次性原子操作（修改 + 标记脏）

### 修改

- **src/main.rs**
  - 新增 `mod chunk_dirty;`
  - `rebuild_dirty_chunks` 系统注册到 Update 阶段（同步执行）

### 已知限制

- 旧 mesh/material 未显式从 `Assets` 移除，会造成轻微 GPU 内存积累
- `mark_block_dirty`（在 chunk.rs 中）暂未与 `set_block_dirty` 联动
- `ChunkMeshHandle` 组件追踪旧 handle 暂未实现

### 待实现

- [ ] `ChunkMeshHandle` 组件：追踪旧 mesh handle，避免内存泄漏
- [ ] `mark_block_dirty` 与 `set_block_dirty` 联动（等方块放置/破坏功能）
- [ ] TextureAtlas + UV 纹理映射方案（Plan B）
- [ ] 方块放置/破坏交互

---

## [初始化] - 2025-05-05

- 项目初始化，基于 Bevy 0.18.1
- CHUNK_SIZE = 32，区块 32×32×32，地形 y=0~2
- BlockId：0=空气，1=草地，2=石头，3=泥土
- 摄像机初始位置：(16.0, 20.0, 16.0)
- 已拆分模块：main.rs、camera.rs、cube.rs、input.rs、chunk.rs、chunk_dirty.rs、chunk_wire_frame.rs
