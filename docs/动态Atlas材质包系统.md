# 动态 Atlas + 材质包系统设计方案

> 本文档描述未来如何实现像 Minecraft 那样的运行时动态 Atlas 构建和材质包热加载。
> 当前 Plan B 为固定 UV 映射，本方案是其演进方向。

---

## 1. 目标

- **动态 Atlas 构建**：启动时自动扫描所有材质文件，拼成一张大图集，无需手动排列
- **材质包热切换**：运行时可替换材质包，Atlas 自动重建，视觉效果即时更新
- **block model JSON**：类似 Minecraft 的 blockstates 文件，用 JSON 声明方块各面引用的纹理名称
- **UV 自动查表**：运行时查表 texture name → atlas UV，无需硬编码 slot

---

## 1.1 当前现状分析（硬编码问题）

当前材质面**完全硬编码**在代码中，体现在两个层面：

### 1.1.1 `atlas.rs` — UV 槽位硬编码

- `grass::TOP`、`dirt::RIGHT`、`stone::TOP` 等都是 `const` 常量，`col`/`row` 写死
- `AtlasSlot::uv()` 基于固定的 `ATLAS_WIDTH_PX`/`ATLAS_HEIGHT_PX` 计算 UV
- 新增纹理必须手动调整行列坐标并修改代码

```rust
// 当前硬编码示例
pub mod grass {
    pub const TOP: AtlasSlot = AtlasSlot { col: 0, row: 0 };
    pub const BOTTOM: AtlasSlot = AtlasSlot { col: 0, row: 1 };
    pub const RIGHT: AtlasSlot = AtlasSlot { col: 1, row: 0 };
    // ...
}
```

### 1.1.2 `chunk.rs` — 方块→纹理映射硬编码

- `BlockTexture::from_block_and_face()` 用 `match` 将 `block_id` + `Face` 映射到固定的 `BlockTexture` 枚举
- `BlockTexture::atlas_slot()` 再 `match` 到 `atlas.rs` 中的硬编码常量
- `face_quad()` 通过这条链路获取 UV，写入 mesh

```rust
// 当前硬编码链路
block_id + Face → BlockTexture枚举 → 硬编码AtlasSlot → UV
```

### 1.1.3 问题总结

| 问题 | 影响 |
|------|------|
| 新增纹理需改代码 | 每次添加新方块/纹理都要修改 `atlas.rs` 和 `chunk.rs` |
| 无法运行时切换 | 材质包概念不存在，所有纹理在编译时确定 |
| Atlas 尺寸固定 | 250×1000 像素写死，扩展困难 |
| 纹理命名缺失 | 用 `col/row` 坐标而非语义化名称，难以维护 |

---

## 2. Minecraft 1.13+ 的实现参考

### 2.1 动态 Atlas 构建

```
启动流程：
1. 扫描 assets/textures/ 目录，找出所有 .png 文件
2. 扫描 blockstates/*.json，找出所有引用的 texture name
3. 将所有用到的 texture 加载为 Image
4. 排列到一张大图集上（类似 bin-packing 算法）
5. 记录每个 texture name → UV 坐标的映射表
```

### 2.2 Block Model JSON

```json
{
  "textures": {
    "top": "block/grass_top",
    "side": "block/grass_side",
    "bottom": "block/dirt"
  },
  "elements": [
    {
      "from": [0, 0, 0],
      "to": [16, 16, 16],
      "faces": {
        "up":    { "texture": "#top" },
        "down":  { "texture": "#bottom" },
        "north": { "texture": "#side" },
        "south": { "texture": "#side" },
        "west":  { "texture": "#side" },
        "east":  { "texture": "#side" }
      }
    }
  ]
}
```

`#top` 表示引用 `textures.top`，渲染时解析为实际 UV 坐标。

### 2.3 材质包热切换

```
切换材质包：
1. 收到切换事件
2. 释放旧 Atlas Image
3. 扫描新材质包目录
4. 重建 Atlas
5. 更新映射表
6. 标记所有 Chunk 实体脏，下次重建 mesh 时自动使用新 UV
```

### 2.4 纹理图集与单独 PNG 的两阶段设计

Minecraft 的纹理系统采用**两阶段设计**，存储和渲染使用不同的形式：

#### 阶段一：存储时 → 单独的 PNG 文件

