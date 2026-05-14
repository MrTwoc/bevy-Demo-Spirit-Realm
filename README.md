# Spirit Realm 灵境

> 更新时间：2026年5月14日
>
> 项目计划近期开始学习&更新，更新介绍文章将放在个人博客：https://www.twocblog.site/

---

## 项目简介

本项目是个人学习 Rust 和 Bevy 引擎的实践项目，使用 Bevy 引擎 + Rust 编程语言实现的一个 3D 体素沙盒类游戏。

### 项目目标

- **超大的生成、建造空间**：Y 轴 ±20480 格，自然生成高度 ±10240 格
- **大规模渲染**：128+ 区块视距，60+ FPS
- **参考基准**：Voxy 模组在 GTX 1060 + i7-8700K + 16GB RAM 上，256 区块视距，静止不动~270 FPS

### 技术选型

| 类别 | 技术 |
|------|------|
| 游戏引擎 | Bevy 0.18.1 |
| 编程语言 | Rust |
| 空间索引 | SubChunk (32³) + HashMap 稀疏索引 |
| 地形生成 | OpenSimplex + FBM 噪声 |

---

## Phase 进度看板

```
Phase 0 ✅       Phase 1 ✅       Phase 2 🚧       Phase 3 ⏳       Phase 4-5 ⏳
异步网格生成      LOD 系统         GPU 面剔除       MegaLOD Tile     空间索引
已完成           初步完成          下一步           待开始           待开始
```

### Phase 0：异步网格生成 — ✅ 已完成

- [x] 后台工作线程架构（`src/async_mesh.rs`，mpsc 通道通信）
- [x] MeshTask 任务队列（Generate + Cancel 两种任务）
- [x] 分帧结果收集（每帧 MESH_UPLOADS_PER_FRAME 上限）
- [x] 全空气区块跳过（实体数 ~2,200 → ~600-800）
- [x] 幽灵方块 Bug 修复（异步竞态条件修复）
- [x] GPU 内存泄漏修复（卸载时 meshes.remove() + materials.remove()）
- [x] 加载尖峰消除（21.8ms → <8ms）

### Phase 1：LOD 系统 — ✅ 初步完成

- [x] 四级 LOD 降采样（LOD0 1:1 ~ LOD3 1:8）
- [x] LodManager Resource（滞后切换策略）
- [x] generate_lod_mesh 降采样算法
- [x] 跨 LOD 接缝处理
- [x] 渲染距离扩展：8 → 32 区块

### Phase 2：GPU 面剔除 — 🚧 下一步

- [ ] Compute Shader 面剔除（CPU → GPU）
- [ ] MultiDrawIndirect
- [ ] 视锥剔除 + Hi-Z 遮挡剔除
- [ ] Draw Call 优化目标：~600-800 → ~100

### Phase 3-5：远期规划

- Phase 3：MegaLOD Tile 远景系统（64×64×16 SubChunks 合并为瓦片）
- Phase 4：多级空间索引（WorldColumn → MegaColumn → SubChunk）
- Phase 5：内存池优化（MeshBufferPool 复用）

---

## 核心功能

- [x] 超平坦世界生成
- [x] 噪声世界生成（OpenSimplex2 + FBM）
- [x] 方块放置/破坏
- [x] 方块材质加载（TextureAtlas + UV 映射）
- [x] 区块系统（32×32×32 SubChunk）
- [x] 异步网格生成（后台工作线程）
- [x] 四级 LOD 系统（1:1 / 1:2 / 1:4 / 1:8 降采样）
- [x] 空气区块跳过优化（减少 60-70% 实体）
- [x] 性能日志系统（FPS、帧时间、三角面数、Draw Call）
- [x] 射线检测与方块交互
- [x] 资源包系统
- [ ] GPU Compute Shader 面剔除（Phase 2）
- [ ] 多噪声层地形生成（Phase 4）
- [ ] 动态 Atlas 材质包系统

---

## 当前性能基线

| 指标 | 数值 |
|------|------|
| RENDER_DISTANCE | 8-32 区块（可配置） |
| 区块实体数 | 每区块一个实体 |
| FPS 稳态 | ~170-200 |
| 帧时间 | ~5.9ms（均值），~8ms（峰值） |
| GPU 三角面 | ~8.7M-9.0M |

### 性能目标

| 阶段 | 硬件 | 视距 | 预估 FPS |
|------|------|------|---------|
| Phase 1 完成 | GTX 1060+ | 16 区块 | ~200-250 |
| Phase 2 完成 | GTX 1060+ | 32 区块 | ~150-200 |
| Phase 3 完成 | GTX 1060+ | 128 区块 | ~80-120 |
| 远期目标 | RTX 4070+ | 2048 区块 | ~60+ |

