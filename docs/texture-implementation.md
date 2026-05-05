# 体素游戏纹理方案详解

> 本文档整理 Bevy 引擎中两种主流纹理实现方案的原理、区别、适用场景，及其在体素游戏中的应用。

---

## 1. 概述

在体素游戏中，方块（voxel）需要为不同的面映射不同的纹理。例如草方块的顶面是草纹理、侧面是草+土混合纹理、底面是土纹理。Minecraft 等游戏通过**纹理图集（Atlas）**实现，Bevy 引擎提供了两种底层机制来支持这一需求。

| 方案 | 名称 | 机制 |
|------|------|------|
| 方案 A | `texture_2d_array` | GPU 纹理数组，通过 layer 索引选取纹理层 |
| 方案 B | `TextureAtlas` | 普通 2D 大图，通过 UV 坐标偏移选取区域 |

---

## 2. 纹理资源

项目路径：`assets/textures/array_texture.png`

- **尺寸**：250 × 1000 像素
- **布局**：水平排列的子图，每个子图约 16×16 或 32×32 像素
- **内容**：
  - 第 1 个纹理：草地（grass）
  - 第 2 个纹理：泥土（dirt）
  - 第 3、4 个纹理：预留

> 注意：当前项目代码中此文件**未被使用**，Mesh 顶点目前使用纯色（顶点颜色），未接入纹理系统。

---

## 3. 方案 B — TextureAtlas（纹理图集 + UV 偏移）

### 3.1 原理

将所有子纹理打包进一张大图，shader 中对整张大图进行普通 2D 采样。每个面的顶点携带不同的 UV 坐标，指向大图中对应子图的区域。

```
atlas 大图 (250 x 1000)
┌─────────────────────────────────────────────┐
│  grass   │  dirt   │  ???   │  ???   │      │
│  (0-62)  │(63-125) │(126-188)│(189-251)    │
└─────────────────────────────────────────────┘

顶点 UV 坐标示例（grass 子图宽 62px）：
  ┌────────────┐
  │ (0,0)─┬──→ │  uv.x = x / 250.0
  │   ↓    │    │  uv.y = y / 1000.0
  │ (0,1) └────│
  └────────────┘
```

采样在 shader 中等价于：
```wgsl
let pixel_pos = vec2(u * atlas_width, v * atlas_height);
let color = textureSample(atlas_texture, sampler, pixel_pos);
```

### 3.2 Bevy 实现方式

**参考示例**：`bevy/examples/3d-rendering/generate-custom-mesh/`

核心步骤：

#### a) 定义 UV 坐标（在 generate_chunk_mesh 中）

每个顶点根据其所属面的类型，设置不同的 UV 坐标：

```rust
// 伪代码：generate_chunk_mesh 函数内
let mut uvs: Vec<[f32; 2]> = Vec::new();

// grass 面（layer 0，宽 62px，高 62px，位于 atlas 左侧）
let grass_u_min = 0.0 / 250.0;
let grass_u_max = 62.0 / 250.0;
let grass_v_min = 0.0 / 1000.0;
let grass_v_max = 62.0 / 1000.0;

// dirt 面（layer 1）
let dirt_u_min = 63.0 / 250.0;
// ... 以此类推

// 为每个顶点写入 UV
for (block_type, face_normal) in visible_faces {
    let (u0, u1, v0, v1) = match block_type {
        BlockType::Grass => (grass_u_min, grass_u_max, grass_v_min, grass_v_max),
        BlockType::Dirt  => (dirt_u_min, dirt_u_max, dirt_v_min, dirt_v_max),
        BlockType::Stone => /* ... */,
    };
    // 根据面法线确定 UV 排列
    // ...
    uvs.push([u, v]);
}

mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
```

#### b) 材质

直接使用 `StandardMaterial`，无需自定义 shader：

```rust
let material = materials.add(StandardMaterial {
    base_color_texture: Some(atlas_texture_handle),
    ..default()
});
```

#### c) 纹理加载

```rust
let texture_handle = asset_server.load("textures/array_texture.png");
let atlas = TextureAtlas::new_empty(texture_handle, Vec2::new(250.0, 1000.0));
```

### 3.3 优点

- 实现简单，不需要写自定义 shader
- 所有纹理打包在一张图，GPU 一次绑定即可
- UV 计算符合直觉，美术资源导出为单张图即可使用