在材质包（Resource Pack）中，纹理以**单独的 PNG 文件**存储：

```
assets/minecraft/textures/block/
├── grass_top.png      ← 16×16 像素
├── grass_side.png     ← 16×16 像素
├── dirt.png           ← 16×16 像素
├── stone.png          ← 16×16 像素
├── oak_log_top.png    ← 16×16 像素
├── oak_log_side.png   ← 16×16 像素
└── ... (数百个小文件)
```

这是**材质包作者看到和编辑的形式**，便于管理、替换、分发。

#### 阶段二：运行时 → 动态拼接成一张大图集

游戏启动时，引擎自动执行：

```
数百个单独的 PNG 文件
        ↓  （加载所有图片）
        ↓  （bin-packing 算法排列）
        ↓  （拼接到一张大图上）
┌─────────────────────────────────────┐
│           运行时 Atlas 图集          │
│  ┌──────┐ ┌──────┐ ┌──────┐        │
│  │grass │ │grass │ │ dirt │  ...    │
│  │_top  │ │_side │ │      │        │
│  └──────┘ └──────┘ └──────┘        │
└─────────────────────────────────────┘
        ↓
   上传到 GPU，作为一张纹理使用
```

#### 为什么要这样做？

| 方面 | 单独 PNG（存储） | 图集（渲染） |
|------|-----------------|-------------|
| **目的** | 便于管理、编辑、替换 | 减少 GPU 绘制调用 |
| **谁使用** | 材质包作者、文件系统 | GPU、渲染管线 |
| **数量** | 数百个小文件 | 1 张大图 |
| **切换成本** | 替换文件即可 | 需要重建图集 |

**核心原因**：GPU 每次切换纹理（bind texture）都有开销。如果每个方块面用单独的纹理，渲染一个 chunk 可能需要切换几百次纹理。而用图集的话，**整个 chunk 只需绑定一次纹理**，所有面的 UV 坐标指向同一张大图的不同区域。

#### 与当前项目的对比

当前项目 `assets/textures/array_texture.png` 是**手动预先拼好的图集**，相当于跳过了"单独 PNG → 动态拼接"这一步，直接把最终结果写死了。实现材质包系统后，这个手动拼接的过程会被自动化，用户只需提供单独的 PNG 文件即可。

### 2.5 纹理映射链：从方块 ID 到 UV 坐标

Minecraft **不是**通过"图集中的位置"来判断哪个纹理对应哪个方块，而是通过**多层 JSON 文件的引用链**来建立映射关系：

#### 完整查询链

```
方块 ID (如 grass_block = 9)
    ↓
blockstates/grass_block.json → 模型 "block/grass_block"
    ↓
models/block/grass_block.json → 面定义
    ↓  例：north 面 → "#side" → "block/grass_side"
    ↓
Atlas 映射表查询 → "block/grass_side" → UV (0.25, 0.0, 0.5, 0.25)
    ↓
写入 mesh 的 UV 坐标
```

#### 四层架构

| 层次 | 文件 | 作用 |
|------|------|------|
| **物理层** | `textures/block/*.png` | 纹理图片文件，ID = 相对路径 |
| **模型层** | `models/block/*.json` | 定义"哪个面用哪个纹理"（通过局部变量引用） |
| **注册层** | `blockstates/*.json` | 定义"哪个方块用哪个模型"（支持变体，如朝向） |
| **渲染层** | 运行时 Atlas | bin-packing 拼图 + HashMap 查表 |

#### 关键设计思想

Minecraft 从不关心纹理在图集中的物理位置（行列坐标），完全通过**语义化的字符串 ID**（如 `block/grass_top`）来建立映射。图集的排列方式可以随意变化，只要映射表正确，渲染就不会出错。这正是材质包能热替换的根本原因——替换 `.png` 文件后重建 Atlas，映射表自动更新。

---

## 3. 核心数据结构设计

### 3.1 TextureAtlas 元数据

```rust
/// 单个纹理的元数据
struct TextureEntry {
    name: String,          // e.g. "block/grass_top"
    atlas_rect: Rect,       // 在 atlas 中的像素区域 (x, y, w, h)
    uv_min: Vec2,           // UV 左下角 (u0, v0)
    uv_max: Vec2,           // UV 右上角 (u1, v1)
}

/// 动态 Atlas 管理器
struct DynamicAtlas {
    image: Handle<Image>,   // 实际的大图集纹理
    entries: HashMap<String, TextureEntry>, // name → UV 映射
    width: u32,
    height: u32,
}
```

