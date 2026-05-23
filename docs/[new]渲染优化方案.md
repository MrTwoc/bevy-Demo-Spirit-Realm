# 渲染优化方案：基于 Bevy 官方 Demo 借鉴指南

> 撰写日期：2025-07-16
> 参考文档：[Bevy官方Demo借鉴指南](./Bevy官方Demo借鉴指南.md)
> 项目现状分析基于主分支 AI-coder (commit a46f5f6)

---

## 1. 项目渲染现状

### 1.1 核心参数

| 参数 | 值 | 说明 |
|------|-----|------|
| 区块尺寸 | 32×32×32 | 每个区块 32,768 个体素 |
| Render Distance | 16 | 水平 33×33 = 1,089 列 |
| Y Load Radius | 2 | 垂直方向 5 层 |
| 区块实体数 | ≈ 5,445 | 1,089 × 5（最密情况）|
| 每区块三角数 | 200–800 | CPU Greedy Meshing 后 |
| 当前 Draw Call | **≥ 5,445** | 每区块 1 mesh → 1 draw call |

### 1.2 当前管线架构

```
CPU 线程池 (async_mesh.rs)
  │  生成 SubMeshData { positions, uvs, normals, indices }
  ▼
Bevy Asset Server
  │  mesh_handle = assets.add(Mesh::from(submesh))
  ▼
chunk_manager::spawn_chunk_entity()
  │  commands.spawn((Mesh3d(handle), MeshMaterial3d(material), Transform))
  ▼
Bevy 标准 PBR 渲染管线
  │  每 Entity → 1 Draw Call
  ▼
屏幕 (×5,445 次)
```

### 1.3 已有基础设施

| 模块 | 当前状态 | 可复用度 |
|------|---------|---------|
| `voxel_render/pipeline.rs` | 骨架（PipelineKey + 布局定义） | ⭐⭐⭐ 高 |
| `voxel_render/prepare.rs` | 骨架（PrepareResult + 系统注册） | ⭐⭐ 中 |
| `voxel_render/queue.rs` | 骨架（DrawVoxel 命令） | ⭐⭐ 中 |
| `voxel_render/extract.rs` | 骨架 | ⭐⭐ 中 |
| `voxel_render/buffers.rs` | 完整（BufferAllocator + VoxelBuffers + upload_chunk_mesh） | ⭐⭐⭐ 高 |
| `voxel_render/bridge.rs` | 骨架 | ⭐⭐ 中 |
| `assets/shaders/voxel_meshing.wgsl` | 完整 Compute Shader（310 行） | ⭐⭐⭐ 高 |
| `assets/shaders/voxel_cull.wgsl` | 完整 Compute Shader（123 行） | ⭐⭐⭐ 高 |
| `assets/shaders/voxel.wgsl` | Fragment Shader（texture_2d_array） | ⭐⭐⭐ 高 |

---

## 2. 三路径方案

### 路径一：自动实例化

> **参考 Bevy Demo：** `shader/automatic_instancing.rs`（指南 §4）
> **难度：** ★★☆☆☆（中低）
> **工期：** 3–5 天

#### 原理

所有区块共享相同的 `Mesh3d` Handle + `MeshMaterial3d` Handle，Bevy 渲染管线自动将它们合批到一次 Instanced Draw Call 中。

每个实体通过 `MeshTag(instance_index)` 标记自己的索引，Vertex Shader 通过 `@builtin(instance_index)` 读取该区块的体素数据并计算顶点位置。

#### 改动清单

| 文件 | 改动说明 |
|------|---------|
| `chunk_manager.rs` | `spawn_chunk_entity`: 改为使用共享的 unit_mesh Handle 和 shared_material Handle |
| `new SharedVoxelMaterial` | 添加 `#[derive(AsBindGroup)]`，包含 `chunk_data: Handle<Image>` 或 Storage Buffer 引用 |
| `自定义 Vertex Shader` | 在 WGSL 中读取 `instance_index`，从 Storage Buffer 中获取区块数据 |
| `Cargo.toml` | 无需新增依赖 |

#### 架构