### 3.4 缺点

- 切换子纹理需要修改顶点缓冲中的 UV 值（CPU 端操作，略有开销）
- UV 坐标跨区域跳转时，纹理缓存命中率可能下降
- atlas 尺寸变化时需要重新计算所有 UV

### 3.5 适用场景

- 静态纹理映射：每个面绑定后基本不换
- 单一 Draw Call：所有面共享同一 atlas 纹理
- 快速原型：实现成本低

---

## 4. 方案 A — texture_2d_array（GPU 纹理数组 + MeshTag）

### 4.1 原理

GPU 端存储多个独立的纹理层（类似一叠 PNG），shader 中通过 **layer 索引** 选取具体层进行采样。Layer 索引来自实体上的 `MeshTag` 组件，与 UV 坐标完全独立。

```
texture_2d_array（GPU 内部结构）
┌─────────────────────────┐
│ layer 0: grass          │ ← 独立显存区域，GPU 直接索引
├─────────────────────────┤
│ layer 1: dirt           │
├─────────────────────────┤
│ layer 2: stone          │
├─────────────────────────┤
│ layer 3: reserved        │
└─────────────────────────┘

采样方式：
  color = textureSample(array_texture, sampler, uv, layer_index)
                    ↑           ↑         ↑        ↑
                    纹理        采样器    2D坐标   layer索引
```

每个面使用统一的 UV [0,1]，只需改变 layer 索引即可切换纹理。

### 4.2 Bevy 实现方式

**参考示例**：`bevy/examples/shader/array_texture/`

核心步骤：

#### a) 自定义材质 + WGSL Shader

新建 `assets/shaders/array_texture.wgsl`：

```wgsl
#import bevy_pbr::mesh_view_bindings::view
#import bevy_pbr::mesh_functions::mesh
#import bevy_pbr::pbr_functions::pbr_input_new
#import bevy_pbr::pbr_bindings::{pbr_bindings,StandardMaterialFlags}
#import bevy_pbr::tonemapping::tone_mapping
#import bevy_core_pipeline::tonemapping::tone_mapping

@group(1) @binding(0) var my_array_texture: texture_2d_array<f32>;
@group(1) @binding(1) var my_sampler: sampler;

@fragment
fn fragment(
    @builtin(front_facing) is_front: bool,
    mesh: mesh::VertexOutput,
) -> @location(0) vec4 {
    // 通过 MeshTag 获取 layer 索引（草=0, 土=1, 石=2）
    let layer = mesh_functions::get_tag(mesh.instance_index);

    var pbr_input = pbr_input_new();
    pbr_input.material.base_color = textureSample(
        my_array_texture,
        my_sampler,
        mesh.uv,
        layer,
    );

    pbr_input.frag_coord = mesh.position;
    pbr_input.world_position = mesh.world_position;
    pbr_input.world_normal = mesh.world_normal;

    let double_sided = (pbr_input.material.flags
        & StandardMaterialFlags::DOUBLE_SIDED_BIT) != 0u;

    pbr_input.is_orthographic = view.clip_from_view[3].w == 1.0;
    pbr_input.N = normalize(pbr_input.world_normal);

    return tone_mapping(
        bevy_pbr::pbr_functions::apply_pbr_lighting(pbr_input),
        view.color_grading,
    );
}
```

#### b) Rust 端材质定义

```rust
use bevy::{
    prelude::*,
    reflect::TypePath,
    render::render_resource::{AsBindGroup, ShaderRef},
    image::ImageArrayLayout,
};

// 自定义材质
#[derive(Asset, TypePath, AsBindGroup)]
pub struct ArrayTextureMaterial {
    #[texture(0, dimension = "2d_array")]
    #[sampler(1)]
    pub array_texture: Handle<Image>,
}

// 实现 Material trait
impl Material for ArrayTextureMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/array_texture.wgsl".into()
    }
    fn vertex_shader() -> ShaderRef {
        "shaders/array_texture.wgsl".into() // 或使用内置顶点 shader
    }
}
```

#### c) 纹理加载（带 ImageArrayLayout）

```rust
use bevy::image::{ImageArrayLayout, ImageLoaderSettings};

let array_texture: Handle<Image> = asset_server.load_with_settings(
    "textures/array_texture.png",
    |settings: &mut ImageLoaderSettings| {
        settings.layout = ImageArrayLayout::二维阵列(4); // 4 层
    },
);
```

