# 项目长期记忆

## 技术约定

### 系统调度
- chunk_manager 模块已拆分为 5 个细粒度系统，main.rs 中按 First / Update 分组调度
- 所有 chunk 系统使用 `.chain()` 确保顺序执行（共享 LoadedChunks ResMut）
- 旧 chunk_loader_system 已废弃，新代码请勿引用

### 渲染
- 使用 Bevy Mesh 路径（非 Indirect Draw）
- 固体方块使用 Opaque 材质，水方块使用 Blend 材质
- 共享空 Mesh（SharedEmptyMesh）用于零几何体/空气区块

### 代码质量
- 性能相关 PR 需包含量化收益分析
- 不执行 cargo check，由用户手动验证
