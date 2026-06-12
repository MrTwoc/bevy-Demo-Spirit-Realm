# 项目长期记忆

## 技术约定

### 系统调度
- chunk_manager 模块拆分为 4 个细粒度系统（collect_and_upload_meshes / process_pending_deletions / manage_chunk_load_state / spawn_entities_from_prepare），main.rs 中按 First / Update 分组调度
- 所有 chunk 系统使用 `.chain()` 确保顺序执行（共享 LoadedChunks ResMut）
- `chunk_loader_system` 已于 2026-06-12 完全删除（含 has_load_queue_items），代码库零残留引用

### 渲染
- 使用 Bevy Mesh 路径（路径 A），Indirect Draw 代码（路径 B）已于 2026-05-30 全部移除
- 路径 B 残留文件已删除：`src/voxel_render/`、`src/gpu_meshing.rs`、`voxel_indirect.wgsl`、`voxel_cull.wgsl`、`voxel_meshing.wgsl`
- SVO 剔除系统已重构为 Plan A：体素数据源从独立的 Section/SectionTracker 切换到 ChunkData（通过 `VoxelSource` trait 抽象）
- 删除模块：`terrain_bridge.rs`（重复地形生成）、`section.rs` / `section_tracker.rs`（独立 section 缓存）、`batch_renderer.rs`（空壳）、`render_distance.rs`（替代为 svo_sync.rs）
- 新增模块：`voxel_source.rs`（VoxelSource trait + ChunkVoxelSource 适配器）、`svo_sync.rs`（SVO 同步系统）
- `NodeManager` 所有体素查询已改为 `&dyn VoxelSource` 参数，为 Plan B（GPU buffer 后端）预留扩展点
- SVO 八叉树构建时从 LoadedChunks.entries 读取 ChunkData，不再独立生成地形
- SVO 模块清单（8 个文件）：node_store, node_manager, voxel_source, svo_sync, gpu_traversal, visibility_bridge, hierarchical_bitset, mod
- 固体方块使用 Opaque 材质，水方块使用 Blend 材质
- 共享空 Mesh（SharedEmptyMesh）用于零几何体/空气区块

### LOD
- 顶点归一化：`face_quad_lod` 坐标除以 `step_f`，模型空间保持 1x1（2026-05-31）
- 世界空间放大由 `Transform::scale(Vec3::splat(step_f))` 通过 GPU 矩阵完成
- LOD 切换时在 `manage_chunk_load_state` 同步更新 Transform.scale
- 水方块在 LOD1+ 合并到固体 Mesh（`generate_lod_mesh_separated` 返回 `None`）

### 网格生成优化
- `generate_solid_mesh`（async_mesh.rs）使用列扫描（Column Scanning）优化：对每个 (x,z) 列从顶部向下跳过连续空气/水块，减少 50-70% 的 chunk.get() 调用
- `is_skippable()` 内联函数统一判断空气(0)和水(5)的跳过条件

### Mesh 创建/销毁优化
- `collect_and_upload_meshes`（chunk_manager.rs）仅在 Mesh Handle 实际变化时调用 `commands.entity().insert()`，正常重建路径（get_mut 成功）零 Command 开销
- 提取 `build_bevy_mesh()` 辅助函数消除固体/水 Mesh 构建代码重复
- 修复 bug：`get_mut` 失败创建新 Handle 后必须写回 `entry.solid_mesh_handle`，否则下次重建用旧无效 Handle
- 水 Mesh 路径同样仅在 Handle 变化时 insert，旧代码每次都 insert
- 移除冗余的 `entry.solid_material_handle = shared_material.handle.clone()`（共享材质永不变化）

### 加载队列优化
- `rebuild_load_queue` 使用 `OnceLock<Vec<OffsetEntry>>` 预计算偏移量表，按 dx²+dz² 排序，懒初始化
- `QUEUE_BUILD_STEPS_PER_FRAME` 从 500 提高到 2000（每步仅为一次 HashMap 查询）
- `LoadQueueBuildState` 用 `offset_idx: usize` 替代 `dx/dz` 螺旋扫描状态

### LOD 优化
- `update_incremental` 改为仅在玩家跨越区块边界时触发全量 `update`，不再每帧增量滚动 200 区块
- `LodManager` 用 `last_player_chunk: Option<ChunkCoord>` 替代 `last_checked/chunks_per_frame`
- 3000 区块全量遍历 < 0.5ms（纯整数距离平方比较）

### 系统合并
- `submit_prepare_tasks` 合并到 `manage_chunk_load_state` 末尾（步骤 2.7），减少一次系统调度和 ResMut 获取
- main.rs 区块生命周期管道从 4 个系统减少到 3 个

### 代码质量
- 性能相关 PR 需包含量化收益分析
- 不执行 cargo check，由用户手动验证

### 地形生成
- 公式：`surface = base + (2^coarse - 1) * amplitude + detail * detail_amp`（指数+细节层）
- 核心函数：`compute_surface_height()` in chunk.rs，被 fill_terrain / get_surface_height 共享
- 粗轮廓：Fbm<Simplex> 5 octaves, freq=0.003, seed=12345
- 细节层：Fbm<Simplex> 3 octaves, freq=0.02, seed=12346
- 常量：TERRAIN_BASE_HEIGHT=96, TERRAIN_AMPLITUDE=180.0, TERRAIN_DETAIL_AMP=20.0, WATER_LEVEL=80
- 高度范围：约 [6, 296→256]，MAX_Y=256 截顶，高峰形成高原平台