#### d) Mesh 生成

每个面的 UV 统一设为整层范围 [0,1]，但需要附加 `MeshTag` 组件：

```rust
// 为实体附加 MeshTag，告知 shader 使用哪一层
commands.entity(entity).insert(MeshTag(0)); // grass 层
```

在 WGSL 中：
```wgsl
let layer = mesh_functions::get_tag(mesh.instance_index);
```

### 4.3 优点

- GPU 高效采样：layer 是硬件级索引，无 UV 坐标计算开销
- 动态切换零成本：改变 `MeshTag` 值即可切换纹理，无需修改顶点缓冲
- 纹理缓存友好：各层独立存储，连续访问同一层时缓存命中率高
- 适合大量纹理变体：同一种草方块可有多套皮肤，切换成本极低

### 4.4 缺点

- 需要编写自定义 WGSL shader
- 需要为每个面单独创建实体（或使用 InstancedMesh + per-instance MeshTag）
- 迁移成本：从方案 B 切换过来需要重写 UV 和材质逻辑

### 4.5 适用场景

- 需要动态换肤的方块（如季节变换、工具磨损等）
- 大量高频切换的纹理变体
- 追求极致 GPU 采样效率
- 移动端省电需求

---

## 5. 方案对比

| 维度 | 方案 A (texture_2d_array) | 方案 B (TextureAtlas) |
|------|---------------------------|---------------------|
| **GPU 采样方式** | 3D 采样 `textureSample(tex, uv, layer)` | 2D 采样 `textureSample(tex, uv)` |
| **选图机制** | layer 索引（组件数据） | UV 坐标偏移（顶点数据） |
| **Shader 复杂度** | 需要自定义 WGSL | 使用 StandardMaterial 即可 |
| **改纹理成本** | 改 MeshTag 值，GPU 零开销 | 需 CPU 修改顶点 UV 缓冲 |
| **显存布局** | 多层独立纹理 | 单张大 atlas 图 |
| **纹理缓存** | 层内连续访问命中率高 | UV 跨区跳转可能失效 |
| **实现难度** | 中高（需 shader 知识） | 低（标准 API） |
| **Minecraft 使用** | ❌ 未使用 | ✅ 使用 atlas + UV |
| **切换成本（方案A→B）** | 需改材质和 UV | — |
| **切换成本（方案B→A）** | 需写 shader + 简化 UV | — |

### 渲染效率（体素游戏场景）

两者**几乎没有差距**：

- 体素游戏 Draw Call 数量才是主要瓶颈（当前项目已优化为每 Chunk 1 个 Draw Call）
- atlas 尺寸有限（250×1000），UV 计算成本可忽略
- GPU 纹理采样本身不是瓶颈

### 真实瓶颈排序（体素游戏）

```
1. Draw Call 数量          ← 最影响 CPU 端
2. 面剔除（Frustum/OC）    ← 已做
3. 纹理采样效率             ← 两者无差别
4. 顶点处理
```

---

## 6. Minecraft 中的实现

Minecraft 使用的是 **方案 B（TextureAtlas + UV 坐标）**。

### terrain.png 格式

早期 Minecraft 将所有方块纹理打包进一张 `terrain.png`（256×256 或 512×512），每个子图 16×16 像素。

### JSON 模型定义（1.8+）

```json
{
    "parent": "block/cube",
    "textures": {
        "particle": "block/grass_side",
        "down":  "block/dirt",
        "up":    "block/grass_top",
        "north": "block/grass_side",
        "south": "block/grass_side",
        "east":  "block/grass_side"
    }
}
```

每张纹理的 UV 在运行时映射到 atlas 的对应区域：

```
atlas UV 计算：
  grass_top  → [0,   0] - [16, 16]   → 在 atlas 的第 0 列
  dirt       → [16,  0] - [32, 16]   → 在 atlas 的第 1 列
  grass_side → [0,  16] - [16, 32]   → 在 atlas 的第 0 行第 1 列
```

### 渲染流程（Minecraft 1.16+ Bundle 系统）

