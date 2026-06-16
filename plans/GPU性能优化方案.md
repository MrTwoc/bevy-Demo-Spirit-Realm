

# GPU 网格生成与剔除迁移评估报告

## 一、当前 CPU 端管线完整梳理

### 1.1 网格生成管线

```text
阶段              | 模块                    | 线程    | 耗时/区块     | 调用频率
─────────────────┼────────────────────────┼────────┼──────────────┼──────────
① 地形生成        | terrain_gen.rs         | Worker | ~50ms        | 新区块加载
  - 8层噪声采样    | terrain_noise.rs       |        | (33000+次)   |
  - 洞穴检测       | (2次3D Simplex/体素)    |        |              |
  - 群系判定       | biome.rs               |        |              |
  - 树木生成       | tree_gen.rs            |        |              |
② 网格构建        | async_mesh.rs          | Worker | ~2-8ms       | 区块加载/脏块
  - 面可见性检查    | is_face_visible_fast   |        |              |
  - 列扫描优化      | generate_solid_mesh    |        |              |
  - 顶点/索引生成   | MeshVertex AoS         |        |              |
③ 水 Greedy Mesh  | greedy_mesh.rs         | Worker | ~0.1-0.3ms   | 含水区块
④ LOD 降采样       | lod.rs                 | Worker | ~0.5-2ms     | LOD1-3
⑤ GPU 上传        | collect_and_upload_meshes| Main  | ~0.1ms/批    | 每帧(≤64)
⑥ 脏块重建        | chunk_dirty.rs         | Main→W | 同②          | 方块修改
```

**核心瓶颈分析**：

| 瓶颈 | 严重程度 | 原因 | 量化数据 |
|------|---------|------|---------|
| `fill_terrain()` 噪声采样 | **极高** | 每区块 32³=32768 体素，每列 8 次 2D 噪声 + 每体素 2 次 3D 噪声(洞穴) | ~33000+ 次噪声采样/区块 |
| 网格面剔除 | **中等** | 每体素 6 面 × 邻居查询，列扫描已优化 50-70% | ~5000-15000 次面检查/区块 |
| 跨区块邻居查询 | **低** | `ChunkNeighbors` 已预取，但 HashMap 查找仍有开销 | 6 面边界 × 32×32 列 |
| CPU→GPU 上传 | **低** | 已优化：仅 Handle 变化时 insert，原地更新零 Command | 每帧 ≤64 区块 |

### 1.2 剔除管线

```text
层级              | 实现                    | 设备  | 粒度           | 状态
─────────────────┼────────────────────────┼──────┼───────────────┼──────
① SVO 八叉树构建  | node_manager.rs        | CPU  | LOD=4(512³)    | ✅ 已实现
② GPU 遍历        | gpu_traversal.rs       | GPU  | Top-level 节点 | ✅ 已实现
③ 可见性桥接      | visibility_bridge.rs   | CPU  | 逐 Chunk       | ✅ 已实现
④ Bevy 视锥剔除   | Bevy 内置              | GPU  | 逐实体         | ✅ 自动
⑤ LOD 距离切换    | lod.rs                 | CPU  | 逐 Chunk       | ✅ 已实现
```

**当前剔除管线的问题**：

| 问题 | 位置 | 影响 |
|------|------|------|
| SVO 只做 top-level 剔除 | `visibility_bridge.rs:195` | LOD=4 节点覆盖 16×16×16=4096 个体素区块，粒度过粗 |
| CPU 端可见性遍历 | `apply_svo_visibility()` | 每 2 帧遍历所有 `loaded_chunks.entries`，O(N) |
| `visible_coords` HashSet 重建 | `visibility_bridge.rs:341-350` | 每次更新都 clear + 重建，大量内存写入 |
| GPU 遍历结果未直接驱动渲染 | `SvoRenderQueue` 写入但未被消费 | GPU 剔除结果仅用于调试，实际可见性由 CPU 端 `visibility_bridge` 计算 |

---

## 二、GPU 迁移方案设计

### 2.1 方案对比：Compute Shader vs GPU-Driven 渲染管线

