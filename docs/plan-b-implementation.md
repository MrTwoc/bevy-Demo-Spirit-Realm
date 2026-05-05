# 方案 B 实现文档 — TextureAtlas + UV 纹理映射

> 本文档是 `docs/texture-implementation.md` 中**方案 B**的完整实施指南，详细说明每一步的改动点、代码实现和注意事项。

---

## 1. 概述

### 1.1 目标

将 `generate_chunk_mesh` 从顶点颜色方案切换到 **TextureAtlas + UV** 纹理映射方案：
- 每个面根据方块类型（grass/dirt/stone）和面方向（top/side/bottom）映射到 `array_texture.png` 中对应区域的纹理
- 渲染材质从 `StandardMaterial` + 顶点颜色改为 `StandardMaterial` + `base_color_texture`
- Draw Call 数量不变（每 Chunk 1 个）

### 1.2 纹理资源规格

文件：`assets/textures/array_texture.png`
尺寸：250 × 1000 像素

布局规划（水平排列，每子图 32×32 像素）：

```
atlas 布局（250 宽 x 1000 高，子图 32x32，共 4 行 7 列 = 28 个槽位）

        u=0.0                          u=1.0
   v=0.0 ┌─────┬─────┬─────┬─────┬─────┬─────┬─────┐
         │ 0,0 │ 1,0 │ 2,0 │ 3,0 │ 4,0 │ 5,0 │ 6,0 │  ← row 0
         ├─────┼─────┼─────┼─────┼─────┼─────┼─────┤
   v≈0.25 │ 0,1 │ 1,1 │ 2,1 │ 3,1 │ 4,1 │ 5,1 │ 6,1 │  ← row 1
         ├─────┼─────┼─────┼─────┼─────┼─────┼─────┤
   v≈0.50 │ 0,2 │ 1,2 │ 2,2 │ 3,2 │ 4,2 │ 5,2 │ 6,2 │  ← row 2
         ├─────┼─────┼─────┼─────┼─────┼─────┼─────┤
   v≈0.75 │ 0,3 │ 1,3 │ 2,3 │ 3,3 │ 4,3 │ 5,3 │ 6,3 │  ← row 3
         └─────┴─────┴─────┴─────┴─────┴─────┴─────┘

实际使用槽位（按 block_type × face 组合）：
  grass_top    → slot (0, 0)  grass_side   → slot (1, 0)
  dirt_top     → slot (0, 1)  dirt_side    → slot (1, 1)
  stone        → slot (2, 0)
```

### 1.3 UV 计算规则

每个子图 UV 范围：

```
slot (col, row)，子图尺寸 = 32px，atlas 尺寸 = 250 × 1000

u_min = (col * 32) / 250
u_max = ((col + 1) * 32) / 250
v_min = (row * 32) / 1000
v_max = ((row + 1) * 32) / 1000
```

实际值（32×32 子图，250×1000 atlas）：

| Slot | 纹理 | u_min | u_max | v_min | v_max |
|------|------|-------|-------|-------|-------|
| (0,0) | grass_top | 0.000 | 0.128 | 0.000 | 0.032 |
| (1,0) | grass_side | 0.128 | 0.256 | 0.000 | 0.032 |
| (0,1) | dirt | 0.000 | 0.128 | 0.032 | 0.064 |
| (1,1) | dirt_side | 0.128 | 0.256 | 0.032 | 0.064 |
| (2,0) | stone | 0.256 | 0.384 | 0.000 | 0.032 |

### 1.4 文件变更清单

```
src/
  atlas.rs          [新增] — UV 计算常量、AtlasSlot 定义、BlockTexture 枚举
  chunk.rs          [修改] — generate_chunk_mesh 中 UV 生成逻辑
  main.rs           [修改] — AssetServer 加载纹理

assets/
  textures/
    array_texture.png  [已有，使用之]
```

---

## 2. 新增模块 — `src/atlas.rs`

### 2.1 模块职责