---

## 项目结构

```
src/
├── main.rs                 # 程序入口，Bevy App 配置
├── chunk.rs                # 区块数据结构（32³ SubChunk）
├── chunk_manager.rs        # 区块生命周期管理、加载/卸载
├── chunk_dirty.rs          # 脏区块标记与重建触发
├── async_mesh.rs           # 异步网格生成（后台工作线程）
├── lod.rs                  # LOD 系统（四级降采样）
├── raycast.rs              # 射线检测（方块选择）
├── block_interaction.rs    # 方块放置/破坏交互
├── camera.rs               # 摄像机控制
├── input.rs                # 输入处理
├── lighting.rs             # 光照系统
├── hud.rs                  # HUD 显示（坐标、方块信息）
├── fps_overlay.rs          # FPS 覆盖层
├── perf_logger.rs          # 性能日志记录
├── resource_pack.rs        # 资源包加载与材质管理
└── chunk_wire_frame.rs     # 区块线框调试

assets/
├── textures/               # 纹理图片
├── shaders/                # WGSL 着色器
├── resourcepacks/          # 材质包
└── skybox/                 # 天空盒纹理

docs/                       # 设计文档
├── 架构总纲.md             # 项目整体架构
├── 体素管理方案.md         # GPU 渲染管线设计
├── LOD系统设计文档.md       # Phase 1 详细设计
├── Phase完整规划.md        # 完整路线图
├── 多噪声层地形生成方案.md # 地形系统设计
└── ...                     # 更多文档

plans/                      # 实施计划
└── Phase2-StepA-ComputeShader面剔除实现方案.md
```

---

## 技术架构

### 渲染管线

```
玩家移动 → ChunkLoader → LoadedChunks → 
    ↓
DirtyChunk 标记 → AsyncMeshManager → 后台线程生成 Mesh →
    ↓
Mesh 上传 → Draw Call → GPU 渲染
```

### 核心技术点

1. **ECS 架构**：Bevy 引擎的 Entity-Component-System 模式
2. **异步网格生成**：后台工作线程 + mpsc 通道 + 分帧上传
3. **LOD 降采样**：4 级 LOD（1:1 / 1:2 / 1:4 / 1:8），滞后切换策略
4. **GPU 驱动渲染**（规划中）：Compute Shader 面剔除 + MultiDrawIndirect
5. **SubChunk 存储**：32³ 体素块 + HashMap 稀疏索引
6. **材质系统**：TextureAtlas + UV 映射，支持资源包切换

---

## 开发路线图

```
Phase 0 ✅  2024-11 ~ 2026-05-08
└── 异步网格生成 + 优化

Phase 1 ✅  2026-05
└── LOD 系统（四级降采样）

Phase 2 🚧  2026-05（下一步）
├── Compute Shader 面剔除
├── MultiDrawIndirect
└── Draw Call ~100

Phase 3 ⏳
└── MegaLOD Tile（64×64×16 瓦片）

Phase 4 ⏳
└── 多级空间索引

Phase 5 ⏳
└── 内存池优化

远景目标
└── 128+ 区块视距，Y=±20480
```

---

## 参考资料

### 官方文档

- [Bevy 引擎](https://bevyengine.org/)
- [Rust 编程语言](https://www.rust-lang.org/zh-CN/)

### 项目文档

- [`docs/文档目录.md`](docs/文档目录.md) — 完整文档索引
- [`docs/架构总纲.md`](docs/架构总纲.md) — 项目整体架构蓝图
- [`docs/Phase完整规划.md`](docs/Phase完整规划.md) — 完整 Phase 路线图
- [`plans/Phase2-StepA-ComputeShader面剔除实现方案.md`](plans/Phase2-StepA-ComputeShader面剔除实现方案.md) — Phase 2 实施计划

### 技术参考

- [Voxy 体素引擎](https://github.com/Geofmorgen/voxy) — 核心技术借鉴
- [Entity Component System 架构](https://mp.weixin.qq.com/s/dfEyst39sZ1fRCV6hcqCDA)
- [GPU 体素渲染技术](https://www.bilibili.com/video/BV1he411q7Zi/)
- [地形生成借鉴](https://github.com/Apollounknowndev/tectonic)

---

## 运行项目

```bash
# 编译
cargo build --release

# 运行
cargo run --release

# 调试模式（显示 FPS、线框等）
cargo run
```

### 控制说明

| 按键 | 功能 |
|------|------|
| W/A/S/D | 移动 |
| 空格 / Shift | 上升 / 下降 |
| 鼠标左键 | 破坏方块 |
| 鼠标右键 | 放置方块 |
| V键 | 切换线框模式 |