| 维度 | 方案 A：Compute Shader（渐进迁移） | 方案 B：GPU-Driven 渲染管线（Indirect Draw） |
|------|----------------------------------|-------------------------------------------|
| 核心思路 | CPU 生成体素数据 → GPU Compute 生成 Mesh → 回读或直接渲染 | 全链路 GPU：体素数据 → Mesh → 剔除 → Indirect Draw |
| 对 Bevy 改造 | **小**：仅新增 Compute Pass | **极大**：需绕过 Bevy PBR 管线 |
| 网格生成 | GPU 端生成顶点/索引 Buffer | 同左 |
| 剔除集成 | 扩展现有 SVO GPU 遍历，输出 Draw 参数 | GPU 端完整剔除 + Indirect Draw |
| CPU-GPU 同步 | 需要 Readback（地形数据回 CPU 用于碰撞/逻辑） | 最小同步 |
| 开发周期 | **4-8 周** | **12-20 周** |
| 风险 | 中等（wgpu API 约束） | 极高（Bevy 渲染管线深度改造） |

**推荐方案 A**（Compute Shader 渐进迁移），理由：
1. 项目已实现 SVO GPU 遍历的 Compute Shader 基础设施
2. Bevy 0.18 的 Material/Mesh 系统不原生支持 GPU-driven rendering
3. CPU 端仍需体素数据用于碰撞检测、方块交互等逻辑
4. 可分阶段交付，每阶段都有可量化的性能提升

### 2.2 Phase 1：GPU 地形噪声生成

**目标**：将 `fill_terrain()` 的噪声采样从 CPU 迁移到 GPU。

```text
当前：CPU Worker → fill_terrain() → ChunkData → CPU mesh generation
目标：GPU Compute → 噪声采样 + 地形判定 → Storage Buffer → CPU Readback → ChunkData
```

**Compute Shader 设计**：

```wgsl
// 地形生成 Compute Shader
@group(0) @binding(0) var<uniform> params: TerrainParams;      // 区块坐标 + 种子
@group(0) @binding(1) var<storage, read_write> output: array<u8>; // 32³ BlockId 数组

struct TerrainParams {
    chunk_x: i32, chunk_y: i32, chunk_z: i32,
    seed: u32,
    // 噪声参数...
}

@compute @workgroup_size(4, 4, 4)  // 8×8×8 = 512 threads, 覆盖 32³
fn generate_terrain(@builtin(global_invocation_id) id: vec3<u32>) {
    let x = id.x; let y = id.y; let z = id.z;
    if (x >= 32u || y >= 32u || z >= 32u) { return; }
    
    let world_x = f64(params.chunk_x) * 32.0 + f64(x);
    let world_y = params.chunk_y * 32 + i32(y);
    let world_z = f64(params.chunk_z) * 32.0 + f64(z);
    
    // GPU 端执行所有噪声采样 + 地形判定
    let block_id = compute_block_at(world_x, world_y, world_z, params);
    output[z * 32 * 32 + y * 32 + x] = block_id;
}
```

**关键挑战**：

| 挑战 | 当前状态 | 解决方案 |
|------|---------|---------|
| Simplex 噪声 GPU 实现 | 项目已有 `terrain_noise.rs` CPU 实现 | 在 WGSL 中实现 Simplex 2D/3D（~100 行） |
| FBM 多八度叠加 | `noise` crate 的 `Fbm<Simplex>` | WGSL 循环实现 octave 叠加 |
| RidgedMulti 噪声 | `noise` crate 的 `RidgedMulti` | `abs(noise)` + 频率/振幅调整 |
| Spline 插值 | `spline.rs` 的 Catmull-Rom | WGSL 实现分段三次 Hermite |
| 洞穴检测 (3D Simplex) | `is_cave()` 每体素 2 次采样 | GPU 并行化，天然高效 |
| wgpu Buffer 约束 | `MAP_READ` 不可与 `STORAGE` 组合 | 双缓冲：GPU STORAGE → COPY → MAP_READ（已实现） |

**性能收益估算**：

```text
CPU fill_terrain() 耗时：~50ms/区块（8 核 Worker 线程）
GPU Compute 耗时估算：
  - 32³ = 32768 体素，workgroup(4,4,4) = 64 threads
  - dispatch = (8,8,8) = 512 workgroups
  - 每线程执行 ~8 次 2D 噪声 + 2 次 3D 噪声 ≈ ~200 FLOP
  - GPU 吞吐量 ~1 TFLOP/s → ~0.07ms/区块
  - 含 buffer 上传/回读延迟：~0.5-1ms/区块
  
加速比：~50-100×（地形生成阶段）
```

