# MultiDrawIndirect 渲染系统 - 实现状态报告

## 已完成的工作

### 1. 架构设计和模块结构 (Phase 2b.1) ✓

创建了新的 `voxel_render` 模块，包含以下子模块：

```
src/voxel_render/
├── mod.rs          # 模块入口，配置常量
├── plugin.rs       # VoxelRenderPlugin 核心插件
├── buffers.rs      # 缓冲区管理，数据结构定义
├── pipeline.rs     # 渲染管线，BindGroup Layout
├── extract.rs      # 数据提取（Main World → Render World）
├── prepare.rs      # 准备阶段，更新GPU缓冲区
├── queue.rs        # 排队阶段，加入渲染队列
├── draw.rs         # 绘制命令，MultiDrawIndirect调用
└── bridge.rs       # 桥接模块，连接现有系统
```

### 2. 核心数据结构 (Phase 2b.2) ✓

定义了以下关键数据结构：

- `IndirectCommand`: 间接绘制命令（对应 VkDrawIndexedIndirectCommand）
- `ChunkMeshData`: 区块Mesh数据（CPU端）
- `ChunkBufferRegion`: 区块在全局Buffer中的位置
- `VoxelBuffers`: 全局缓冲区资源
- `BufferAllocator`: 简单的Buffer分配器

### 3. 渲染管线定义 (Phase 2b.3) ✓

定义了渲染管线的 BindGroup Layout：

- Binding 0: 全局顶点缓冲区（Storage Buffer, 只读）
- Binding 1: 全局索引缓冲区（Storage Buffer, 只读）
- Binding 2: 区块偏移数组（Storage Buffer, 只读）
- Binding 3: 区块元数据（Storage Buffer, 只读）

### 4. WGSL 着色器 (Phase 2b.4) ✓

创建了新的着色器 `assets/shaders/voxel_indirect.wgsl`：

- 顶点着色器从 Storage Buffer 读取顶点数据
- 支持实例化渲染（每个区块作为独立实例）
- 包含简单的 Lambertian 光照计算
- 支持 Texture Array 纹理采样
- 包含 LOD 调试着色模式

### 5. 桥接系统 (Phase 2b.5) ✓

创建了 `bridge.rs` 模块，连接现有异步网格系统和新渲染系统：

- `render_bridge_system`: 收集异步网格结果并转换格式
- `dirty_chunk_bridge_system`: 处理脏块标记
- `chunk_unload_bridge_system`: 处理区块卸载

### 6. 依赖更新 ✓

在 Cargo.toml 中添加了 `bytemuck` 依赖，用于数据打包。

---

## 待完成的工作

### Phase 2b.7: CPU端视锥体剔除和参数生成

需要实现：

1. **视锥体剔除系统**
   - 从相机提取视锥体平面
   - 对每个区块执行 AABB 视锥体测试
   - 生成可见区块列表

2. **参数生成系统**
   - 将可见区块的Mesh数据打包到全局Buffer
   - 生成 IndirectCommand 数组
   - 上传到 GPU Buffer

### Phase 2b.8: 集成测试和调试

需要实现：

1. **Buffer 创建和上传**
   - 在 RenderDevice 上创建 Buffer
   - 实现数据上传逻辑
   - 处理 Buffer 更新

2. **BindGroup 创建**
   - 将 Buffer 绑定到 BindGroup
   - 处理 BindGroup 更新

3. **渲染命令集成**
   - 实现 DrawVoxelChunks 渲染命令
   - 集成到 Bevy 的渲染队列
   - 处理渲染顺序

---

## 技术挑战和风险

### 1. Bevy 渲染管线集成

Bevy 的渲染管线非常复杂，需要深入理解：
- Render Graph 系统
- PhaseItem 和 RenderCommand
- Extract-Prepare-Queue-Draw 流程

### 2. Buffer 管理

需要处理：
- 大缓冲区分配（数十MB）
- 动态更新（脏区块）
- 内存碎片化

### 3. 着色器兼容性

需要确保：
- 着色器与 Bevy 的 View 绑定兼容
- 支持 Bevy 的反向Z深度缓冲
- 支持 Bevy 的色调映射

---

## 下一步计划

### 短期（1-2天）

1. 实现 CPU 端视锥体剔除
2. 实现 Buffer 创建和上传
3. 实现 BindGroup 创建

### 中期（3-5天）

1. 完成渲染命令集成
2. 集成测试
3. 性能测试和优化

### 长期（1-2周）

1. 添加 GPU 端剔除（Phase 2c.1）
2. 添加 Hi-Z 遮挡剔除（Phase 2c.2）
3. 考虑 Mesh Shader 集成（Phase 2c.3）

---

## 代码统计

新增代码：
- `src/voxel_render/`: ~600 行
- `assets/shaders/voxel_indirect.wgsl`: ~200 行
- 总计：~800 行

修改代码：
- `src/main.rs`: ~10 行
- `Cargo.toml`: ~2 行

---

## 结论

Phase 2b 的基础框架已经完成，建立了 MultiDrawIndirect 渲染系统的骨架。

下一步是实现 CPU 端视锥体剔除和 Buffer 管理，这是使系统可运行的关键。

预计完成 Phase 2b.7 和 2b.8 后，可以获得初步的性能提升（Draw Call 从 5000+ 降低到 100-200）。

完全实现后，Draw Call 可以降低到 10-20，FPS 提升 3-6 倍。
