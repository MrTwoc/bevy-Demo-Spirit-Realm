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

## 6. 参考资料

- Minecraft Wiki - Resource Pack：https://minecraft.wiki/w/Resource_Pack
- Minecraft Wiki - Models：https://minecraft.wiki/w/Model
- Bevy Asset Server 文档：Bevy 内置资产系统
- bin-packing 算法：First Fit Decreasing (FFD) 或 Skyline algorithm