- 定义 `AtlasSlot`（atlas 中的行列位置）
- 定义 `BlockTexture` 枚举（每个 BlockId × Face 组合对应的 slot）
- 提供 `atlas_slot_uv(slot, atlas_w, atlas_h, tile_px)` 计算任意 slot 的 UV 范围
- 提供 `TILE_SIZE` 常量（每个子图的像素尺寸 = 32）

### 2.2 代码实现

```rust
//! Texture Atlas — UV 计算和纹理槽位定义
//!
//! array_texture.png 布局：250×1000 像素，子图 32×32 像素，水平排列。
//! Slot (col, row) 表示第 col 列、第 row 行的子图区域。

/// 子图像素尺寸（长和宽均为 32px）
pub const TILE_SIZE_PX: u32 = 32;

/// Atlas 图片尺寸
pub const ATLAS_WIDTH_PX: f32 = 250.0;
pub const ATLAS_HEIGHT_PX: f32 = 1000.0;

/// Atlas 中的一个槽位（子图位置）
#[derive(Clone, Copy, Debug)]
pub struct AtlasSlot {
    pub col: u32,
    pub row: u32,
}

impl AtlasSlot {
    /// 从行列计算 UV 范围（返回 u_min, u_max, v_min, v_max）
    pub fn uv(&self) -> (f32, f32, f32, f32) {
        let u_min = (self.col * TILE_SIZE_PX) as f32 / ATLAS_WIDTH_PX;
        let u_max = ((self.col + 1) * TILE_SIZE_PX) as f32 / ATLAS_WIDTH_PX;
        let v_min = (self.row * TILE_SIZE_PX) as f32 / ATLAS_HEIGHT_PX;
        let v_max = ((self.row + 1) * TILE_SIZE_PX) as f32 / ATLAS_HEIGHT_PX;
        (u_min, u_max, v_min, v_max)
    }
}

/// 草方块的 6 个面各自使用的 atlas slot
pub mod grass {
    use super::AtlasSlot;
    pub const TOP:    AtlasSlot = AtlasSlot { col: 0, row: 0 }; // 草地顶
    pub const BOTTOM: AtlasSlot = AtlasSlot { col: 0, row: 1 }; // 泥土（底面用土）
    pub const RIGHT:  AtlasSlot = AtlasSlot { col: 1, row: 0 }; // 草土侧面
    pub const LEFT:   AtlasSlot = AtlasSlot { col: 1, row: 0 }; // 草土侧面
    pub const FRONT:  AtlasSlot = AtlasSlot { col: 1, row: 0 }; // 草土侧面
    pub const BACK:   AtlasSlot = AtlasSlot { col: 1, row: 0 }; // 草土侧面
}

/// 泥土方块的 6 个面
pub mod dirt {
    use super::AtlasSlot;
    pub const TOP:    AtlasSlot = AtlasSlot { col: 0, row: 1 }; // 土顶
    pub const BOTTOM: AtlasSlot = AtlasSlot { col: 0, row: 1 }; // 土底
    pub const RIGHT:  AtlasSlot = AtlasSlot { col: 1, row: 1 }; // 土侧
    pub const LEFT:   AtlasSlot = AtlasSlot { col: 1, row: 1 }; // 土侧
    pub const FRONT:  AtlasSlot = AtlasSlot { col: 1, row: 1 }; // 土侧
    pub const BACK:   AtlasSlot = AtlasSlot { col: 1, row: 1 }; // 土侧
}

/// 石头方块的 6 个面
pub mod stone {
    use super::AtlasSlot;
    pub const TOP:    AtlasSlot = AtlasSlot { col: 2, row: 0 }; // 石顶
    pub const BOTTOM: AtlasSlot = AtlasSlot { col: 2, row: 0 }; // 石底
    pub const RIGHT:  AtlasSlot = AtlasSlot { col: 2, row: 0 }; // 石侧
    pub const LEFT:   AtlasSlot = AtlasSlot { col: 2, row: 0 }; // 石侧
    pub const FRONT:  AtlasSlot = AtlasSlot { col: 2, row: 0 }; // 石侧
    pub const BACK:   AtlasSlot = AtlasSlot { col: 2, row: 0 }; // 石侧
}
```