```
// CPU 侧：所有区块共享 1 个 Mesh Handle
let unit_mesh = meshes.add(Cuboid::from_size(Vec3::splat(0.01))); // 极小 mesh
let shared_material = materials.add(SharedVoxelMaterial { .. });

for (i, coord) in visible_chunks.iter().enumerate() {
    commands.spawn((
        Mesh3d(unit_mesh.clone()),
        MeshMaterial3d(shared_material.clone()),
        ChunkIndex(i as u32),       // 实例索引
        Transform::from_translation(chunk_center_world_pos(coord)),
    ));
}
```

#### Vertex Shader 关键逻辑

```wgsl
@vertex
fn vertex(@builtin(instance_index) instance: u32, ...) -> VertexOutput {
    let chunk_data = chunk_storage_buffer[instance];
    let world_pos = chunk_data.offset + local_pos; // local_pos 来自 unit_cube
    // 从体素数据计算最终顶点位置…
}

// Bevy 的自动合批会在底层将相同 (mesh, material) handle 的实体
// 合并为一次 DrawIndexedInstanced(instance_count = N)
```

#### 收益

- Draw Call: 5,445 → **~500–1,000**（受 GPU 实例批次上限约束）
- 实现代价：小，不改渲染管线
- 局限：Bevy 自动合批不可控，批次上限 ~64–256 实例/批次

---

### 路径二：自定义实例化渲染（MDI/Instanced）

> **参考 Bevy Demo：** `shader_advanced/custom_shader_instancing.rs`（指南 §5）
> **难度：** ★★★★☆（中高）
> **工期：** 1–2 周

#### 原理

绕过 Bevy 自动合批，通过 `SpecializedMeshPipeline` 实现**完全自定义**的渲染管线。所有区块的变换/数据写入一个 `StorageBuffer<ChunkInstance>`，一次 `DrawIndexedInstanced` 调用完成全部渲染。

#### 改动清单

| 文件 | 改动说明 |
|------|---------|
| `voxel_render/pipeline.rs` | 实现 `SpecializedMeshPipeline`，编译自定义 Vertex + Fragment Shader |
| `voxel_render/prepare.rs` | 构建 `ChunkInstanceBuffer`（每个区块 1 条），写入 GPU |
| `voxel_render/queue.rs` | 实现 `DrawChunkInstanced` RenderCommand，发出 1 次 draw call |
| `voxel_render/extract.rs` | 从 MainWorld 提取可见区块列表 → RenderWorld |
| `voxel_render/bridge.rs` | 注册 Render Graph Node，串联 prepare → queue → draw |
| `voxel_render/buffers.rs` | 对接 mega vertex/index buffer（目前已实现 `upload_chunk_mesh`）|
| `assets/shaders/render_chunk.wgsl` | **新建**：自定义 Vertex Shader + Fragment Shader |
| `chunk_manager.rs` | 不再 spawn Mesh3d/MeshMaterial3d，改为更新 RenderWorld 数据 |

#### 架构

```
┌─ CPU (每帧 extract) ───────────────────────────┐
│                                                  │
│  // 从 LoadedChunks 收集所有区块的 GPU buffer 偏移 │
│  for entry in visible_entries {                  │
│      instance_data.push(ChunkInstance {          │
│          vertex_offset: entry.vertex_offset,     │
│          index_offset: entry.index_offset,       │
│          index_count: entry.index_count,         │
│          world_offset: chunk_center,             │
│      });                                         │
│  }                                               │
│  instance_buffer.write(&instance_data);           │
└────────────────────┬────────────────────────────┘
                     ▼
┌─ GPU RenderPass ────────────────────────────────┐
│                                                  │
│  // 自定义 Vertex Shader                         │
│  // 读取 instance_buffer[instance_index]         │
│  // 从 mega vertex buffer 中获取顶点              │
│  // 输出到 Fragment Shader                       │
│                                                  │
│  pass.draw_indexed(                              │
│      index_buffer,                              │
│      0..total_indices,                          │
│      0..instance_count                          │
│  );  // ← 1 次调用                              │
└──────────────────────────────────────────────────┘
```

#### Queue 系统关键代码（参考指南 §5.2）

