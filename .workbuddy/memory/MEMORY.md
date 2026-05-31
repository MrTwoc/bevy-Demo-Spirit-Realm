# 项目长期记忆

## 技术约定

### 系统调度
- chunk_manager 模块已拆分为 5 个细粒度系统，main.rs 中按 First / Update 分组调度
- 所有 chunk 系统使用 `.chain()` 确保顺序执行（共享 LoadedChunks ResMut）
- 旧 chunk_loader_system 已废弃，新代码请勿引用

### 渲染
- 使用 Bevy Mesh 路径（路径 A），Indirect Draw 代码（路径 B）已于 2026-05-30 全部移除
- 路径 B 残留文件已删除：`src/voxel_render/`、`src/gpu_meshing.rs`、`voxel_indirect.wgsl`、`voxel_cull.wgsl`、`voxel_meshing.wgsl`
- SVO 剔除系统（`gpu_traversal.rs`、`visibility_bridge.rs`）保留，驱动 Bevy PBR 渲染
- 固体方块使用 Opaque 材质，水方块使用 Blend 材质
- 共享空 Mesh（SharedEmptyMesh）用于零几何体/空气区块

### LOD
- 顶点归一化：`face_quad_lod` 坐标除以 `step_f`，模型空间保持 1x1（2026-05-31）
- 世界空间放大由 `Transform::scale(Vec3::splat(step_f))` 通过 GPU 矩阵完成
- LOD 切换时在 `manage_chunk_load_state` 同步更新 Transform.scale
- 水方块在 LOD1+ 合并到固体 Mesh（`generate_lod_mesh_separated` 返回 `None`）

### 代码质量
- 性能相关 PR 需包含量化收益分析
- 不执行 cargo check，由用户手动验证
