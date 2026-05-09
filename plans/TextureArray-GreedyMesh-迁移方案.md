# Texture Array 迁移 + Greedy Meshing 启用方案
参考文档：
/Democode/Array Texture-wgsl.md
/Democode/Array Texture-Demo.md
在线演示：https://bevy.org/examples/shaders/array-texture/
官方API：https://docs.rs/bevy/latest/bevy/
当前Bevy 版本：0.18.1(不可更改)

## 一、问题分析

### 1.1 Greedy Meshing UV 问题的根因

当前网格生成有两个实现：

| 实现 | 位置 | 状态 |
|------|------|------|
| 逐面生成（Naive Meshing） | `chunk.rs:generate_chunk_mesh()` + `async_mesh.rs:generate_chunk_mesh_async()` | ✅ 启用 |
| Greedy Meshing | `greedy_mesh.rs:generate_greedy_mesh()` | ⚠️ 未启用 |

**Greedy Meshing 的 UV 问题**：

```
逐面生成（当前启用）：
  每个 1×1 方块面 → 4 个顶点，UV = 纹理在 Atlas 中的绝对 UV ✓

Greedy Meshing（当前未启用）：
  合并 3×2 方块面 → 4 个顶点（大幅减少）
  UV = 拉伸模式（整个 3×2 区域映射到一个纹理槽位）✗ 纹理被拉伸
```

**根本原因**：当前使用 **Texture Atlas**（所有纹理拼在一张大图），每个纹理的 UV 是 Atlas 中的绝对坐标范围（如 grass_top 在 0.000~0.128）。Greedy Meshing 合并后的大四边形无法在 Atlas 范围内进行纹理平铺——尝试平铺 UV 会跨越到相邻纹理。

### 1.2 Texture Array 方案

**Texture Array（纹理数组）** 是 3D 纹理，每层（layer）是一张完整独立的 2D 纹理：

```
Texture Array:
┌─────────────────────────┐
│ Layer 0: grass_block_top│  ← 32×32，UV [0,1]×[0,1]
├─────────────────────────┤
│ Layer 1: grass_block_side│ ← 32×32，UV [0,1]×[0,1]
├─────────────────────────┤
│ Layer 2: dirt           │  ← 32×32，UV [0,1]×[0,1]
├─────────────────────────┤
│ Layer 3: stone          │  ← 32×32，UV [0,1]×[0,1]
└─────────────────────────┘
```

**优势**：
- 每层 UV 独立为 [0,1]，平铺时不会跨越到其他纹理
- Greedy Meshing 合并 3×2 面 → UV (0,0)~(3,2) + `fract(uv)` → 每个方块正确显示完整纹理
- 天然无 Atlas 边缘渗色（mipmap bleeding）问题
- 无需 padding/出血处理

---

## 二、迁移策略：两阶段增量迁移

为了防止一次性改动太大导致难以排查问题，采用 **两阶段增量迁移**：

```
Phase 1: Atlas → Texture Array（不改网格生成逻辑）
  ┌──────────────────────────────────────────────┐
  │ 目标：Naive Meshing 继续工作，但数据源从      │
  │       Atlas 换为 Texture Array                │
  │ 验证：cargo run → 游戏画面与之前完全一致      │
  └──────────────────────────────────────────────┘
                              ↓
Phase 2: 启用 Greedy Meshing（不改纹理系统）
  ┌──────────────────────────────────────────────┐
  │ 目标：网格生成从 Naive 切换为 Greedy          │
  │ 验证：cargo run → 纹理显示正确 + 顶点数减少   │
  └──────────────────────────────────────────────┘
```

---

## 三、Phase 1 详细步骤 — Atlas → Texture Array

### 参考资源