### 2.3 Phase 2：GPU 网格生成

**目标**：在 GPU 端直接生成顶点缓冲和索引缓冲。

```text
当前：CPU Worker → 面剔除 + 顶点生成 → MeshResult → CPU 上传
目标：GPU Compute → 面剔除 + 顶点生成 → Storage Buffer → 直接绑定渲染
```

**两阶段 Compute Shader**：

```
Pass 1: 面可见性计数（compact）
  - 输入：32³ BlockId Buffer（Phase 1 输出）
  - 每体素检查 6 面可见性
  - 输出：可见面列表 + 前缀和（用于分散写入）

Pass 2: 顶点/索引生成
  - 输入：可见面列表 + BlockId Buffer + UV 表
  - 输出：VertexBuffer + IndexBuffer（直接可渲染）
```

**关键设计**：

| 设计点 | 方案 | 说明 |
|--------|------|------|
| 顶点格式 | `MeshVertex` (32 bytes) | 与当前 CPU 格式一致，可复用 UV 表 |
| 面剔除 | GPU 端 6 方向邻居查询 | 需要相邻区块边界数据（2 行 padding） |
| 输出缓冲 | Storage Buffer → Vertex Buffer | wgpu 支持 `STORAGE | VERTEX` 组合 |
| 索引缓冲 | 间接绘制索引 | 用 `DrawIndexedIndirect` 参数控制 |
| 跨区块边界 | 上传 34³ 的 padded 数据 | 边界 1 行来自邻居区块 |

**性能收益估算**：

```text
CPU generate_solid_mesh() 耗时：~2-8ms/区块（Worker 线程）
GPU Compute 耗时估算：
  - Pass 1：32³ × 6 面 × O(1) 查表 ≈ ~2M 操作 → ~0.02ms
  - Pass 2：~5000 面 × 4 顶点 ≈ ~20K 写入 → ~0.01ms
  - 含 buffer 管理：~0.1-0.3ms/区块
  
加速比：~10-30×（网格生成阶段）
```

### 2.4 Phase 3：GPU 剔除增强 + Indirect Draw

**目标**：扩展现有 SVO GPU 遍历，直接输出 Indirect Draw 参数。

```text
当前：GPU SVO 遍历 → visible_node_ids → CPU visibility_bridge → Visibility 组件
目标：GPU SVO 遍历 → visible_node_ids → GPU 端 Mesh 剔除 → DrawIndexedIndirect
```

**架构改造**：

```
┌─────────────────────────────────────────────────────────────┐
│ GPU Compute Pass 1: SVO 遍历（已有）                         │
│   → 输出：visible_chunk_ids[]                                │
├─────────────────────────────────────────────────────────────┤
│ GPU Compute Pass 2: 逐 Chunk 视锥体精确剔除（新增）           │
│   - 输入：visible_chunk_ids + 每 Chunk 的 AABB               │
│   - 输出：DrawIndexedIndirect 参数（剔除后的 Draw Call 列表） │
├─────────────────────────────────────────────────────────────┤
│ GPU Render Pass: Indirect Draw（新增）                       │
│   - 执行 indirect_draw_indexed()                             │
│   - 每个可见 Chunk 一个 Draw Call（但由 GPU 发起）            │
└─────────────────────────────────────────────────────────────┘
```

**Bevy 集成方案**：

由于 Bevy 0.18 不原生支持 Indirect Draw，需要：
1. 自定义 `PhaseItem` + `RenderCommand`
2. 在 Render World 中维护 GPU Buffer
3. 绕过标准的 `MeshPipeline`，使用自定义 `RenderPipeline`

这是**改造成本最高**的阶段。

---

## 三、定量性能收益评估

### 3.1 显存占用变化