```rust
// 自定义 RenderCommand — 只需要发 1 次 draw call
#[derive(Clone, Copy)]
pub struct DrawChunkInstanced;

impl<P: PhaseItem> RenderCommand<P> for DrawChunkInstanced {
    fn render<'w>(
        _item: &P,
        _view: Entity,
        entity_query: Option<ROQueryItem<'w, Self::Param>>,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let (vertex_slice, index_slice, instance_count) = entity_query.unwrap();
        pass.set_vertex_buffer(0, vertex_slice.buffer, vertex_slice.offset);
        pass.set_index_buffer(index_slice.buffer, index_slice.offset, IndexFormat::Uint32);
        pass.draw_indexed(0..index_slice.count, 0..instance_count);
        RenderCommandResult::Success
    }
}
```

#### 收益

- Draw Call: 5,445 → **1 次**
- CPU 渲染命令构建：O(n) → O(1)
- 为 Phase 3 打基础：Render Graph Node 已接通，后续只需替换数据来源

---

### 路径三：完全 GPU 管线（Compute Meshing + GPU Cull + Instancing）

> **参考 Bevy Demo：** `compute_shader_game_of_life.rs`（指南 §1）+ custom shader instancing（指南 §5）
> **难度：** ★★★★★（高）
> **工期：** 2–4 周

#### 原理

网格生成和视锥体剔除**完全移到 GPU**，CPU 仅维护体素数据。每帧管线：Dispatch Meshing CS → Dispatch Culling CS → 1× DrawIndexedInstanced。

#### 改动清单（在路径二基础上追加）

| 文件 | 改动说明 |
|------|---------|
| `voxel_render/meshing_node.rs` | **新建**：Render Graph Compute Node，dispatch voxel_meshing.wgsl |
| `voxel_render/culling_node.rs` | **新建**：Render Graph Compute Node，dispatch voxel_cull.wgsl |
| `voxel_render/pipeline.rs` | 追加 Compute Pipeline 创建 |
| `voxel_render/prepare.rs` | 将体素数据（`Arc<ChunkData>`）复制到 GPU StorageBuffer |
| `voxel_render/buffers.rs` | 双缓冲（Ping-Pong）mega buffer，防止读写冲突 |
| `chunk_manager.rs` | 移除 CPU 端 `generate_chunk_mesh_async` 调用 |
| `Cargo.toml` | 无需新增依赖 |

#### 每帧 GPU 管线

```
帧 N:
  ┌──────────────────────────────────────────────────────────┐
  │  Step 1: Dispatch VoxelMeshing Compute Shader            │
  │    workgroup_count = chunk_count (≈5,445)                │
  │    workgroup_size = 64                                   │
  │    输入: chunk_voxel_data[] (StorageBuffer<u8>)          │
  │    输出: vertex_buffer_A, index_buffer_A (GPU only)      │
  │                                                          │
  │  Step 2: Dispatch Frustum Culling Compute Shader         │
  │    workgroup_count = chunk_count                         │
  │    输入: chunk_metadata[], camera frustum uniform        │
  │    输出: indirect_commands[] (visible list, compacted)   │
  │    atomic counter: visible_count                         │
  │                                                          │
  │  Step 3: DrawIndexedIndirect                             │
  │    从 indirect_commands 读取绘制参数                      │
  │    1 次间接绘制调用 = 所有可见区块                        │
  │    使用 vertex_buffer_A, index_buffer_A                  │
  └──────────────────────────────────────────────────────────┘

帧 N+1:
  ┌─ Step 1-3 同上，但写入 vertex_buffer_B, index_buffer_B ─┐
  │  Step 3 使用 vertex_buffer_A, index_buffer_A（读取上一帧）│
  └──────────────────────────────────────────────────────────┘
```

#### Render Graph 节点链（指南 §1.4）

```rust
use bevy::render::render_graph::{RenderGraph, RenderGraphApp, RenderLabel, ViewNodeRunner};

#[derive(Debug, Hash, PartialEq, Eq, Clone, RenderLabel)]
struct VoxelComputePass;

pub struct VoxelMeshingNode {
    pipeline: CachedComputePipelineId,
}

impl ComputeNode for VoxelMeshingNode {
    fn run(&self, _graph: &mut RenderGraphContext, render_context: &mut RenderContext, world: &World) -> Result<(), NodeRunError> {
        let pipeline_cache = world.resource::<PipelineCache>();
        let pipeline = pipeline_cache.get_compute_pipeline(self.pipeline).unwrap();
        let mut pass = render_context.begin_compute_pass();
        pass.set_pipeline(pipeline);
        // 绑定 chunk_data buffer → dispatch
        pass.dispatch_workgroups(chunk_count, 1, 1);
        Ok(())
    }
}

// 注册到渲染图
app.render_graph_mut()
    .add_node("voxel_meshing", VoxelMeshingNode::new(...))
    .add_node_edge("voxel_meshing", bevy::render::graph::CameraDriverLabel);
```