### 3.2 Block Model

```rust
/// Block Model 定义（类似 Minecraft blockstates）
struct BlockModel {
    textures: HashMap<String, String>, // 局部变量名 → 实际 texture name
    elements: Vec<MeshElement>,
}

/// 单个元素（一个立方体或部分立方体）
struct MeshElement {
    from: Vec3,
    to: Vec3,
    faces: HashMap<Face, FaceDefinition>,
}

/// 面定义
struct FaceDefinition {
    texture: String,         // 引用 #变量名 或直接是 texture name
    uv: Option<[f32; 4]>,   // 可选，手动指定 UV，覆盖默认
    cullface: Option<Face>,  // 可选，指定面剔除方向
}
```

### 3.3 Block ID → Block Model 映射

```rust
/// BlockStates 注册表
struct BlockRegistry {
    blocks: HashMap<BlockId, BlockModel>,
    default_model: BlockModel,
}
```

---

## 4. 实现步骤（后期）

### Phase 1: 动态 Atlas 构建器

- [ ] `atlas_builder.rs` 模块
- [ ] 实现 bin-packing 排列算法（按面积/边长排序）
- [ ] 扫描指定目录生成 `DynamicAtlas`
- [ ] 提供 `query_uv(texture_name) -> (u0, v0, u1, v1)` 查询接口

### Phase 2: Block Model JSON 解析

- [ ] `block_model.rs` 模块
- [ ] 解析 `blockstates/*.json` 文件
- [ ] 实现 `#variable` 引用解析
- [ ] `BlockRegistry` 注册表管理

### Phase 3: 渲染系统集成

- [ ] 替换当前 `face_quad` 中的硬编码 UV 逻辑
- [ ] 改为 `atlas.query_uv(texture_name)` 动态查询
- [ ] 支持 UV 手动覆盖（JSON 中的 `uv` 字段）

#### 3.1 渲染系统改造细节

**当前 UV 查询链路（硬编码）：**
```
block_id + Face
    ↓
BlockTexture::from_block_and_face()  // chunk.rs 中的 match 硬编码
    ↓
BlockTexture::atlas_slot()           // 返回硬编码的 AtlasSlot 常量
    ↓
AtlasSlot::uv()                      // 基于固定尺寸计算 UV
    ↓
face_quad() 写入 mesh
```

**改造后 UV 查询链路（动态）：**
```
block_id
    ↓
BlockRegistry.get(block_id)          // 查表获取 BlockModel
    ↓
BlockModel.resolve_face(face)        // 解析面纹理名（处理 #variable 引用）
    ↓
DynamicAtlas.query_uv(texture_name)  // 动态查询 UV 坐标
    ↓
face_quad() 写入 mesh
```

**具体代码改造：**

1. **修改 `face_quad()` 函数签名**：
```rust
// 改造前
fn face_quad(x: usize, y: usize, z: usize, face: Face, block_id: BlockId)
    -> ([[f32; 3]; 4], [[f32; 2]; 4], [f32; 3])

// 改造后
fn face_quad(
    x: usize,
    y: usize,
    z: usize,
    face: Face,
    block_id: BlockId,
    atlas: &DynamicAtlas,        // 新增：动态 Atlas 资源
    registry: &BlockRegistry,    // 新增：方块注册表
) -> ([[f32; 3]; 4], [[f32; 2]; 4], [f32; 3])
```

2. **替换 UV 计算逻辑**：
```rust
// 改造前（chunk.rs 第 322-328 行）
let tex = BlockTexture::from_block_and_face(block_id, face);
let (u0, u1, v0, v1) = {
    let slot = tex.atlas_slot();
    let (u0, u1, v0, v1) = slot.uv();
    (u0, u1, v0, v1)
};

// 改造后
let texture_name = registry.resolve_texture(block_id, face);
let (u0, u1, v0, v1) = atlas.query_uv(&texture_name)
    .unwrap_or(atlas.query_uv("missing").unwrap()); // 回退到 missing 纹理
```

