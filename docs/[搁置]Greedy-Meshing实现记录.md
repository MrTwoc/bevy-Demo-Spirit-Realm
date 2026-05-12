# Greedy Meshing 实现记录

> **日期**：2026-05-09
> **状态**：✅ 已实现，⚠️ 因 Texture Atlas UV 平铺问题暂未启用
> **模块**：[`src/greedy_mesh.rs`](../src/greedy_mesh.rs)

---

## 1. 背景与动机

### 1.1 当前网格生成瓶颈

项目使用逐面生成（Naive Meshing）方式，每个非空气方块的每个可见面都独立生成 4 个顶点 + 6 个索引：

| 指标 | 逐面生成 |
|------|---------|
| 顶点数/区块 | 4000-8000 |
| CPU 耗时/区块 | ~0.5-1.5ms |
| Draw Call | ~2200+（每区块一个实体）|

### 1.2 Greedy Meshing 原理

将相邻同材质方块面合并为更大的四边形，大幅减少顶点数：

| 指标 | 逐面生成 | Greedy Meshing |
|------|---------|----------------|
| 顶点数/区块 | 4000-8000 | 500-1500 |
| CPU 耗时/区块 | ~0.5-1.5ms | ~0.1-0.3ms |

---

## 2. 实现方案

### 2.1 模块结构

新建 [`src/greedy_mesh.rs`](../src/greedy_mesh.rs)，纯数据模块（不依赖 Bevy），可在主线程和工作线程安全使用。

```
greedy_mesh.rs (~310 行)
├── GreedyMeshResult 结构体              ~10 行
├── FaceConfig 面方向配置                 ~40 行
├── FACE_CONFIGS 常量（6 个面）           ~60 行
├── generate_greedy_mesh() 主函数        ~30 行  ← 对外接口
├── process_face() 单方向处理            ~50 行  ← 核心：构建2D掩码 + 贪心合并
├── build_mask() 掩码构建                ~50 行  ← 面可见性检查
└── emit_quad() 合并后四边形生成          ~70 行  ← 顶点/UV/法线/索引生成
```

### 2.2 核心算法

对每个面方向（+X, -X, +Y, -Y, +Z, -Z）：

1. **沿法线方向逐层扫描**（共 32 层）
2. **构建 2D 可见性掩码**（[`build_mask()`](../src/greedy_mesh.rs:267)）
   - 遍历 32×32 的掩码格子
   - 将 `(layer, u, v)` 映射到区块局部坐标 `(x, y, z)`
   - 检查该位置的方块面是否需要渲染（邻居遮挡检查）
   - `mask[v][u] = Some(block_id)` 表示可见，`None` 表示被遮挡
3. **贪心合并**（[`process_face()`](../src/greedy_mesh.rs:196)）
   - 扫描掩码，找到第一个未消费的可见格子
   - 沿 u 方向延伸宽度 W（连续同材质）
   - 沿 v 方向延伸高度 H（整行匹配）
   - 标记已消费区域，生成一个 W×H 的大四边形
4. **生成顶点数据**（[`emit_quad()`](../src/greedy_mesh.rs:362)）
   - 将 `(layer, u_start, v_start, width, height)` 映射回区块局部坐标
   - 生成 4 个顶点位置、UV 坐标、法线和索引

### 2.3 面方向配置

使用 `FaceConfig` 结构体通用化处理 6 个面方向，避免重复代码：

```rust
struct FaceConfig {
    face_name: &'static str,  // "top", "bottom", "side"
    face_index: usize,        // ChunkNeighbors 中的索引
    normal_axis: usize,       // 法线轴：0=X, 1=Y, 2=Z
    normal_sign: i32,         // 法线方向：+1 或 -1
    u_axis: usize,            // 掩码第一轴（宽度方向）
    v_axis: usize,            // 掩码第二轴（高度方向）
}
```

### 2.4 UV 映射策略

**当前实现**：拉伸模式 — 整个合并四边形映射到同一个纹理槽位。

```rust
// UV 坐标（拉伸模式）
let (u_min, u_max, v_min, v_max) = get_uv(block_id, face.face_name);
let face_uvs = [
    [u_min + UV_EPS, v_max - UV_EPS],
    [u_max - UV_EPS, v_max - UV_EPS],
    [u_max - UV_EPS, v_min + UV_EPS],
    [u_min + UV_EPS, v_min + UV_EPS],
];
```

**问题**：在 Texture Atlas 方案下，UV 是 Atlas 中的绝对位置。合并后 UV 无法正确平铺——如果将 UV 乘以 W/H 来平铺，会跨越 Atlas 中的其他纹理槽位。