| 资源 | 当前 CPU 方案 | Phase 1 (GPU 噪声) | Phase 2 (GPU Mesh) | Phase 3 (Indirect) |
|------|-------------|--------------------|--------------------|-------------------|
| ChunkData (CPU) | 32KB/区块 × 2000 = **64MB** | 不变 | 不变 | 不变 |
| Mesh Vertex Buffer | ~1MB/区块 × 2000 = **~2GB** | 不变 | **同量级**（GPU 端生成） | 不变 |
| Mesh Index Buffer | ~0.5MB/区块 × 2000 = **~1GB** | 不变 | 同量级 | 不变 |
| GPU 噪声 Buffer | 0 | **32KB/区块**（临时） | 32KB | 32KB |
| GPU Mesh Buffer | 0 | 0 | **~1.5MB/区块**（持久） | 不变 |
| Indirect Buffer | 0 | 0 | 0 | **~24B/区块** |
| SVO Node Buffer | **~4MB**（已有） | 不变 | 不变 | 不变 |
| **总计** | **~3GB + 64MB** | **~3GB + 64MB + 临时** | **~3GB**（GPU 持久化） | **~3GB** |

**结论**：显存占用基本不变。GPU 端生成的 Mesh Buffer 与 CPU 上传的 Buffer 大小相同。

### 3.2 带宽消耗变化

| 方向 | 当前 | Phase 1 | Phase 2 | Phase 3 |
|------|------|---------|---------|---------|
| CPU→GPU (地形数据) | 0（CPU 生成） | **+32KB/区块**（params uniform） | 不变 | 不变 |
| GPU→CPU (地形 Readback) | 0 | **+32KB/区块**（ChunkData 回读） | 不变 | 不变 |
| CPU→GPU (Mesh 上传) | **~1.5MB/区块** | 不变 | **0**（GPU 端生成） | 0 |
| GPU→CPU (Readback) | 0 | 32KB | 0 | 0 |
| **净变化** | 基准 | **+64KB/区块** | **-1.5MB/区块** | 同 Phase 2 |

**Phase 2 的带宽收益显著**：消除了每区块 ~1.5MB 的 CPU→GPU Mesh 上传。

### 3.3 CPU-GPU 同步开销

| 阶段 | 同步点 | 当前开销 | 迁移后开销 |
|------|--------|---------|-----------|
| 地形生成 | GPU→CPU Readback | 0 | **~0.5ms**（双缓冲延迟） |
| 网格上传 | CPU→GPU 写入 | ~0.1ms/批(64) | 0 |
| 剔除结果 | GPU→CPU Readback | ~0.1ms（SVO） | ~0.1ms（同） |
| 帧延迟 | 0 | 0 | **+1 帧**（Readback 延迟） |

### 3.4 Draw Call 数量变化

| 方案 | Draw Call 数量 | 来源 |
|------|---------------|------|
| 当前 | ~2000（可见区块） | Bevy 自动批处理 |
| Phase 3 Indirect | ~2000（GPU 剔除后） | `DrawIndexedIndirect` |
| Phase 3 + 合批 | **~100-200** | 材质合批 + 实例化渲染 |

**注意**：Bevy 的自动批处理已经将同材质区块合并，Phase 3 的 Draw Call 数量变化不大。主要收益是**GPU 发起 Draw Call** 避免了 CPU 端的遍历和 Command 提交开销。

---

## 四、开发复杂度与风险评估

### 4.1 各阶段开发成本

| 阶段 | 开发时间 | 改造范围 | 新增代码量 | 关键风险 |
|------|---------|---------|-----------|---------|
| Phase 1: GPU 噪声 | **2-3 周** | 新增 Compute Shader + Readback | ~500 行 WGSL + ~300 行 Rust | wgpu Buffer 约束（已解决） |
| Phase 2: GPU Mesh | **3-5 周** | 新增 2-pass Compute + Buffer 管理 | ~400 行 WGSL + ~500 行 Rust | 跨区块边界数据同步 |
| Phase 3: Indirect Draw | **6-10 周** | 自定义 Bevy 渲染管线 | ~600 行 WGSL + ~1000 行 Rust | Bevy 版本升级兼容性 |

### 4.2 兼容性风险

| 风险 | 影响 | 缓解措施 |
|------|------|---------|
| wgpu Buffer 用途组合限制 | `MAP_READ` 不可与 `STORAGE` 组合 | 双缓冲方案（已实现） |
| Bevy 版本升级破坏 API | 0.18 → 未来版本可能改 Render API | Phase 3 尽量使用公共 API |
| GPU 端噪声精度差异 | f32 vs f64 精度不同 | 关键路径使用 f32，接受微小差异 |
| 跨平台 Shader 兼容性 | WGSL 在不同 GPU 上行为一致 | 已验证 wgpu 的 WGSL 支持 |
| CPU 逻辑仍需体素数据 | 碰撞、射线检测、方块交互 | GPU→CPU Readback（延迟 1 帧可接受） |

