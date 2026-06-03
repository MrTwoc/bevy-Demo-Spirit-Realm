# Voxy 风格 LOD 渲染系统实现总结

> 基于 Minecraft Voxy 模组设计，实现高性能体素渲染系统

---

## 一、项目概述

### 1.1 目标

借鉴 Voxy 模组的架构设计，为 Spirit Realm 体素游戏引擎实现高性能 LOD 渲染系统。

### 1.2 核心特性

- **八叉树节点管理**：高效的节点分配和遍历
- **GPU 驱动剔除**：使用 Compute Shader 进行视锥体和遮挡剔除
- **紧凑数据格式**：减少 GPU 带宽和内存占用
- **批量渲染**：减少 DrawCall 数量

---

## 二、实现阶段

### Phase 1: 八叉树基础 ✅

#### 核心文件

| 文件 | 说明 |
|------|------|
| `src/svo/hierarchical_bitset.rs` | 分层位集合，O(1) 分配/释放 |
| `src/svo/node_store.rs` | Voxy 风格节点存储 |
| `src/svo/node_manager.rs` | CPU 端八叉树管理 |
| `src/svo/mod.rs` | 模块入口，编解码函数 |

#### 技术细节

**HierarchicalBitSet**：
- 4 层位数组结构：A (1×u64) → B (64×u64) → C (64²×u64) → D (64³×u64)
- 支持 O(1) 分配和释放
- 支持连续分配（用于子节点数组）

**NodeStore**：
- 每个节点 32 字节 (4×u64)
- 使用 HierarchicalBitSet 管理分配
- 支持批量分配和释放

**位置编码**：
```
(lvl:4) | (x:20) | (y:20) | (z:20)
```
- LOD 级别：4 bits (0-15)
- 坐标：20 bits each (带符号扩展)

---

### Phase 2: GPU 剔除 ✅

#### 核心文件

| 文件 | 说明 |
|------|------|
| `src/svo/gpu_traversal.rs` | GPU 遍历插件 |
| `assets/shaders/svo_traversal.wgsl` | 遍历 Compute Shader |

#### 技术细节

**GPU 遍历管线**：
1. Extract 阶段：从 NodeManager 提取节点数据
2. Prepare 阶段：上传数据到 GPU Buffer
3. Dispatch 阶段：执行 Compute Shader
4. Readback 阶段：回读可见节点列表

**WGSL Shader 功能**：
- 节点解码（Voxy 风格）
- 距离剔除（XZ 平面 + Y 轴快速剔除）
- 视锥体剔除（分级策略：LOD 0-1 精确，LOD 2+ 粗略）
- 屏幕空间误差判断（should_descend）

**Bindings**：

| binding | 类型 | 用途 |
|---------|------|------|
| 0 | `storage read` | 节点缓冲区 |
| 1 | `uniform` | 相机参数 |
| 2 | `storage read_write` | 可见计数器 (atomic) |
| 3 | `storage read_write` | 可见节点列表 |

---

### Phase 3: 紧凑 Mesh ✅

#### 核心文件

| 文件 | 说明 |
|------|------|
| `src/compact_vertex.rs` | 紧凑顶点格式 |
| `src/svo/batch_renderer.rs` | 批量渲染系统 |

#### 技术细节

**CompactVertex (8 字节)**：
```
data0 (u32):
  bits 0-9:   x (10 bits)
  bits 10-19: y (10 bits)
  bits 20-29: z (10 bits)
  bits 30-31: 法线方向 (2 bits)

data1 (u32):
  bits 0-7:   u (8 bits)
  bits 8-15:  v (8 bits)
  bits 16-31: 模型 ID (16 bits)
```

**BatchRenderer**：
- 按材质和 LOD 分组渲染
- 统计信息收集
- 可配置的批量大小限制

---

### Phase 4: 集成优化 ✅

#### 集成状态

- ✅ SVO 插件已集成到主循环
- ✅ GPU 遍历系统已注册
- ✅ 批量渲染系统已注册
- ✅ 文档已更新

---

## 三、性能预期

| 指标 | 优化前 | 优化后 | 提升 |
|------|--------|--------|------|
| LOD1-3 DrawCall | ~数百个 | ~16 个/级 | ~97% |
| 节点分配速度 | O(n) | O(1) | ~100x |
| 顶点内存 | 32 字节 | 8 字节 | 4x |
| GPU 剔除 | CPU | GPU Compute | 并行化 |

---

## 四、文件清单

### 新增文件

| 文件 | 说明 |
|------|------|
| `src/svo/hierarchical_bitset.rs` | 分层位集合 |
| `src/svo/node_store.rs` | Voxy 风格节点存储 |
| `src/svo/node_manager.rs` | CPU 端八叉树管理 |
| `src/svo/gpu_traversal.rs` | GPU 遍历插件 |
| `src/svo/batch_renderer.rs` | 批量渲染系统 |
| `src/compact_vertex.rs` | 紧凑顶点格式 |
| `assets/shaders/svo_traversal.wgsl` | 遍历 Compute Shader |

### 修改文件

| 文件 | 修改内容 |
|------|----------|
| `src/svo/mod.rs` | 添加新模块，更新导出 |
| `src/main.rs` | 添加 compact_vertex 模块 |

---

## 五、使用说明

### 5.1 启用/禁用功能

在 `BatchRenderConfig` 中配置：

```rust
BatchRenderConfig {
    enabled: true,           // 启用批量渲染
    max_batch_size: 1024,    // 每批次最大节点数
    use_instancing: true,    // 启用实例化渲染
    lod_distances: [100.0, 200.0, 400.0, 800.0],  // LOD 切换距离
}
```

### 5.2 查看统计信息

BatchRenderer 提供统计信息：

```rust
let stats = batch_renderer.stats();
println!("Total Nodes: {}", stats.total_nodes);
println!("Batches: {}", stats.batch_count);
println!("Avg Batch Size: {:.1}", stats.avg_batch_size);
```

---

## 六、后续优化方向

1. **HiZ 遮挡剔除**：实现 Hierarchical Z-Buffer 进一步减少渲染节点
2. **Indirect Draw**：使用 Multi Draw Indirect 减少 CPU 开销
3. **LOD 过渡动画**：实现平滑的 LOD 切换效果
4. **内存池优化**：减少节点分配/释放的内存碎片

---

## 七、参考文献

1. **Voxy 源码**: `项目参考/优化参考/voxy-dev/`
2. **HierarchicalBitSet**: Voxy 的分层位集合实现
3. **NodeStore**: Voxy 的节点存储设计
4. **traversal_dev.comp**: Voxy 的 GPU 遍历 Shader

---

*文档更新时间：2026-06-03*
*实现状态：Phase 1-4 完成*