3. **删除硬编码模块**：
- 删除 `atlas.rs` 中的 `grass`、`dirt`、`stone` 模块
- 删除 `chunk.rs` 中的 `BlockTexture` 枚举及其方法
- 保留 `AtlasSlot` 结构体（可能需要改造为动态版本）

4. **修改 mesh 构建系统**：
```rust
// chunk.rs 中的 build_mesh 函数需要接收 DynamicAtlas 和 BlockRegistry
pub fn build_mesh(
    chunk_data: &ChunkData,
    atlas: &DynamicAtlas,
    registry: &BlockRegistry,
) -> Mesh {
    // ...
    for each visible face {
        let (verts, uvs, normal) = face_quad(x, y, z, face, block_id, atlas, registry);
        // ...
    }
}
```

### Phase 4: 材质包热切换

- [ ] `MaterialPack` 资源管理
- [ ] 切换时重建 Atlas
- [ ] 脏标记联动：所有 Chunk 自动触发 mesh 重建

### Phase 5: 默认材质

- [ ] 内置默认材质包（类似 Minecraft 的默认资源包）
- [ ] 用户材质包覆盖默认

---

## 5. 优势

| | 当前 Plan B（固定 UV） | 动态 Atlas |
|---|---|---|
| 新增纹理 | 手动重排 atlas + 改代码 | 丢文件 + 写 JSON |
| 材质包 | 需要完整重做 atlas | 替换文件即可 |
| 多人定制 | 每个客户端需同步 atlas | JSON + 材质分离 |
| 实现复杂度 | 低 | 中高 |

---

## 6. 关键设计要点

### 6.1 纹理命名规范

统一使用路径式命名，而非行列坐标：

```
✅ block/grass_top
✅ block/grass_side
✅ block/dirt
✅ block/stone
❌ col:0, row:0
❌ col:1, row:0
```

### 6.2 Atlas 尺寸自适应

根据纹理数量动态计算图集大小，不再固定 250×1000：

```rust
// 启动时计算最优 Atlas 尺寸
let total_pixels = textures.iter().map(|t| t.width * t.height).sum::<u32>();
let side_length = (total_pixels as f32).sqrt().ceil() as u32;
// 对齐到 2 的幂次（GPU 友好）
let atlas_size = side_length.next_power_of_two();
```

### 6.3 Bevy Asset 系统集成

利用 Bevy 的 `AssetServer` 加载纹理，`DynamicAtlas` 作为 Bevy `Resource`：

```rust
#[derive(Resource)]
pub struct DynamicAtlas {
    pub image: Handle<Image>,
    pub entries: HashMap<String, TextureEntry>,
    pub width: u32,
    pub height: u32,
}
```

### 6.4 脏标记复用

复用现有的 chunk dirty 机制，材质包切换时批量标记所有 chunk：

```rust
// 材质包切换事件处理
fn on_material_pack_change(
    mut events: EventReader<MaterialPackChangeEvent>,
    mut chunk_query: Query<&mut ChunkDirty>,
) {
    for _ in events.read() {
        // 标记所有 chunk 为脏
        for mut dirty in chunk_query.iter_mut() {
            dirty.0 = true;
        }
    }
}
```

### 6.5 向后兼容

Phase 1-3 完成后，当前的视觉效果应完全不变，只是数据来源从硬编码变为 JSON + 动态查表：

- 保留当前的 `array_texture.png` 作为默认材质包
- 创建对应的 `blockstates/*.json` 文件
- 确保 UV 计算结果与硬编码版本一致

### 6.6 回退机制

当纹理缺失时，回退到 `missing` 纹理（紫黑棋盘格）：

```rust
let texture_name = registry.resolve_texture(block_id, face);
let (u0, u1, v0, v1) = atlas.query_uv(&texture_name)
    .unwrap_or_else(|| {
        warn!("Missing texture: {}", texture_name);
        atlas.query_uv("missing").unwrap()
    });
```

---

## 7. 参考资料

- Minecraft Wiki - Resource Pack：https://minecraft.wiki/w/Resource_Pack
- Minecraft Wiki - Models：https://minecraft.wiki/w/Model
- Bevy Asset Server 文档：Bevy 内置资产系统
- bin-packing 算法：First Fit Decreasing (FFD) 或 Skyline algorithm
