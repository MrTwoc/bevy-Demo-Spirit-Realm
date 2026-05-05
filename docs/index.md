# 文档目录

> 本目录收录 Spirit Realm 体素沙盒项目的所有设计文档，按优先级和用途分类。

---

## 核心架构（必读）

### [架构总纲.md](./架构总纲.md)
**项目整体架构蓝图**。整合了空间管理、LOD、剔除系统、渲染管线的完整方案。
- 目标规模：Y轴 ±20480、XZ无限、128+ 区块可视距离
- 整体架构：动态近景（0~16区块）+ 静态远景（17~128+区块）
- SubChunk 分层：16×32×16，HashMap 稀疏索引
- 渲染管线：Frustum Culling + Hi-Z Occlusion + MultiDrawIndirect

### [体素管理方案.md](./体素管理方案.md)
**极致性能体素引擎设计方案（GPU 加速全栈）**，77KB 详细设计文档。
- GPU Compute Shader 面提取（WGSL）
- SuperChunk 合批（8×8×4 区块合并为单一 Draw Call）
- MegaLOD 远景瓦片（64×64×16 区块，远景专用）
- ChunkData 三态存储：Empty / Uniform / Mixed（调色板压缩）
- 区块生命周期：Unloaded → Generating → Dormant → Active → Unloaded
- 邻居数据打包跨区块面剔除方案
- Bevy ECS 系统集成示例

---

## 纹理与材质

### [纹理实施方案.md](./纹理实施方案.md)
**早期纹理方案**，描述顶点颜色到 UV 纹理映射的技术路径。
- 已废弃，被 PlanB 方案替代

### [PlanB实施方案.md](./PlanB实施方案.md)
**当前已实施的纹理方案**，TextureAtlas + UV 映射的实际实现。
- atlas.rs：AtlasSlot 结构体、UV 计算、grass/dirt/stone slot 定义
- chunk.rs：BlockTexture 枚举、face_quad UV 计算
- 纹理 atlas：`assets/textures/array_texture.png`，250×1000px，子图 32×32px

### [动态Atlas材质包系统.md](./动态Atlas材质包系统.md)
**后期材质系统目标**，类似 Minecraft 1.13+ 的运行时动态 Atlas 构建。
- 启动时自动扫描材质文件，拼成大图集
- block model JSON 声明方块各面纹理
- 材质包热切换：运行时替换材质包，Atlas 自动重建

---

## 空间管理（进阶）

### [体素空间管理SVO方案.md](./体素空间管理SVO方案.md)
**稀疏八叉树（SVO）空间管理**，可作为架构总纲的补充参考。
- 三维区块 + SVO 混合架构
- SVO 优势：内存效率、射线检测 O(log n)、天然 LOD
- 当前评估：已被 ChunkData 三态替代为 SubChunk 内存储，SVO 降级为远景射线检测可选方案
- 实现路径：Phase 1（单 SubChunk 内 SVO）→ Phase 2（整合调度）→ Phase 3（渲染优化）

---

## 文档关系图

```
架构总纲.md（入口，所有方案整合）
    │
    ├── 体素管理方案.md（GPU 管线、SuperChunk、MegaLOD、ChunkData 三态）
    │       └── 体素空间管理SVO方案.md（补充：SVO 用于远景查询）
    │
    └── 动态Atlas材质包系统.md（后期纹理系统，独立演进）
            │
            └── PlanB实施方案.md（当前已实施）
                    └── 纹理实施方案.md（早期方案，已废弃）
```

---

## 阅读建议

| 身份 | 推荐阅读顺序 |
|------|------------|
| 刚接触项目 | 架构总纲 → 体素管理方案 → PlanB实施方案 |
| 想改纹理/材质 | PlanB实施方案 → 动态Atlas材质包系统 |
| 想研究空间索引 | 架构总纲 → 体素空间管理SVO方案 → 体素管理方案 |
| 关注渲染性能 | 体素管理方案（第 3~6 章）→ 架构总纲（渲染管线）|