**正确方案**：需要配合 **Texture Array**（纹理数组）才能正确工作，因为 Texture Array 的 UV 可以在 [0,1] 范围内自由平铺。

---

## 3. 集成改动

### 3.1 修改的文件

| 文件 | 改动 |
|------|------|
| [`src/main.rs`](../src/main.rs) | 添加 `mod greedy_mesh;` |
| [`src/chunk.rs`](../src/chunk.rs) | 删除旧的逐面生成代码，`spawn_chunk_entity` 改用 `greedy_mesh::generate_greedy_mesh()` |
| [`src/async_mesh.rs`](../src/async_mesh.rs) | 删除重复的异步网格生成代码，工作线程改用 `greedy_mesh::generate_greedy_mesh()` |

### 3.2 消除的代码重复

此前 `chunk.rs` 和 `async_mesh.rs` 各有一套完整的网格生成逻辑：

| 函数 | chunk.rs | async_mesh.rs | 状态 |
|------|----------|---------------|------|
| 网格生成 | `generate_chunk_mesh()` | `generate_chunk_mesh_async()` | 已删除，统一到 greedy_mesh |
| 面可见性 | `is_face_visible()` | `is_face_visible_async()` | 已删除，统一到 greedy_mesh |
| 面四边形 | `face_quad()` | `face_quad_async()` | 已删除，统一到 greedy_mesh |
| 面方向 | `Face` + `FACES` | `FaceAsync` + `FACES_ASYNC` | 已删除，统一到 greedy_mesh |

**净减少代码**：~200 行重复代码

---

## 4. 问题与回退

### 4.1 Texture Atlas UV 平铺问题

**现象**：Greedy Meshing 合并后，方块的材质被拉大，失去了原图的材质效果。

**原因**：当前使用 Texture Atlas（所有纹理拼在一张大图上），UV 坐标是 Atlas 中的绝对位置。合并后 UV 无法正确平铺。

**解决方案**（按优先级）：

1. **迁移到 Texture Array**（推荐）
   - Bevy 支持 `TextureArray`，每个纹理层独立，UV 可在 [0,1] 自由平铺
   - 需要修改 `resource_pack.rs` 的 Atlas 构建逻辑
   - 修改 `greedy_mesh.rs` 的 UV 计算：`uv = base_uv * (width, height)`
   - 工作量：2-3 天

2. **Greedy Meshing + 纹理索引属性**
   - 每个顶点额外存储纹理层索引（`@location(N) texture_index: u32`）
   - 着色器中根据索引从 Texture Array 采样
   - 需要自定义着色器
   - 工作量：3-4 天

3. **保持逐面生成**（当前方案）
   - 放弃 Greedy Meshing 的顶点优化
   - 通过 LOD 系统减少远景顶点数
   - 工作量：0

### 4.2 回退决策

**决定回退到逐面生成**，原因：
- Texture Atlas 方案下 Greedy Meshing 无法正确工作
- 迁移到 Texture Array 是独立的重构任务，不应与 Greedy Meshing 耦合
- 保留 `greedy_mesh.rs` 模块供未来 Texture Array 迁移后使用

---

## 5. 后续计划

### 5.1 启用 Greedy Meshing 的前置条件

```
Texture Array 迁移
    ↓
修改 greedy_mesh.rs UV 计算（平铺模式）
    ↓
集成到 chunk.rs 和 async_mesh.rs
    ↓
性能测试验证
```

### 5.2 Texture Array 迁移要点

1. `resource_pack.rs`：Atlas 构建改为 Texture Array 构建
2. `greedy_mesh.rs`：UV 计算改为平铺模式
   ```rust
   // 平铺模式：每个 1×1 方块都映射到完整纹理
   let face_uvs = [
       [u_min + eps, v_max - eps],           // 左下
       [u_min + w as f32 * uv_width - eps, v_max - eps],  // 右下
       [u_min + w as f32 * uv_width - eps, v_min + h as f32 * uv_height + eps],  // 右上
       [u_min + eps, v_min + h as f32 * uv_height + eps],  // 左上
   ];
   ```
3. 着色器：使用 `textureSample(t_array, s, uv, layer_index)` 替代 `textureSample(t_2d, s, uv)`

---

## 6. 参考资源

- [Greedy Meshing 算法详解](https://0fps.net/2012/06/30/meshing-in-a-minecraft-game/)
- [Voxy 源码](https://github.com/MCRcortex/voxy)
- [Bevy Texture Array 示例](https://bevyengine.org/examples/shaders/texture-array/)
- [WebGPU TextureArray 文档](https://www.w3.org/TR/webgpu/#texture-views)