### 2.3 更新 `src/lib.rs`（如果是 lib 项目）或在 `main.rs` 添加 `mod atlas;`

在 `main.rs` 顶部添加：

```rust
mod atlas;
```

---

## 3. 修改 `src/chunk.rs`

### 3.1 顶部新增 import

```rust
use crate::atlas::{self, grass, dirt, stone};
```

### 3.2 新增 `BlockTexture` 枚举

在 `chunk.rs` 的 `BlockId` 定义附近添加：

```rust
/// Block type + face direction → which atlas slot to use.
#[derive(Clone, Copy)]
enum BlockTexture {
    GrassTop,
    GrassSide,
    GrassBottom,  // dirt texture
    DirtTop,
    DirtSide,
    Stone,
}

impl BlockTexture {
    fn from_block_and_face(block_id: BlockId, face: Face) -> Self {
        match block_id {
            1 => match face {          // grass
                Face::Top    => BlockTexture::GrassTop,
                Face::Bottom=> BlockTexture::GrassBottom,
                _           => BlockTexture::GrassSide,
            },
            2 => BlockTexture::Stone,  // stone (all faces same)
            3 => match face {          // dirt
                Face::Top | Face::Bottom => BlockTexture::DirtTop,
                _                       => BlockTexture::DirtSide,
            },
            _ => BlockTexture::Stone,
        }
    }

    fn atlas_slot(&self) -> atlas::AtlasSlot {
        match self {
            BlockTexture::GrassTop    => grass::TOP,
            BlockTexture::GrassSide    => grass::RIGHT,
            BlockTexture::GrassBottom  => dirt::TOP,
            BlockTexture::DirtTop      => dirt::TOP,
            BlockTexture::DirtSide     => dirt::RIGHT,
            BlockTexture::Stone        => stone::TOP,
        }
    }
}
```

### 3.3 修改 `face_quad` 函数

`face_quad` 当前返回固定 placeholder UV，需要改为根据 block 类型和 face 方向返回正确的 UV。

函数签名改为：

```rust
fn face_quad(
    x: usize,
    y: usize,
    z: usize,
    face: Face,
    block_id: BlockId,
) -> ([[f32; 3]; 4], [[f32; 2]; 4], [f32; 3])
```

内部 UV 部分（替换原来的 `face_uvs(face)` 调用）：

```rust
// 在 match face 分支里，获取 UV 范围后按顶点排列
let tex = BlockTexture::from_block_and_face(block_id, face);
let (u0, u1, v0, v1) = {
    let slot = tex.atlas_slot();
    let (u0, u1, v0, v1) = slot.uv();
    (u0, u1, v0, v1)
};

// 根据面方向确定 UV 顶点的排列（和 positions 一一对应）
// 以 Top 面为例：positions 的 4 个顶点按左下→右下→右上→左上的顺序
// UV 排列要与之匹配：
//
//   positions[3] ─── positions[2]
//        │              │
//        │              │
//   positions[0] ─── positions[1]
//
//   UV 对应：
//   (u0,v1)          (u1,v1)
//        │              │
//        │              │
//   (u0,v0) ────────── (u1,v0)

let face_uvs: [[f32; 2]; 4] = match face {
    Face::Top => [
        [u0, v0],  // 左下
        [u1, v0],  // 右下
        [u1, v1],  // 右上
        [u0, v1],  // 左上
    ],
    Face::Bottom => [
        [u0, v0],
        [u1, v0],
        [u1, v1],
        [u0, v1],
    ],
    Face::Right | Face::Front => [
        [u0, v0],
        [u1, v0],
        [u1, v1],
        [u0, v1],
    ],
    Face::Left | Face::Back => [
        [u1, v0],  // 注意左右/前后面需要镜像
        [u0, v0],
        [u0, v1],
        [u1, v1],
    ],
};
```