#### 已有 Shader 代码映射

| Shader 文件 | 状态 | 在管线中的角色 |
|-------------|------|---------------|
| `voxel_meshing.wgsl` | ✅ 完整 | Compute 阶段：体素 → 顶点/索引 |
| `voxel_cull.wgsl` | ✅ 完整 | Compute 阶段：视锥体剔除 |
| `voxel.wgsl` | ✅ 完整 | Fragment 阶段：texture_2d_array 采样 + PBR |

**接入 `voxel_meshing.wgsl` 时需补充**（目前仅缺）：

1. CPU 端将体素数据写入 StorageBuffer，通过 ExtractResource 传到 RenderWorld
2. Compute Pipeline 的 BindGroup 布局（`voxel_meshing.wgsl` 中已声明 group/binding）
3. Render Graph Node 的 dispatch 调用

#### 双缓冲策略（指南 §1.5）

```
帧 N:  写入 Buffer A (compute meshing), 读取 Buffer B (rendering)
帧 N+1: 写入 Buffer B, 读取 Buffer A
```

避免 GPU 读写同一 buffer 导致的 pipeline stall。

#### 收益

- Draw Call: **1 次**
- CPU 网格生成: **零**
- CPU 剔除: **零**
- GPU 额外开销: ~1–2ms/帧（取决于区块数）
- 内存: 保留 2 份 mega buffer（Ping-Pong）

---

## 3. 三条路径对比总表

| 维度 | 路径一：自动实例化 | 路径二：自定义实例化 | 路径三：完全 GPU |
|------|------------------|-------------------|----------------|
| **参考 Demo** | §4 automatic_instancing | §5 custom_shader_instancing | §1 compute_shader + §5 |
| **Draw Call** | ~500–1,000 | **1** | **1** |
| **CPU Meshing** | ✅ 保留 | ✅ 保留 | ❌ 完全移除 |
| **CPU 剔除** | ✅ 保留 | ✅ 保留 | ❌ 完全移除（GPU Cull）|
| **GPU 负载增加** | 低 | 中（vertex shader 变重）| 中高（~1-2ms compute）|
| **实现难度** | ★★☆☆☆ | ★★★★☆ | ★★★★★ |
| **工期** | 3–5 天 | 1–2 周 | 2–4 周 |
| **改 Rust 行数** | ~50 | ~500 | ~800 |
| **新增 WGSL** | ~30 行 vertex shader | ~80 行 vertex shader | 复用已有 shader |
| **风险** | Bevy 版本合批行为 | 自定义管线与 Bevy 更新兼容 | Compute Shader 调试困难 |
| **可增量演进** | → 路径二需重构 | → 路径三可直接追加 | 终极形态 |

---

## 4. 推荐迁移路线

### Phase 0 → Phase 1（立即可以做）

```
[当前]  5,445 次 Draw Call（Bevy 标准 PBR 管线）
   │
   ▼
[Phase 1]  1 次 Draw Call（自定义 Instancing 管线）
   │
   ├── 实现 pipeline.rs: SpecializedMeshPipeline
   ├── 实现 queue.rs: DrawChunkInstanced RenderCommand
   ├── 实现 prepare.rs: ChunkInstanceBuffer 构建
   ├── 接入 Render Graph（bridge.rs）
   ├── 写自定义 Vertex Shader（读取 mega buffer + instance data）
   └── Fragment Shader 复用现有 voxel.wgsl
   │
   ├─ 收益：Draw Call 降到 1，帧率大幅提升
   ├─ 保留：CPU 异步网格生成（async_mesh.rs 不动）
   └─ 前提：项目已有 voxel_render 骨架，接入成本已降低
```

### Phase 1 → Phase 2（Compute Meshing）