### 4.3 现有渲染管线改造成本

| 组件 | 改造程度 | 说明 |
|------|---------|------|
| `async_mesh.rs` | **Phase 2 后可简化** | Worker 线程仅保留地形生成（Phase 1 后可移除） |
| `chunk_manager.rs` | **小改** | `collect_and_upload_meshes` 需适配 GPU Buffer |
| `svo/gpu_traversal.rs` | **Phase 3 扩展** | 新增 Indirect Draw 输出 |
| `svo/visibility_bridge.rs` | **Phase 3 后可删除** | GPU 端直接驱动渲染 |
| `lod.rs` | **不变** | LOD 切换逻辑保持 CPU 端 |
| Bevy Material 系统 | **Phase 3 大改** | 自定义 `PhaseItem` + `RenderCommand` |

---

## 五、分阶段迁移建议

### 阶段 1：GPU 地形噪声生成（优先级最高）

```text
预期收益：
  - fill_terrain() 耗时：50ms → ~0.5ms（100× 加速）
  - Worker 线程负载大幅降低
  - 区块加载吞吐量提升 ~5-8×

改造步骤：
  1. 实现 WGSL Simplex 2D/3D 噪声函数
  2. 实现 FBM、RidgedMulti、Spline 的 GPU 版本
  3. 创建 terrain_gen.wgsl Compute Shader
  4. 实现 GPU→CPU Readback（复用双缓冲方案）
  5. 在 AsyncMeshManager::Prepare 阶段调用 GPU 地形生成
  6. 保留 CPU fallback（Void/Flat/MengerSponge 世界类型）

依赖项：
  - 已有：wgpu 双缓冲方案、SVO GPU 遍历基础设施
  - 需新增：WGSL 噪声库（~200 行）
```

### 阶段 2：GPU 网格生成

```text
预期收益：
  - 网格生成耗时：2-8ms → ~0.2ms（15× 加速）
  - 消除 CPU→GPU Mesh 上传带宽：~1.5MB/区块 → 0
  - Worker 线程可进一步减少

改造步骤：
  1. 扩展地形生成 Shader，增加邻居边界 padding
  2. 实现面可见性检查 + 顶点生成的 Compute Shader
  3. 输出 Vertex/Index Buffer（STORAGE | VERTEX 用途）
  4. 修改 chunk_manager 使用 GPU Buffer 创建 Bevy Mesh
  5. 实现 LOD 级别的 GPU 降采样

依赖项：
  - Phase 1 完成（GPU 地形数据直接可用）
  - 需新增：mesh_gen.wgsl（~300 行）
```

### 阶段 3：GPU-Driven 剔除（可选，高风险）

```text
预期收益：
  - 消除 CPU 端 visibility_bridge 遍历开销
  - Draw Call 由 GPU 发起，减少 CPU Command 提交
  - 理论上可实现完全 GPU-driven 的体素渲染

改造步骤：
  1. 扩展 SVO GPU 遍历，输出 Chunk AABB 可见性
  2. 实现 DrawIndexedIndirect 参数生成
  3. 自定义 Bevy PhaseItem + RenderCommand
  4. 绕过标准 MeshPipeline 使用自定义管线

依赖项：
  - Phase 2 完成（GPU 端有完整的 Mesh 数据）
  - 深入 Bevy 渲染管线内部（高维护成本）
```

### 总体预期时间线

```text
Week 1-3:   Phase 1 — GPU 地形噪声 → fill_terrain 100× 加速
Week 4-8:   Phase 2 — GPU 网格生成 → 消除 Mesh 上传带宽
Week 9-18:  Phase 3 — GPU-Driven 剔除（可选，视需求决定）
```

### 推荐优先级

**Phase 1 最值得优先实施**，原因：
1. `fill_terrain()` 是单区块耗时最大的操作（~50ms），直接决定区块加载延迟
2. 开发成本最低（2-3 周），风险最小
3. 已有 wgpu 双缓冲 Readback 方案可复用
4. 对用户体验的提升最明显（更快的区块加载、更远的可见距离）

Phase 2 的带宽收益也很显著，但开发复杂度略高。Phase 3 除非需要极致性能，否则建议推迟——当前 Bevy PBR + SVO CPU 剔除的组合已经足够高效。