### 3.4 修改 `generate_chunk_mesh` 中对 `face_quad` 的调用

```rust
// 原来：
let (face_verts, face_uvs, face_normal) = face_quad(x, y, z, face);

// 改为：
let (face_verts, face_uvs, face_normal) = face_quad(x, y, z, face, block_id);
```

### 3.5 删除顶点颜色相关代码

在 `generate_chunk_mesh` 返回值和所有相关变量中：

1. 删除 `colors: Vec<[f32; 4]>` 变量声明
2. 删除 `let block_color = block_color_as_rgba(block_id);` 行
3. 删除 `colors.extend([block_color; 4]);` 行
4. 函数返回类型从 5 元组改为 4 元组：

```rust
pub fn generate_chunk_mesh(
    chunk: &Chunk,
) -> (Vec<[f32; 3]>, Vec<[f32; 2]>, Vec<[f32; 3]>, Vec<u32>) {
//                                   ^^^^^^^^^^ 删除了 Vec<[f32; 4]>
    let mut positions = Vec::new();
    let mut uvs = Vec::new();
    let mut normals = Vec::new();
    let mut indices = Vec::new();
```

5. 删除 `block_color_as_rgba` 函数

### 3.6 修改 `spawn_chunk_entity`

当前函数签名和调用 `generate_chunk_mesh` 的部分需要更新：

```rust
// 返回值解构（删除 colors）
let (positions, uvs, normals, indices) = generate_chunk_mesh(&chunk);
```

`spawn_chunk_entity` 的材质部分从顶点颜色改为纹理：

```rust
// 原来：
let mat = materials.add(StandardMaterial {
    base_color: Color::WHITE,
    ..default()
});

// 改为：
let mat = materials.add(StandardMaterial {
    base_color_texture: Some(texture_handle.clone()),
    ..default()
});
```

函数签名增加 `texture_handle: Handle<Image>` 参数：

```rust
pub fn spawn_chunk_entity(
    commands: &mut Commands,
    materials: &mut Assets<StandardMaterial>,
    meshes: &mut Assets<Mesh>,
    chunk: Chunk,
    position: Vec3,
    texture_handle: Handle<Image>,  // 新增
)
```

### 3.7 修改 `spawn_initial_chunks`

`spawn_initial_chunks` 的签名也要增加 `texture_handle: Handle<Image>` 参数，并向下传递：

```rust
pub fn spawn_initial_chunks(
    mut commands: Commands,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    asset_server: Res<AssetServer>,  // 新增：加载纹理
) {
    // ...
    spawn_chunk_entity(
        &mut commands,
        &mut materials,
        &mut meshes,
        chunk,
        Vec3::ZERO,
        asset_server.load("textures/array_texture.png"),
    );
}
```

---

## 4. 修改 `src/main.rs`

### 4.1 `spawn_initial_chunks` 调用签名变更

```rust
// 原来（无参数）：
chunk::spawn_initial_chunks,

// 改为（传入 asset_server）：
chunk::spawn_initial_chunks(asset_server),
```

注意：`spawn_initial_chunks` 现在接收 `Res<AssetServer>`，Bevy 会自动注入。

### 4.2 完整 `main` 函数预期结果

```rust
fn main() {
    App::new()
        .init_resource::<WireframeMode>()
        .add_plugins((
            DefaultPlugins,
            WireframePlugin::default(),
        ))
        .add_systems(Startup, (
            cube::setup_lighting,
            chunk::spawn_initial_chunks,
            hud::spawn_crosshair,
        ))
        .add_systems(Update, (
            camera::camera_movement,
            camera::camera_rotation,
            input::cursor_grab_system,
            chunk_wire_frame::toggle_wireframe,
            chunk_wire_frame::sync_chunk_wireframe,
            chunk_wire_frame::draw_wireframes,
            hud::update_hud,
        ))
        .run();
}
```