```
[Phase 1]  CPU 生成 mesh → GPU Instancing
   │
   ▼
[Phase 2]  GPU 生成 mesh + GPU Instancing
   │
   ├── 接入 voxel_meshing.wgsl compute pipeline
   ├── CPU 体素数据 → StorageBuffer（ExtractResource）
   ├── 创建 VoxelMeshingNode（ComputeNode）
   ├── 输出写入 mega vertex/index buffer
   └── 移除 CPU 端 generate_chunk_mesh_async 调用
   │
   ├─ 收益：CPU 网格生成消除
   └─ 前提：Phase 1 已完成，Render Graph 已接入
```

### Phase 2 → Phase 3（GPU Culling）

```
[Phase 2]  GPU Meshing + GPU Instancing
   │
   ▼
[Phase 3]  GPU Meshing + GPU Culling + GPU Instancing
   │
   ├── 接入 voxel_cull.wgsl compute pipeline
   ├── 提取相机 Frustum 数据（uniform）
   ├── 创建 VoxelCullingNode
   ├── 间接绘制（DrawIndexedIndirect）
   └── 完全移除 CPU 剔除逻辑
   │
   ├─ 收益：CPU 零参与渲染
   └─ 前提：Phase 2 已完成
```

---

## 5. 与已有基础设施的对应关系

### voxel_render/pipeline.rs 当前骨架 → Phase 1 需补充

```rust
// 当前内容（已存在但为空）
impl SpecializedMeshPipeline for VoxelRenderPipeline {
    fn specialize(...) -> Result<...> {
        // TODO: 配置 VertexBufferLayout → 对应 ChunkVertex 结构
        // TODO: 配置 Shader Defs → 条件编译
        // TODO: 返回 SpecializedMeshPipelineData
    }
}
```

### voxel_render/buffers.rs 当前完整实现 → 所有 Phase 可直接使用

- `BufferAllocator`: 分区分配 mega buffer 空间
- `VoxelBuffers`: 管理 vertex/index/instance 三个 buffer
- `upload_chunk_mesh`: 将 CPU mesh 数据写入 mega buffer

### assets/shaders/voxel_meshing.wgsl + voxel_cull.wgsl → Phase 2-3 直接使用

两个 shader 已完成：
- 声明了所有需要的 group/binding 布局
- 实现了 meshing / culling 算法
- 只需 CPU 端接入 bind group 和 dispatch

---

## 6. Bevy v0.18 API 兼容性

所有参考 Demo 在 Bevy v0.18 下均兼容：

| API | v0.18 状态 | 用于 |
|-----|-----------|------|
| `RenderGraph::add_node` | ✅ | 接入 Compute Node / Render Node |
| `SpecializedMeshPipeline` | ✅ | 自定义渲染管线 |
| `ComputeNode` trait | ✅ | Compute Shader dispatch |
| `ExtractResource` derive | ✅ | MainWorld → RenderWorld |
| `ShaderType` derive | ✅ | Instance buffer 定义 |
| `AsBindGroup` + `#[storage]` | ✅ | Storage buffer 绑定 |
| `RawBufferVec<T>` | ✅ | 快速 GPU buffer 写入 |
| `binding_array<texture_2d>` | ✅ | 纹理数组绑定 |

---

## 7. 注意事项

1. **视锥体剔除的时机**：需要在相机渲染前执行，必须集成到 RenderGraph（指南 §1.4）
2. **Storage Buffer 大小**：每区块 32³ = 32,768 体素 × 5,445 区块 ≈ 178 MB，需规划 buffer 大小
3. **实例数量限制**：GPU 有最大实例数量限制，高版本 Vulkan 支持 2^24，无需担心，但需确认驱动支持
4. **数据布局**：`#[repr(C)]` 确保 Rust 和 GPU 数据结构对齐，所有跨 CPU-GPU 的结构体必须标注
5. **纹理数组**：当前 `voxel.wgsl` 已使用 `texture_2d_array`，Phase 1 的 Fragment Shader 可以直接复用
6. **PBR 光照**：Phase 1 的自定义管线需要自己处理光照，建议从 `voxel.wgsl` 中提取 PBR 代码，保留 `apply_pbr_lighting` 调用 
7. **双缓冲**：Phase 3 必须实现双缓冲（指南 §1.5），避免 Compute 写 + Render 读同一 buffer 导致 pipeline stall
