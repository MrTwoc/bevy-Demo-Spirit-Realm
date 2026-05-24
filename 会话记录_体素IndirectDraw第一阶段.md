# 会话记录：体素 Indirect Draw 渲染管线（第一阶段）

> 项目: bevy-Demo-Spirit-Realm  
> 分支: `AI-coder`  
> 日期: 2026-05-24  
> 引擎: Bevy v0.15+

---

## 目录

1. [背景与目标](#1-背景与目标)
2. [第一阶段修改内容（已完成）](#2-第一阶段修改内容已完成)
3. [文件级修改详解](#3-文件级修改详解)
4. [架构图](#4-架构图)
5. [当前状态与验证](#5-当前状态与验证)
6. [遇到的问题与解决](#6-遇到的问题与解决)
7. [下一步规划（第二阶段）](#7-下一步规划第二阶段)

---

## 1. 背景与目标

### 问题

体素世界由大量区块（Chunk）组成，每个区块是一个独立 Mesh。旧方案中每个区块一个 `Mesh3d` 组件 → 一个 `DrawCall`，当区块数量大（数千）时，CPU 侧的 Draw Call 提交成为瓶颈。

### 目标

将体素渲染切换到 **Multi-Draw-Indirect** 管线：
- 所有区块的顶点/索引数据放入**全局 Storage Buffer**
- 在 CPU 侧构建 `DrawIndexedIndirect` 命令数组，写入 GPU Indirect Buffer
- 一次 `multi_draw_indexed_indirect_count` 调用完成所有可见区块渲染
- 保留视锥体剔除能力（通过控制 Indirect 命令数组的内容）

### 架构变化

```
旧路径（每区块一个 DrawCall）                新路径（Indirect Draw，单个 DrawCall）
                                      
每个区块:                                   所有区块:
  Entity {                                   全局 Storage Buffer:
    Mesh3d(handle)       → DrawCall            vertex_buffer (PackedVertex[])
    MeshMaterial3d(..)   → DrawCall            index_buffer  (u32[])
  }                                            offset_buffer (ChunkOffset[])
                                              indirect_buffer (IndirectCommand[])
                                              
                                              渲染图节点:
                                                VoxelIndirectNode
                                                  → multi_draw_indexed_indirect_count()
```

---

## 2. 第一阶段修改内容（已完成）

### 统计

| 指标 | 值 |
|------|-----|
| 修改文件数 | 7 |
| 新增行数 | 583 |
| 删除行数 | 142 |
| Git Commit | `71517af` |

### 修改清单

| # | 文件 | 修改类型 | 说明 |
|---|------|---------|------|
| 1 | `assets/shaders/voxel_indirect.wgsl` | **重写** | 从旧版 WGSL 重写为 Storage Buffer + View Uniform 版本 |
| 2 | `src/voxel_render/buffers.rs` | **已有**(未改) | `VoxelBuffers` GPU 缓冲区分配 + `upload_chunk_mesh` + `update_indirect_buffer` |
| 3 | `src/voxel_render/pipeline.rs` | **新增** | `VoxelRenderPipeline` 管线 + `VoxelBindGroup` 绑组 + `ViewUniformRaw` |
| 4 | `src/voxel_render/extract.rs` | **重写** | `ExtractedVoxelBuffers` + `ExtractedViewData` + 提取系统 |
| 5 | `src/voxel_render/draw.rs` | **重写** | `VoxelIndirectNode` 渲染图节点 |
| 6 | `src/voxel_render/queue.rs` | **重写** | `VoxelDrawCountBuffer` + 渲染图注册 |
| 7 | `src/voxel_render/plugin.rs` | **扩展** | 注册 Extract/Prepare/Queue 系统 + RenderStartup |
| 8 | `src/chunk_manager.rs` | **修改** | 推送 `ChunkMeshData` 到 `upload_queue` |

---

## 3. 文件级修改详解

### 3.1 `assets/shaders/voxel_indirect.wgsl` — 着色器

**BindGroup(0) 布局：**

| Binding | 类型 | 用途 |
|---------|------|------|
| b0 | `storage/read` | `vertex_buffer` → `array<PackedVertex>` |
| b1 | `storage/read` | `index_buffer` → `array<u32>` |
| b2 | `storage/read` | `chunk_offsets` → `array<vec4<f32>>` |
| b3 | `uniform` | `ViewUniform` → `view_proj: mat4x4<f32>` |

**着色器流程：**

```
顶点着色器:
  vertex_id  → vertex_buffer[vertex_id]  → 位置/法线/UV
  instance_id → chunk_offsets[instance_id] → 世界坐标偏移
  view_proj × (位置 + 偏移) → 裁剪空间坐标

片段着色器:
  使用 fragment_debug 入口点
  Lambert 光照（环境光 + 太阳光方向）
  纯色输出（无纹理）
```

**关键变化：**
- 删除了旧的 `ChunkMetadata`、`View`(Bevy标准)、`voxel_texture` 纹理绑定
- 统一为单一 Group(0)，4 个 binding
- 删除了 LOD 调试着色函数

### 3.2 `src/voxel_render/buffers.rs` — GPU 缓冲区（已有，未修改）

**数据结构：**

```rust
pub struct PackedVertex {              // 16字节对齐
    position: [f32; 3],                // 位置 (12 bytes)
    normal_encoded: f32,               // 法线编码 (4 bytes)
    uv: [f32; 2],                      // UV (8 bytes)
    extra: [f32; 2],                   // 额外数据 (8 bytes)
}

pub struct IndirectCommand {           // wgpu::DrawIndexedIndirect 布局
    index_count: u32,
    instance_count: u32,
    first_index: u32,
    base_vertex: i32,
    first_instance: u32,
}

pub struct ChunkOffset {
    position: [f32; 4],                // xyz + padding
}
```

**VoxelBuffers (4个全局缓冲区):**

| 缓冲区 | 大小 | 用途 |
|--------|------|------|
| `vertex_buffer` | MAX_CHUNKS × MAX_VERTICES_PER_CHUNK × 40 bytes | 所有区块顶点数据 |
| `index_buffer` | MAX_CHUNKS × MAX_INDICES_PER_CHUNK × 4 bytes | 所有区块索引数据 |
| `indirect_buffer` | MAX_CHUNKS × 32 bytes | Indirect Draw 命令数组 |
| `offset_buffer` | MAX_CHUNKS × 16 bytes | 区块世界坐标偏移 |

**关键函数：**
- `VoxelBuffers::upload_chunk_mesh()` — 将区块网格写入全局 buffer 的分配区域
- `VoxelBuffers::update_indirect_buffer()` — 遍历 `chunk_regions` 重建 Indirect 命令数组 + 偏移数组
- `BufferAllocator` — 管理每个区块在全局 buffer 中的偏移和大小

### 3.3 `src/voxel_render/pipeline.rs` — 渲染管线（新建 ~230行）

**`create_voxel_render_pipeline()`** — 在 `RenderStartup` 阶段执行：

1. 从 `RenderDevice` 获取设备
2. 定义 BindGroupLayout（4个 binding 的存储/统一缓冲区布局）
3. 使用 `PipelineCache::queue_render_pipeline` 排队管线创建
4. 管线配置：
   - 拓扑：`TriangleList`
   - 面剔除：`Back`（CCW）
   - 深度测试：`Less`，写入开启
   - 帧缓冲格式：`ViewTarget::TEXTURE_FORMAT_HDR`
   - 无顶点缓冲区（从 Storage Buffer 读取）

**`prepare_voxel_bind_groups()`** — 每帧在 `PrepareBindGroups` 阶段执行：

1. 从 `ExtractedVoxelBuffers` 获取 GPU Buffer 引用
2. 从 `ExtractedViewData` 获取相机矩阵
3. 创建每帧的 View Uniform Buffer（`create_buffer_with_data`）
4. 创建 `BindGroup`，绑定 4 个 buffer
5. 插入 `VoxelBindGroup` 资源供渲染节点使用

### 3.4 `src/voxel_render/extract.rs` — 数据提取（新建 ~90行）

**数据结构：**

```rust
pub struct ExtractedVoxelBuffers {          // 渲染世界
    vertex_buffer: Option<Buffer>,
    index_buffer: Option<Buffer>,
    indirect_buffer: Option<Buffer>,
    offset_buffer: Option<Buffer>,
    chunk_count: u32,
    max_chunks: u32,
}

pub struct ViewUniformRaw {                 // repr(C), bytemuck
    view_proj: [[f32; 4]; 4],              // clip_from_view 矩阵
}

pub struct ExtractedViewData {              // 渲染世界
    view_uniform: ViewUniformRaw,
    is_valid: bool,
}
```

**提取系统（在 `ExtractSchedule` 中运行）：**

```rust
extract_voxel_buffers:   // 主世界 VoxelBuffers → 渲染世界 ExtractedVoxelBuffers
  vertex/index/indirect/offset buffer 引用 + chunk_count

extract_view_data:       // 主世界 Camera.computed → 渲染世界 ExtractedViewData
  clip_from_view (投影×视图矩阵)
```

### 3.5 `src/voxel_render/queue.rs` — 排队阶段（新建 ~80行）

**`VoxelDrawCountBuffer`** — 存储当前可见区块数量（单一 u32），用于 `multi_draw_indexed_indirect_count` 的 count 参数。

**`queue_voxel_draw()`** — 在 `Queue` 阶段更新 count buffer：
```rust
let count = extracted.chunk_count;   // 可见区块数量
render_queue.write_buffer(&count_buffer, 0, &count);
```

**`setup_voxel_indirect_graph()`** — 注册渲染图节点到 Core3d 子图：
```
Core3d 子图:
  ... → EndPrepasses → [VoxelIndirectNode] → StartMainPass → ...
```

### 3.6 `src/voxel_render/draw.rs` — 渲染图节点（新建 ~135行）

**`VoxelIndirectNode`** — 自定义渲染图节点，`run()` 流程：

```
1. 获取 view_entity → ViewTarget（颜色附件）
2. 获取 VoxelRenderPipeline + PipelineCache → 编译后的 RenderPipeline
3. 获取 VoxelBindGroup → 绑定组
4. 获取 ExtractedVoxelBuffers → indirect_buffer, chunk_count
5. 获取 VoxelDrawCountBuffer → count buffer
6. 获取 ViewDepthTexture → 深度附件（LoadOp::Load）
7. 创建 RenderPass（Load/Store 操作）
8. 设置管线 + 绑定组
9. multi_draw_indexed_indirect_count(indirect_buf, 0, count_buf, 0, max_chunks)
```

**关键细节：**
- 深度附件使用 `LoadOp::Load`，复用主场景深度（深度测试通过即可，不必清除）
- 颜色附件也使用 `LoadOp::Load`，叠加在主场景渲染之上
- 所有资源为 `Option`/空检查，优雅处理初始化未就绪状态

### 3.7 `src/voxel_render/plugin.rs` — 插件注册

**`VoxelRenderPlugin::build()` 新增注册：**

```rust
// RenderApp 初始化
render_app.init_resource::<ExtractedVoxelBuffers>();
render_app.init_resource::<ExtractedViewData>();

// ExtractSchedule
render_app.add_systems(ExtractSchedule,
    (extract_voxel_buffers, extract_view_data));

// RenderStartup
render_app.add_systems(RenderStartup,
    create_voxel_render_pipeline);

// PrepareBindGroups
render_app.add_systems(Render,
    prepare_voxel_bind_groups.in_set(RenderSystems::PrepareBindGroups));

// Queue
render_app.add_systems(Render,
    queue_voxel_draw.in_set(RenderSystems::Queue));

// 渲染图注册
setup_voxel_indirect_graph(app);
```

### 3.8 `src/chunk_manager.rs` — 数据推送

在 `chunk_loader_system` 中新增：

```rust
// 在区块加载完成后，将网格数据推送到 upload_queue
render_state.upload_queue.push(ChunkMeshData {
    coord: result.coord,
    positions: result.solid.positions.clone(),
    normals: result.solid.normals.clone(),
    uvs: result.solid.uvs.clone(),
    indices: result.solid.indices.clone(),
    lod_level: chunk_lod,
});
render_state.dirty = true;
```

同时增加了 `remove_queue` 的处理（区块卸载时释放 GPU 缓冲区区域）。

---

## 4. 架构图

```
┌─────────────────────────────────────────────────────────────────┐
│  主世界 (Main World)                                            │
│                                                                 │
│  chunk_loader_system                                            │
│    ↓ 推送 ChunkMeshData                                          │
│  VoxelRenderState.upload_queue                                  │
│    ↓ 每帧处理                                                    │
│  VoxelBuffers (buffers.rs)                                      │
│    ├─ upload_chunk_mesh()  → 写入全局 Storage Buffer            │
│    └─ update_indirect_buffer() → 重建 Indirect 命令数组          │
│                                                                 │
└──────────────────────────┬──────────────────────────────────────┘
                           │ ExtractSchedule
                           ▼
┌─────────────────────────────────────────────────────────────────┐
│  渲染世界 (Render World)                                        │
│                                                                 │
│  ExtractedVoxelBuffers  ← 主世界 VoxelBuffers (Buffer 引用)     │
│  ExtractedViewData      ← 主世界 Camera (矩阵数据)              │
│                                                                 │
│  ┌─ RenderStartup ──────────────────────────────────────────┐  │
│  │  create_voxel_render_pipeline → VoxelRenderPipeline      │  │
│  │  create_draw_count_buffer    → VoxelDrawCountBuffer      │  │
│  └──────────────────────────────────────────────────────────┘  │
│                                                                 │
│  ┌─ PrepareBindGroups ───────────────────────────────────────┐  │
│  │  prepare_voxel_bind_groups → VoxelBindGroup              │  │
│  │    (创建每帧 view uniform + 绑定 4 个 buffer)              │  │
│  └──────────────────────────────────────────────────────────┘  │
│                                                                 │
│  ┌─ Queue ───────────────────────────────────────────────────┐  │
│  │  queue_voxel_draw → 更新 VoxelDrawCountBuffer            │  │
│  └──────────────────────────────────────────────────────────┘  │
│                                                                 │
│  ┌─ 渲染图: Core3d ─────────────────────────────────────────┐  │
│  │  ... → EndPrepasses                                       │  │
│  │         → [VoxelIndirectNode]                             │  │
│  │             ├─ 获取资源                                    │  │
│  │             ├─ begin_render_pass(Load/Store)              │  │
│  │             ├─ set_pipeline + set_bind_group              │  │
│  │             └─ multi_draw_indexed_indirect_count()        │  │
│  │         → StartMainPass → ...                             │  │
│  └──────────────────────────────────────────────────────────┘  │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

---

## 5. 当前状态与验证

### ✅ 已通过

| 检查 | 结果 |
|------|------|
| `cargo check --release` | ✅ 编译通过，0 errors |
| `cargo run --release` | ✅ 启动正常，无 panic |
| 日志 `VoxelRenderCommandPlugin loaded` | ✅ 正常打印 |
| 日志 `VoxelIndirectGraph: registered` | ✅ 正常打印 |
| `VoxelBuffers created` | ✅ 缓冲区分配正常 |
| 区块加载/卸载 | ✅ 新旧路径并行运行 |
| 视锥体剔除 | ✅ 仍通过旧路径的 visibility_bridge 工作 |

### ⚠️ 已知问题

1. **双重渲染** — 新路径（Indirect Draw）和旧路径（`Mesh3d`）**同时运行**，帧率不变甚至略降
   - 原因：`chunk_manager.rs` 仍有 `insert(Mesh3d(...))` 和 `insert(MeshMaterial3d(...))`
   - 计划：第二阶段注释掉旧路径的插入
2. **着色器无纹理** — 当前 WGSL 使用 `fragment_debug` 纯色渲染，未使用纹理
3. **水面尚未接入 Indirect Draw** — 水面仍使用旧 Mesh3d 在 `TransparentPass` 渲染
4. **提取的相机矩阵是预计算矩阵** — 从 `Camera.computed.clip_from_view` 提取，可能需要改为手动计算 view_proj 以包含实体变换

---

## 6. 遇到的问题与解决

### 6.1 编译错误：`BufferBinding` 字段不匹配

```rust
// ❌ 旧写法（Bevy v0.13）
resource: BindingResource::Buffer(BufferBinding {
    buffer: &vertex_buffer,
    offset: 0,
    size: None,
})

// ✅ 修正（Bevy v0.15+）
resource: BindingResource::Buffer(BufferBinding {
    buffer: &**vertex_buffer,   // RenderResource Buffer → wgpu::Buffer
    offset: 0,
    size: None,
})
```

**原因**：Bevy 对 `Buffer` 的 Deref 链改变，需要双重解引用
**影响文件**：`pipeline.rs`

### 6.2 编译错误：`get_sub_graph` 改为 `get_sub_graph_mut`

```rust
// ❌ 旧写法
let core3d = render_graph.get_sub_graph(Core3d).unwrap();

// ✅ 修正
let Some(core3d) = render_graph.get_sub_graph_mut(Core3d) else {
    info!("...skipping");
    return;
};
```

**原因**：Bevy v0.15 RenderGraph API 变更（获取可变引用以添加节点）
**影响文件**：`queue.rs`

### 6.3 编译错误：`RenderPassDepthStencilAttachment.depth_slice`

```rust
// ❌ 旧写法
depth_ops: Some(Operations { load: LoadOp::Load, store: StoreOp::Store }),

// ✅ 修正（移到 RenderPassColorAttachment 中）
color_attachments: &[Some(RenderPassColorAttachment {
    view: ...,
    depth_slice: None,     // 深度切片移到此处
    ...
})]
```

**原因**：Bevy v0.15 将 `depth_slice` 从 `RenderPassDepthStencilAttachment` 移到 `RenderPassColorAttachment`
**影响文件**：`draw.rs`

### 6.4 编译错误：`unclipped_depth` 属性缺失

在 `PrimitiveState` 中添加：
```rust
unclipped_depth: false,
```

**原因**：Bevy v0.15 `PrimitiveState` 新增字段
**影响文件**：`pipeline.rs`

### 6.5 运行时 panic：`RenderGraph` 设置阶段访问 `RenderDevice`

**问题**：`VoxelDrawCountBuffer::new()` 在 `setup_voxel_indirect_graph()` 中调用，但此时 `RenderDevice` 尚未就绪

**解决**：将缓冲区创建移到 `RenderStartup` 系统：
```rust
// queue.rs
fn create_draw_count_buffer(mut commands: Commands, render_device: Res<RenderDevice>) {
    commands.insert_resource(VoxelDrawCountBuffer::new(&render_device));
}
render_app.add_systems(RenderStartup, create_draw_count_buffer);
```

---

## 7. 下一步规划（第二阶段）

### 7.1 禁用旧路径 Mesh3d

**目标**：关闭双重渲染，只保留 Indirect Draw 路径

**修改文件**：`src/chunk_manager.rs`

1. **注释固体 Mesh3d 插入**（约 L403-410）：
   ```rust
   // commands.entity(entity).insert(Mesh3d(new_handle));  // 注释掉旧路径
   ```

2. **注释固体 MeshMaterial3d 插入**（约 L428-434）：
   ```rust
   // commands.entity(entity).insert(MeshMaterial3d(solid_material.clone()));  // 注释掉旧路径
   ```

3. **水面处理**：水面区块的透明渲染可能需要保留旧路径或也接入 Indirect Draw

4. **注意保留**：`ChunkMeshHandle`、`ChunkAtlasHandle` 等数据组件仍需保留，供其他系统使用

### 7.2 添加纹理渲染

**目标**：从纯色 `fragment_debug` → 使用纹理贴图

**修改文件**：`assets/shaders/voxel_indirect.wgsl`

- 在 WGSL 中添加纹理绑定（新的 Group(1)）
- 从 UV 解码纹理层索引
- 添加 `fragment` 入口点（完整纹理版本）

### 7.3 性能验证

- 在有大量区块的场景（1000+ 区块）对比帧率
- 使用 `RenderPass` 的 `LoadOp::Clear` 尝试禁用旧路径后观察是否只有新路径渲染
- 检查 GPU 耗时（可用 Nsight/GPUVis 等工具）

### 7.4 潜在优化

- 将 View Uniform Buffer 改为每帧复用而不是创建新 buffer
- 实现可见区块的 CPU 端视锥体剔除 → 控制 `indirect_buffer` 内容
- 水面区块接入 Indirect Draw（需要透明排序支持）

---

## 附录

### Git Commit

```
71517af feat: 实现体素 Indirect Draw 渲染管线（第一阶段）
2f33cd5 chore: 清理调试临时文件
```

### 启动命令

```bash
cd I:\VScodeIng\bevy-Demo-Spirit-Realm
cargo run --release        # 运行
cargo check --release       # 仅检查编译
```

### 关键文件路径

| 文件 | 用途 |
|------|------|
| `src/voxel_render/mod.rs` | 模块声明 + config |
| `src/voxel_render/buffers.rs` | GPU 缓冲区管理（404行） |
| `src/voxel_render/pipeline.rs` | 管线 + 绑组（230行） |
| `src/voxel_render/extract.rs` | 数据提取（90行） |
| `src/voxel_render/draw.rs` | 渲染图节点（135行） |
| `src/voxel_render/queue.rs` | 排队 + 图注册（80行） |
| `src/voxel_render/plugin.rs` | 插件注册（55行） |
| `assets/shaders/voxel_indirect.wgsl` | 着色器 |