`spawn_initial_chunks` 的参数 `asset_server: Res<AssetServer>` 由 Bevy App 自动注入，不需要显式传入。

---

## 5. 纹理资源准备

### 5.1 `array_texture.png` 当前状态

当前文件尺寸为 250×1000，但子图尺寸未知。需要确认实际布局。

如果实际子图是 **16×16**（而非 32×32），则调整 `TILE_SIZE_PX`：

```rust
pub const TILE_SIZE_PX: u32 = 16;  // 而非 32
```

并且 UV 计算中的像素到 UV 映射会自动随之调整。

### 5.2 推荐的 atlas 布局（如果需要重新制作）

建议将 `array_texture.png` 制作成 **256×512 像素**，每子图 **32×32 像素**（2 的幂次，GPU 友好），排列为 8 列 × 16 行。

```
256w × 512h，每格 32×32，共 8×16=128 个槽位

实际使用：
  grass_top    → (0, 0)   草地顶面
  grass_side   → (1, 0)   草地侧面（土色）
  grass_bottom → (0, 1)   泥土（底面）
  dirt         → (1, 1)   泥土
  stone        → (2, 0)   石头
```

### 5.3 UV 镜像问题

体素方块有些面需要 **UV 镜像**（翻转）以保证纹理方向一致：

- **Top 面（朝上）**：UV (0,0)-(1,1)，无需翻转
- **Bottom 面（朝下）**：UV (0,0)-(1,1)，无需翻转
- **Side 面（朝外）**：部分需要水平镜像，取决于方块类型

在 `face_quad` 的 `BlockTexture` 实现中，`LEFT` 和 `BACK` 面使用了 `u1, v0` → `u0, v0` → `u0, v1` → `u1, v1` 的排列来实现水平翻转。

如果发现纹理方向反转，调整对应 face 的 UV 顶点顺序即可。

---

## 6. 面剔除与纹理的关系

### 6.1 面剔除逻辑不变

当前 `is_face_visible` 的逻辑：邻居方块类型与当前不同时，渲染暴露面。这与纹理方案无关，继续沿用。

### 6.2 纹理切换成本

当方块类型改变（如放置/破坏方块）时：
- 整个 Chunk 的 mesh 需要重新生成（已有逻辑）
- UV 属性全部重新计算（成本极低，CPU 遍历一次可见面列表）
- Draw Call 数量不变

---

## 7. 实现检查清单

```
Step 1: [ ] 创建 src/atlas.rs，定义 AtlasSlot、grass/dirt/stone 模块的常量
Step 2: [ ] 在 main.rs 添加 mod atlas;
Step 3: [ ] 在 chunk.rs 添加 BlockTexture 枚举及 from_block_and_face 实现
Step 4: [ ] 修改 face_quad 函数签名，增加 block_id 参数
Step 5: [ ] 实现 face_quad 中的 UV 计算（按 atlas slot）
Step 6: [ ] 修改 generate_chunk_mesh：删除 colors 相关代码，传递 block_id 给 face_quad
Step 7: [ ] 修改 spawn_chunk_entity：移除顶点颜色材质，改为 base_color_texture
Step 8: [ ] 修改 spawn_initial_chunks：加载纹理并传递 handle
Step 9: [ ] 修改 main.rs：spawn_initial_chunks 调用适配 Res<AssetServer> 参数
Step 10: [ ] cargo build 验证编译
Step 11: [ ] 运行游戏，检查纹理是否正确显示
Step 12: [ ] 检查 UV 方向是否正确（无翻转/镜像异常）
```

---

## 8. 注意事项

### 8.1 `face_quad` 中 UV 与 positions 的对应关系

UV 的 4 个顶点**必须**与 positions 的 4 个顶点一一对应。当前代码中每个面 4 个顶点的排列顺序是：

```
positions[0] ─── positions[1]
     │              │
     │              │
positions[3] ─── positions[2]
```

UV 排列需要根据这个顺序来排列，不能颠倒。

### 8.2 Minecraft 风格的 grass 侧面