1. 每个 Chunk 收集所有可见面（面剔除后）
2. 按纹理类型分组（所有 grass_top 面合并，共享同一 UV 范围）
3. 同一纹理的所有面合并为 1 个 Mesh（1 个 Draw Call）
4. GPU 采样 `terrain_sampler` atlas，UV 决定具体取哪个子图

这与当前项目 `generate_chunk_mesh` 的实现思路完全一致，只是目前用的是顶点颜色而非纹理。

---

## 7. 从方案 B 迁移到方案 A

### 改动范围

| 层级 | 改动内容 |
|------|---------|
| **纹理加载** | `TextureAtlas` → `ImageArrayLayout` |
| **材质** | `StandardMaterial` → 自定义 `Material` + WGSL |
| **Mesh 生成** | UV 值简化 + 附加 `MeshTag` |
| **着色器** | 新增 `array_texture.wgsl` |

### 具体步骤

#### 1. 纹理加载（Rust）

```rust
// 旧（方案 B）
let texture_handle = asset_server.load("textures/array_texture.png");
let atlas = TextureAtlas::new_empty(texture_handle, Vec2::new(250.0, 1000.0));

// 新（方案 A）
let array_texture = asset_server.load_with_settings(
    "textures/array_texture.png",
    |settings: &mut ImageLoaderSettings| {
        settings.layout = ImageArrayLayout::二维阵列(4); // 4 层
    },
);
```

#### 2. UV 简化（generate_chunk_mesh）

```rust
// 旧（方案 B）：每个顶点计算不同的 UV 偏移
uvs.push([u / 250.0, v / 1000.0]);

// 新（方案 A）：所有顶点 UV 都是整层 [0,1]
uvs.push([0.0, 0.0]); // 左下
uvs.push([1.0, 0.0]); // 右下
uvs.push([1.0, 1.0]); // 右上
uvs.push([0.0, 1.0]); // 左上
```

#### 3. MeshTag 附加

```rust
// 新（方案 A）：为每个 instanced mesh 附加 MeshTag
for (block_type, face_normal) in visible_faces {
    let layer = match block_type {
        BlockType::Grass => 0,
        BlockType::Dirt  => 1,
        BlockType::Stone => 2,
    };
    // 创建 instanced mesh 时指定 layer
    commands.entity(entity).insert(MeshTag(layer));
}
```

#### 4. WGSL Shader（新增）

`assets/shaders/array_texture.wgsl`（见上方 4.2 节完整代码）。

### 迁移成本

- **工作量**：约 2-3 小时
- **风险点**：
  - WGSL 中必须正确 `#import bevy_pbr` 以保留光照计算
  - `MeshTag` 需要在 `mesh.set_indices()` 之后正确附加
- **不可逆性**：无，迁移是纯增量改动

---

## 8. 当前项目状态

| 组件 | 状态 | 说明 |
|------|------|------|
| `array_texture.png` | ❌ 未使用 | 纹理资源存在但未被加载 |
| `generate_chunk_mesh` | ✅ 纯色顶点 | 目前每个 block 类型用不同顶点颜色（草绿/土黄/石头灰） |
| 纹理系统 | ❌ 未实现 | 当前渲染不依赖任何纹理 |
| 计划 | 📋 方案 B 先行 | 先实现 TextureAtlas + UV，保留后期迁移方案 A 的路径 |

---

## 9. 推荐路线

```
当前（纯色顶点）
    ↓
方案 B（TextureAtlas + UV）← 近期实现，约 2-4 小时
    ↓
方案 A（texture_2d_array）← 后期探索，若有动态换肤需求时迁移
```

**理由**：
1. 方案 B 实现成本低，可以快速看到纹理效果
2. 核心 Mesh 生成函数 `generate_chunk_mesh` 两者几乎不变
3. 迁移到方案 A 时，只需改动 UV 计算逻辑 + 新增 shader，Mesh 合并策略完全不变

---

## 10. 参考资料

- [Bevy Array Texture 示例](https://bevy.org/examples/shaders/array-texture/)
- [Bevy Generate Custom Mesh 示例](https://bevy.org/examples/3d-rendering/generate-custom-mesh/)
- [Bevy 官方文档 - Material](https://bevyengine.org/learn/book/materials/)
- [Minecraft Wiki - Texture](https://minecraft.wiki/w/Texture)
- [Minecraft Wiki - Models](https://minecraft.wiki/w/Model)