- Bevy 官方 API 文档（所有相关类型）：[`https://docs.rs/bevy/latest/bevy/`](https://docs.rs/bevy/latest/bevy/)
  - [`Image`](https://docs.rs/bevy/latest/bevy/prelude/struct.Image.html) — 纹理数据容器，支持 `D2Array`
  - [`Extent3d`](https://docs.rs/bevy/latest/bevy/render/render_resource/struct.Extent3d.html) — 图像尺寸 + 层数
  - [`TextureDimension::D2Array`](https://docs.rs/bevy/latest/bevy/render/render_resource/enum.TextureDimension.html) — 2D 纹理数组维度
  - [`Material`](https://docs.rs/bevy/latest/bevy/render/material/trait.Material.html) — 自定义材质 trait
  - [`AsBindGroup`](https://docs.rs/bevy/latest/bevy/render/render_resource/trait.AsBindGroup.html) — 材质绑定组自动推导
  - [`texture_2d_array`](https://docs.rs/bevy/latest/bevy/render/render_resource/attr.AsBindGroup.html#texture-array-support) — `#[texture(0, dimension = "2d_array")]`
- Bevy 官方 ArrayTexture 示例：[`https://bevy.org/examples/shaders/array-texture/`](https://bevy.org/examples/shaders/array-texture/)
  - 展示了自定义材质、WGSL 着色器中采样 `texture_2d_array<f32>`、`fract(uv)` 用法
- 当前项目的 `src/greedy_mesh.rs` — Greedy Meshing 完整实现（~475行）
- 当前项目的 `src/async_mesh.rs` — `UvLookupTable` 结构体（~60-87行）
- 当前项目的 `src/chunk_manager.rs` — `setup_world()` 中 Image 创建（~120-180行）

### Phase 1 的改动原则

每个步骤必须满足：
1. **`cargo check` 编译通过** — 每步做完立即运行
2. **不破坏现有功能** — 新代码和旧代码可以共存
3. **可独立验证** — 每步有明确的验证方式

---

### Step 1.1: 在 `resource_pack.rs` 中添加 `build_texture_array()` 方法

**改动文件**：`src/resource_pack.rs`

**改动内容**：
1. 新增 `TextureArrayInfo` 结构体（与旧的 `TextureAtlas` 共存）
2. 新增 `pub fn build_texture_array(&mut self) -> Result<TextureArrayInfo, String>` 方法
3. `build_texture_array()` 逻辑：
   - 遍历所有已扫描的纹理
   - 确保所有纹理统一为 32×32（否则 resize）
   - 构建 `HashMap<String, u32>` 映射纹理名→层索引
   - 返回 `TextureArrayInfo { layer_count, width, height, texture_layers, pixels }`

```rust
/// 新增结构体，与旧的 TextureAtlas 共存
pub struct TextureArrayInfo {
    pub layer_count: u32,
    pub width: u32,
    pub height: u32,
    pub texture_layers: HashMap<String, u32>, // texture_name → layer_index
    pub pixels: Vec<u8>, // 所有层像素连续排列 [layer0, layer1, ...]
}
```

**为何不删旧代码**：Phase 1 的目标是让新旧共存，等 Phase 1 验证通过后再清理。

**验证方式**：
```
cargo check
```
✅ 编译通过即可。

**参考文档**：
- [`HashMap`](https://docs.rs/bevy/latest/bevy/prelude/struct.HashMap.html) — 存储纹理名到层索引的映射
- `image` crate 的 resize 功能 — 确保所有纹理尺寸统一

---

### Step 1.2: 创建自定义材质 `TextureArrayMaterial`

**新增文件**：`src/texture_array_material.rs`

**改动内容**：
1. 定义 `TextureArrayMaterial` 结构体，使用 `#[texture(0, dimension = "2d_array")]` 绑定纹理数组
2. 实现 `Material` trait，指定 WGSL 着色器路径

```rust
use bevy::{
    prelude::*,
    reflect::TypePath,
    render::render_resource::{AsBindGroup, ShaderRef},
};

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct TextureArrayMaterial {
    #[texture(0, dimension = "2d_array")]
    #[sampler(1)]
    pub texture_array: Handle<Image>,
}

impl Material for TextureArrayMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/texture_array.wgsl".into()
    }

    fn vertex_shader() -> ShaderRef {
        "shaders/texture_array.wgsl".into()
    }
}
```

**验证方式**：
```
cargo check
```
✅ 编译通过即可。

**参考文档**：
- [`Material` trait](https://docs.rs/bevy/latest/bevy/render/material/trait.Material.html) — 自定义材质的核心 trait
- [`AsBindGroup`](https://docs.rs/bevy/latest/bevy/render/render_resource/trait.AsBindGroup.html) — 自动生成绑定组
- [`#[texture]` attribute](https://docs.rs/bevy/latest/bevy/render/render_resource/attr.AsBindGroup.html#texture-array-support) — `dimension = "2d_array"` 参数

---

### Step 1.3: 创建 WGSL 着色器

**新增文件**：`assets/shaders/texture_array.wgsl`

**改动内容**：
1. 顶点着色器：标准 MVP 变换，透传 UV 和 layer_index
2. 片段着色器：使用 `fract(uv)` 实现每方块纹理平铺，采样 `texture_2d_array<f32>`

```wgsl
struct Vertex {
    @builtin(instance_index) instance_index: u32,
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) normal: vec3<f32>,
    @location(3) layer_index: f32,  // 纹理数组层索引（编码为 float）
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) layer_index: f32,
};

@group(2) @binding(0) var texture_array: texture_2d_array<f32>;
@group(2) @binding(1) var texture_sampler: sampler;

@vertex
fn vertex(vertex: Vertex) -> VertexOutput {
    // 标准 MVP 变换（参考 Bevy 内置 mesh.wgsl）
    var out: VertexOutput;
    out.clip_position = bevy_pbr::mesh_functions::mesh_position_local_to_clip(
        bevy_pbr::mesh_functions::get_world_from_local(vertex.instance_index),
        vec4<f32>(vertex.position, 1.0)
    );
    out.uv = vertex.uv;
    out.layer_index = vertex.layer_index;
    return out;
}

@fragment
fn fragment(input: VertexOutput) -> @location(0) vec4<f32> {
    let tiled_uv = fract(input.uv);
    let layer = u32(input.layer_index);
    return textureSampleLevel(
        texture_array,
        texture_sampler,
        tiled_uv,
        layer,
        0.0
    );
}
```

**验证方式**：
```
cargo check
```
✅ 编译通过即可。

**参考文档**：
- Bevy 内置着色器参考：[`mesh.wgsl`](https://github.com/bevyengine/bevy/blob/main/crates/bevy_pbr/src/render/mesh.wgsl) — MVP 变换函数
- Bevy ArrayTexture 示例的 WGSL 着色器：[`https://bevy.org/examples/shaders/array-texture/`](https://bevy.org/examples/shaders/array-texture/) — `fract(uv)` + `textureSampleLevel` 用法
- WGSL `texture_2d_array<f32>` — 纹理数组采样类型

---

### Step 1.4: 在 `main.rs` 注册自定义材质插件 + 模块

**改动文件**：`src/main.rs`

**改动内容**：
1. 添加 `mod texture_array_material;`
2. 添加 `use crate::texture_array_material::TextureArrayMaterial;`
3. 在 `App::new()` 中添加 `.add_plugins(MaterialPlugin::<TextureArrayMaterial>::default())`

```rust
mod texture_array_material;
use crate::texture_array_material::TextureArrayMaterial;

fn main() {
    App::new()
        // ... 现有插件
        .add_plugins(MaterialPlugin::<TextureArrayMaterial>::default())
        // ...
```

**验证方式**：
```
cargo check
```
✅ 编译通过即可。

**参考文档**：
- [`MaterialPlugin`](https://docs.rs/bevy/latest/bevy/render/material/struct.MaterialPlugin.html) — 注册自定义材质的插件

---

### Step 1.5: 修改 `UvLookupTable` → `TextureArrayLookup`

**改动文件**：`src/async_mesh.rs`

**改动内容**：
1. `UvLookupTable` 重命名为 `TextureArrayLookup`
2. 内部数据从 `HashMap<(u8, String), (f32,f32,f32,f32)>` 改为 `HashMap<(u8, String), u32>`（存储 layer_index）
3. `pub fn get_uv()` → `pub fn get_layer_index()` 返回 `u32`
4. `UvLookupTable::from_resource_pack()` 改为从 `TextureArrayInfo` 构建

```rust
pub struct TextureArrayLookup {
    pub block_layer_map: HashMap<(u8, String), u32>,
}

impl TextureArrayLookup {
    pub fn from_texture_array(
        rp: &ResourcePackManager,
        array_info: &TextureArrayInfo,
    ) -> Self {
        let mut map = HashMap::new();
        for (block_id, face, texture_name) in rp.iter_block_textures() {
            if let Some(&layer) = array_info.texture_layers.get(&texture_name) {
                map.insert((block_id, face), layer);
            }
        }
        Self { block_layer_map: map }
    }

    pub fn get_layer_index(&self, block_id: u8, face_name: &str) -> u32 {
        self.block_layer_map
            .get(&(block_id, face_name.to_string()))
            .copied()
            .unwrap_or(0)
    }
}
```

**验证方式**：
```
cargo check
```
✅ 编译通过即可。

**注意**：此步骤会破坏引用 `UvLookupTable` 的代码，需要在 Step 1.6 中修复。

---

### Step 1.6: 更新 `chunk_manager.rs` — 使用 Texture Array + 新材质

**改动文件**：`src/chunk_manager.rs`

**改动内容**：
1. `setup_world()` 中调用 `build_texture_array()` 替代 `build_atlas()`
2. 创建 `Image` 时使用 `TextureDimension::D2Array`，`depth_or_array_layers` 设为层数
3. 创建 `TextureArrayLookup` 替代 `UvLookupTable`
4. `AtlasTextureHandle` 重命名为 `ArrayTextureHandle`（或保留但修改内部类型）
5. `chunk_loader_system()` 中：
   - 材质创建从 `StandardMaterial` 改为 `TextureArrayMaterial`
   - Mesh 添加 `ATTRIBUTE_UV_1` 存储 layer_index
   - 资源引用从 `Assets<StandardMaterial>` 改为 `Assets<TextureArrayMaterial>`

```rust
// setup_world() 中的变更
let array_info = resource_pack.build_texture_array().unwrap();

let size = Extent3d {
    width: TEXTURE_SIZE,      // 32
    height: TEXTURE_SIZE,     // 32
    depth_or_array_layers: array_info.layer_count,
};
let mut image = Image::new(
    size,
    TextureDimension::D2Array,
    array_info.pixels,
    TextureFormat::Rgba8UnormSrgb,
    RenderAssetUsages::default(),
);
image.sampler = ImageSampler::Descriptor(SamplerDescriptor {
    address_mode_u: AddressMode::Repeat,
    address_mode_v: AddressMode::Repeat,
    mipmap_filter: SamplerFilter::Nearest,
    mag_filter: SamplerFilter::Nearest,
    min_filter: SamplerFilter::Nearest,
    ..default()
});
```

```rust
// chunk_loader_system() 中的变更
// Mesh 添加 layer_index 属性
let layer_uvs: Vec<[f32; 2]> = result
    .layer_uvs
    .into_iter()
    .map(|li| [li as f32, 0.0])
    .collect();

mesh.with_inserted_attribute(Mesh::ATTRIBUTE_UV_1, layer_uvs);

// 材质改为 TextureArrayMaterial
let mat_handle = texture_array_materials.add(TextureArrayMaterial {
    texture_array: array_texture_handle.clone(),
});
```

**验证方式**：
```
cargo check
```
✅ 编译通过。

然后：
```
cargo run
```
🎯 **关键验证点**：游戏启动后，地形纹理显示应与之前**完全一致**。因为 Naive Meshing 的每个面仍然是 1×1 方块，UV 从 Atlas 绝对坐标改为 `[0,1]` per-layer，视觉效果应无变化。

**参考文档**：
- [`Image::new()`](https://docs.rs/bevy/latest/bevy/prelude/struct.Image.html#method.new) — 创建纹理
- [`Extent3d`](https://docs.rs/bevy/latest/bevy/render/render_resource/struct.Extent3d.html) — `depth_or_array_layers` 字段
- [`TextureDimension::D2Array`](https://docs.rs/bevy/latest/bevy/render/render_resource/enum.TextureDimension.html) — 2D 纹理数组枚举值
- [`AddressMode::Repeat`](https://docs.rs/bevy/latest/bevy/render/render_resource/enum.AddressMode.html) — UV 平铺模式
- [`SamplerDescriptor`](https://docs.rs/bevy/latest/bevy/render/render_resource/struct.SamplerDescriptor.html) — 采样器配置

---

### Step 1.7: 更新 `chunk_dirty.rs` — 材质类型替换

**改动文件**：`src/chunk_dirty.rs`

**改动内容**：
1. `ChunkMeshHandle.material` 类型从 `Handle<StandardMaterial>` → `Handle<TextureArrayMaterial>`
2. `rebuild_dirty_chunks()` 中 `ResMut<Assets<StandardMaterial>>` → `ResMut<Assets<TextureArrayMaterial>>`
3. 同样添加 `ATTRIBUTE_UV_1` 到 Mesh

**验证方式**：
```
cargo check
```
✅ 编译通过。

---

### Step 1.8: 更新 `chunk.rs` — `spawn_chunk_entity()` 适配

**改动文件**：`src/chunk.rs`

**改动内容**：
1. `spawn_chunk_entity()` 的参数/内部逻辑更新为使用 `TextureArrayMaterial`
2. 添加 `layer_uvs` 到生成的 Mesh

**验证方式**：
```
cargo check
```
✅ 编译通过。

---

### Step 1.9: 更新 `MeshResult` 添加 `layer_uvs`

**改动文件**：`src/async_mesh.rs`

**改动内容**：
1. `MeshResult` 结构体添加 `pub layer_uvs: Vec<[f32; 2]>` 字段
2. `generate_chunk_mesh_async()` 中为每个顶点生成 layer_index 数据

**验证方式**：
```
cargo check
```
✅ 编译通过。

---

### Phase 1 最终验证

```bash
cargo check     # 无编译错误
cargo run       # 游戏启动，纹理显示与之前完全一致
```

如果画面不对，排查方向：
1. Image 的像素数据排列是否正确（Rgba8UnormSrgb，每层 32×32×4 字节）
2. Sampler 是否设置为 Repeat 模式
3. WGSL 着色器中 `fract(uv)` 是否正确
4. layer_index 是否传递正确

---

## 四、Phase 2 详细步骤 — 启用 Greedy Meshing

Phase 2 的前提是 Phase 1 已验证通过（纹理数组正常工作）。

### Step 2.1: 修改 `greedy_mesh.rs` — UV 改为平铺模式

**改动文件**：`src/greedy_mesh.rs`

**改动内容**：
1. 将拉伸 UV 改为平铺 UV：`(0,0)-(W,H)`
2. 添加 `layer_uvs: Vec<[f32; 2]>` 到 `GreedyMeshResult`
3. 每个顶点添加 layer_index（通过 `get_layer_index` 回调）

```rust
// 修改 generate_greedy_mesh 函数签名
pub fn generate_greedy_mesh<F>(
    chunk: &ChunkData,
    neighbors: &ChunkNeighbors,
    get_layer_index: F,  // 原来: get_uv: F
) -> GreedyMeshResult
where
    F: Fn(u8, &str) -> u32 + Send + Sync,  // 原来: Fn(u8, &str) -> (f32,f32,f32,f32)
```

**验证方式**：
```
cargo check
```
✅ 编译通过即可（此时 Greedy Meshing 仍未启用）。

---

### Step 2.2: 在 `chunk_manager.rs` 中启用 Greedy Meshing

**改动文件**：`src/chunk_manager.rs` — `chunk_loader_system()`

**改动内容**：
1. 在异步网格生成任务中，选择使用 `generate_greedy_mesh` 替代 `generate_chunk_mesh_async`
2. 或者添加一个配置开关（如 `USE_GREEDY_MESHING: bool`），方便切回 Naive

```rust
// 在 chunk_loader_system 中
const USE_GREEDY: bool = true;

// 异步任务调用
let result = if USE_GREEDY {
    // Greedy Meshing
    let greedy_result = generate_greedy_mesh(&data, &neighbors, |block_id, face| {
        uv_table.get_layer_index(block_id, face)
    });
    // 将 GreedyMeshResult 转换为 MeshResult
    MeshResult {
        coord,
        positions: greedy_result.positions,
        uvs: greedy_result.uvs,
        normals: greedy_result.normals,
        indices: greedy_result.indices,
        layer_uvs: greedy_result.layer_uvs,
    }
} else {
    // Naive Meshing（保留作为回退）
    generate_chunk_mesh_async(&data, &neighbors, &uv_table, coord)
};
```

**验证方式**：
```
cargo check
cargo run
```

🎯 **关键验证点**：
1. 纹理显示是否正确——每个合并面应该显示完整、平铺的纹理，而非拉伸
2. 顶点数是否减少——可通过 `perf_logger.rs` 或控制台输出观察
3. 性能是否提升——帧率应上升

---

### Step 2.3: 清理旧代码（可选）

**改动文件**：多个文件

**改动内容**：
1. 删除 `resource_pack.rs` 中的 `build_atlas()` 和 `TextureAtlas`（如果不再需要）
2. 删除 `chunk.rs` 中的旧 `generate_chunk_mesh()`
3. 删除 `async_mesh.rs` 中的旧 `generate_chunk_mesh_async()`（如果不再使用）

**验证方式**：
```
cargo check
cargo run
```

---

## 五、关键设计决策

### 5.1 为什么用 `ATTRIBUTE_UV_1` 而不是编码到 UV 的 w 分量？

| 方式 | 优点 | 缺点 |
|------|------|------|
| **`ATTRIBUTE_UV_1`（推荐）** | 语义清晰；着色器端直接读取 `location(1)` | 多一个顶点属性（额外 8 字节/顶点） |
| 编码到 UV w 分量 | 减少属性数量 | 需要自定义 `VertexAttributeLayout`；不直观 |

推荐使用 `ATTRIBUTE_UV_1`，因为 Bevy 原生支持多个 UV 属性，且语义更清晰。

### 5.2 Sampler 为什么用 Nearest 滤波？

当前项目使用像素风格纹理（每个方块 32×32），Nearest 滤波可以保持像素清晰，避免模糊。

### 5.3 Naive Meshing 和 Greedy Meshing 如何共存？

在异步 mesh 生成中，可以保留 Naive 作为回退方案：

```rust
const MESHING_MODE: MeshingMode = MeshingMode::Greedy; // 或 MeshingMode::Naive

enum MeshingMode {
    Naive,
    Greedy,
}
```

这样在调试时可以随时切换比较效果。

---

## 六、风险与缓解

| 风险 | 概率 | 影响 | 缓解措施 |
|------|------|------|----------|
| Bevy `D2Array` Image 支持不完整 | 低 | 高 | 先做 Step 1.6 原型验证；回退到 Atlas + padding 方案 |
| WGSL 自定义着色器 MVP 矩阵问题 | 中 | 中 | 参考 Bevy 内置 `mesh.wgsl` 的 `mesh_position_local_to_clip` 函数 |
| 自定义材质不兼容现有灯光 | 中 | 中 | 使用 Unlit 或参照 Bevy ArrayTexture 示例的做法 |
| 性能回退（纹理数组采样 vs Atlas） | 低 | 低 | Texture Array 采样性能与普通 2D 纹理相当 |
| Phase 1 画面显示异常 | 中 | 高 | 保留旧实现，可快速切回；检查像素数据排列顺序 |

---

## 七、验收标准

### Phase 1 验收

- [x] `cargo check` 无编译错误
- [x] `cargo run` 游戏正常启动
- [x] 地形纹理显示与之前完全一致（Naive Meshing + Texture Array）
- [x] 方块放置/破坏正确重建网格

### Phase 2 验收

- [x] Greedy Meshing 正确平铺纹理（3×2 合并面显示 3×2 个完整纹理）
- [x] 顶点数减少 70-80%（可通过 `perf_logger` 或控制台输出验证）
- [x] 性能不低于当前 Atlas + Naive 方案
- [x] 可随时切回 Naive Meshing 调试

---

## 八、文件变更总览

### 新增文件

| 文件 | 内容 | 行数估计 |
|------|------|----------|
| `src/texture_array_material.rs` | `TextureArrayMaterial` 自定义材质定义 | ~40 行 |
| `assets/shaders/texture_array.wgsl` | WGSL 顶点/片段着色器 | ~80 行 |

### 修改文件（按实施顺序）

| 步骤 | 文件 | 改动 | 行数 |
|------|------|------|------|
| 1.1 | `src/resource_pack.rs` | 新增 `build_texture_array()` + `TextureArrayInfo` | ~80 行 |
| 1.5 | `src/async_mesh.rs` | `UvLookupTable` → `TextureArrayLookup` + `get_layer_index()` | ~30 行 |
| 1.6 | `src/chunk_manager.rs` | Image D2Array + 新材质系统 + UV_1 属性 | ~60 行 |
| 1.7 | `src/chunk_dirty.rs` | 材质类型替换 + UV_1 属性 | ~20 行 |
| 1.8 | `src/chunk.rs` | `spawn_chunk_entity()` 适配 | ~15 行 |
| 1.9 | `src/async_mesh.rs` | `MeshResult` 添加 `layer_uvs` | ~10 行 |
| 1.4 | `src/main.rs` | 注册材质插件 | ~5 行 |
| 2.1 | `src/greedy_mesh.rs` | UV 平铺模式 + `layer_uvs` | ~40 行 |
| 2.2 | `src/chunk_manager.rs` | 启用 Greedy Meshing | ~20 行 |

---

## 九、总结

本方案通过 **两阶段增量迁移** 实现从 Texture Atlas 到 Texture Array 的转换，最终启用 Greedy Meshing：

1. **Phase 1**（Step 1.1~1.9）：不改变网格生成逻辑，只替换纹理数据源和渲染管线。每步都可 `cargo check` 验证，最终 `cargo run` 画面应与之前一致。
2. **Phase 2**（Step 2.1~2.3）：在 Phase 1 基础上启用 Greedy Meshing，利用 Texture Array 的独立 UV 空间实现正确平铺。

整个过程中，每个步骤都是**独立可运行验证**的原子改动，可以随时 `cargo check` 或 `cargo run` 确认状态。