Minecraft 中草方块的侧面是"草包着土"的效果（上下草色，中间土色）。如果 `array_texture.png` 中的 `grass_side` 就是这样的设计，则直接使用；如果只是纯色，则 grass 侧面和 dirt 侧面看起来会一样（都是纯色），这是正常的。

### 8.3 纹理加载时机

`asset_server.load()` 是异步的。首次渲染时纹理可能还没加载完成，Bevy 会自动等待。无需额外的加载状态管理。

### 8.4 迁移到方案 A

如果将来需要迁移到 `texture_2d_array`，核心改动是：
1. 将 `BlockTexture` 的返回值从 `AtlasSlot` 改为 `u32`（layer index）
2. 简化 `face_quad` 中 UV 为 `[0,0]-[1,1]`（整层覆盖）
3. 改用 `MeshTag` 组件指定 layer
4. 新增自定义 WGSL shader

**`generate_chunk_mesh` 的调用流程不变**，只是 UV 数据和材质不同。

---

## 9. 参考代码对照

| 文件 | 改前 | 改后 |
|------|------|------|
| `chunk.rs` import | `use bevy::{...}` | + `use crate::atlas::{self, grass, dirt, stone};` |
| `chunk.rs` BlockId | `pub type BlockId = u8;` | 不变 |
| `chunk.rs` generate_chunk_mesh 返回 | 5 元组 (pos,uv,norm,color,idx) | 4 元组 (pos,uv,norm,idx) |
| `chunk.rs` face_quad 参数 | `(x,y,z,face)` | `(x,y,z,face,block_id)` |
| `chunk.rs` face_quad UV | `face_uvs(face)` 固定值 | 动态查 atlas slot |
| `chunk.rs` spawn_chunk_entity 参数 | 无 texture_handle | + `texture_handle: Handle<Image>` |
|| `chunk.rs` spawn_chunk_entity 材质 | `base_color: Color::WHITE` | `base_color_texture: Some(texture_handle)` |
|| `main.rs` | `chunk::spawn_initial_chunks,` | `chunk::spawn_initial_chunks,`（参数由 Bevy 注入）|

---

## 10. 区块数据管理优化（参考体素管理方案）

> 以下内容参考 `docs/体素管理方案.md` 中的区块管理设计，结合当前项目实际情况，筛选出可直接落地的优化项。这些改动**与纹理方案相互独立**，可以先于方案 B 实施，也可以合并实施。

### 10.1 `ChunkData` 三态枚举（推荐立即采用）

当前项目 `Chunk` 使用 `Vec<BlockId>` 扁平淡色存储，高空和地下完全空气的区块仍然占用 `32³ = 32768` 字节的堆内存。引入三态枚举后，空区块降至零内存占用：

```rust
/// 三态区块数据：完全空气 / 全同材质 / 真实异构数据
pub enum ChunkData {
    /// 全空气，不占用任何堆内存
    Empty,
    /// 整个区块全部是同一种方块，仅存 2 字节的 BlockId
    Uniform(BlockId),
    /// 异构区块，存储完整的 32³ 体素数组（64 KB）
    Mixed(Vec<BlockId>),
}

impl ChunkData {
    /// 获取指定坐标的方块 ID（索引公式与原 Chunk 一致）
    pub fn get(&self, x: usize, y: usize, z: usize) -> BlockId {
        match self {
            ChunkData::Empty => 0,
            ChunkData::Uniform(id) => *id,
            ChunkData::Mixed(data) => {
                let idx = x + y * CHUNK_SIZE + z * CHUNK_SIZE * CHUNK_SIZE;
                data[idx]
            }
        }
    }

    /// 设置指定坐标的方块 ID
    /// 如果写入导致 Uniform 区块变为异构，自动升级为 Mixed
    pub fn set(&mut self, x: usize, y: usize, z: usize, id: BlockId) {
        match self {
            ChunkData::Empty => {
                *self = ChunkData::Uniform(id);
            }
            ChunkData::Uniform(current_id) => {
                if *current_id != id {
                    // 升级为 Mixed，先填充当前值，再写入新值
                    let mut data = vec![*current_id; CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE];
                    let idx = x + y * CHUNK_SIZE + z * CHUNK_SIZE * CHUNK_SIZE;
                    data[idx] = id;
                    *self = ChunkData::Mixed(data);
                }
            }
            ChunkData::Mixed(data) => {
                let idx = x + y * CHUNK_SIZE + z * CHUNK_SIZE * CHUNK_SIZE;
                data[idx] = id;
            }
        }
    }

    /// 区块是否完全为空
    pub fn is_empty(&self) -> bool {
        matches!(self, ChunkData::Empty)
    }
}
```

**预期效果**：Y=3 以上所有区块立即变为 `Empty`，内存占用从 `32768 × N` 字节降至接近 0。

### 10.2 邻居边界脏标记扩散

当玩家在区块边界放置/破坏方块时，**相邻区块**的可见面也可能改变，需要一起重建网格。

修改方块时的脏标记传播逻辑：

```rust
/// 区块坐标（32³ 为一个单位）
#[derive(Clone, Copy, Hash, Eq, PartialEq)]
pub struct ChunkCoord {
    pub cx: i32,
    pub cy: i32,
    pub cz: i32,
}

impl ChunkCoord {
    /// 世界坐标转区块坐标
    pub fn from_world(world_pos: Vec3) -> Self {
        Self {
            cx: (world_pos.x / CHUNK_SIZE as f32).floor() as i32,
            cy: (world_pos.y / CHUNK_SIZE as f32).floor() as i32,
            cz: (world_pos.z / CHUNK_SIZE as f32).floor() as i32,
        }
    }
}

/// 修改方块后，标记需要重建的区块集合（含相邻区块）
pub fn mark_block_dirty(
    coord: ChunkCoord,
    local_pos: (usize, usize, usize),
    dirty_chunks: &mut Vec<ChunkCoord>,
) {
    dirty_chunks.push(coord);

    // 检查是否在区块边界上
    let (x, y, z) = local_pos;
    if x == 0 {
        dirty_chunks.push(ChunkCoord { cx: coord.cx - 1, cy: coord.cy, cz: coord.cz });
    }
    if x == CHUNK_SIZE - 1 {
        dirty_chunks.push(ChunkCoord { cx: coord.cx + 1, cy: coord.cy, cz: coord.cz });
    }
    if y == 0 {
        dirty_chunks.push(ChunkCoord { cx: coord.cx, cy: coord.cy - 1, cz: coord.cz });
    }
    if y == CHUNK_SIZE - 1 {
        dirty_chunks.push(ChunkCoord { cx: coord.cx, cy: coord.cy + 1, cz: coord.cz });
    }
    if z == 0 {
        dirty_chunks.push(ChunkCoord { cx: coord.cx, cy: coord.cy, cz: coord.cz - 1 });
    }
    if z == CHUNK_SIZE - 1 {
        dirty_chunks.push(ChunkCoord { cx: coord.cx, cy: coord.cy, cz: coord.cz + 1 });
    }
}
```

**预期效果**：玩家在区块边界放置方块时，相邻区块也会重建网格，接缝处永远正确。

### 10.3 区块状态机（简化版）

当前项目只有 `Active` 状态。等将来需要多区块时，扩展为以下状态：

```
Unloaded → Generating → Dormant → Active
```

| 状态 | 数据存在 | GPU 实体存在 | 说明 |
|------|---------|-------------|------|
| Unloaded | ❌ | ❌ | 未加载或已卸载 |
| Generating | ⏳ | ❌ | 正在生成/从磁盘加载 |
| Dormant | ✅ | ❌ | 数据保留，网格已销毁（释放 GPU 资源）|
| Active | ✅ | ✅ | 完全激活，可交互 |

**当前项目只需 Active**，等有多区块时再加入 Dormant 状态（数据保留但网格销毁）。

### 10.4 整合后的实施顺序

```
Phase 1: ChunkData 三态枚举（内存优化，立即生效）
Phase 2: 方案 B 纹理（视觉升级，与 Phase 1 互不干扰）
Phase 3: 多区块 + 邻居脏标记扩散 + 状态机
Phase 4: SuperChunk 合批（性能优化）
Phase 5: GPU 面提取 + MultiDrawIndirect（高级优化）
```

Phase 1 和 Phase 2 可以并行开发，互不依赖。

### 10.5 ChunkData 三态枚举对现有代码的影响

| 函数/类型 | 需要改动的内容 |
|-----------|--------------|
| `Chunk::new()` | 改为 `ChunkData::Empty` 初始化 |
| `Chunk::filled(block_id)` | 改为 `ChunkData::Uniform(block_id)` |
| `chunk.get_block_unchecked()` | 改为委托给 `ChunkData::get()` |
| `chunk.set_block()` | 改为委托给 `ChunkData::set()` |
| `fill_terrain()` | 行为不变，内部自动升级为 Uniform/Mixed |
| `generate_chunk_mesh()` | 改为接受 `&ChunkData` 而非 `&Chunk` |
| `spawn_chunk_entity()` | 签名不变，透传 `ChunkData` |
| `is_face_visible()` | 需要能查询邻居数据，将来传入 `&ChunkData` |

当前 `Chunk` 结构体可以**直接替换为** `ChunkData`，因为 `Chunk` 的 `blocks: Vec<BlockId>` 和 `ChunkData::Mixed(Vec<BlockId>)` 完全等价。只需把 `Chunk` 类型别名改为 `ChunkData` 即可平滑过渡。

---

## 11. 未来扩展项（暂不实施）

以下内容是方案 B 当前的空白点，当前 MVP 阶段暂不实施，留作后续参考：

### 11.1 方块放置/破坏后的 mesh 重建机制

当前文档只覆盖了静态纹理显示。玩家放/挖方块后，需要：

- 设计 `DirtyFlag` 组件或 `DirtyChunk` 资源标记需要重建的区块
- 在 Update 系统里检测 dirty 状态，调用 `generate_chunk_mesh` 重建并替换 mesh
- 区分**同步重建**（立即执行，影响操作手感）和**异步重建**（分摊到多帧，不阻塞）

这块与 §10.2 的 `mark_block_dirty` 紧密耦合，建议同时实施。

### 11.2 UV 顶点排列的运行时验证

`Face::Left` / `Face::Back` 的 UV 顶点做了水平镜像处理，但实际是否与 `positions` 的顶点顺序完全对应，需要在实际运行时验证。

调试方法：在 UV 计算后临时将某个面的 UV 强制设为 `[[0,0],[1,0],[1,1],[0,1]]`，观察纹理是否出现拉伸/翻转，据此判断顶点顺序是否正确。

### 11.3 多区块加载与视锥剔除

当前 `spawn_initial_chunks` 只生成 1 个区块。扩展到多区块时需要：

- 实现 `ChunkManager` 管理多个 `ChunkData` 的加载/卸载
- `spawn_chunk_area(area_radius)` 批量生成区块网格
- 添加基于视锥的可见性判断，不再渲染相机背后和视锥外的区块

### 11.4 相机初始位置随区块数量调整

当前 camera 初始位置 `(16.0, 20.0, 16.0)` 固定在第一个区块（原点）的正上方。多区块场景下应改为：

```rust
// 以玩家/相机位置为中心，周围 1~2 区块范围内加载
let camera_target = player_position; // 或固定在地形高度上方
```

### 11.5 纹理加载状态监控

`asset_server.load()` 异步加载，首次加载失败（文件损坏/路径错误）时 mesh 会渲染为白色。建议后续加入：

- `TextureLoaderState` 资源跟踪加载进度
- 或利用 Bevy 的 `AssetServer::get_load_state` 查询加载结果
- 出错时在 HUD 显示调试信息

