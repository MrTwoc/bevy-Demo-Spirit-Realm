# 多群系真实感地形系统 — Spec

> 创建时间：2026-06-06
>
> 状态：设计阶段
>
> 参考项目：[Tectonic](../项目参考/地形参考/tectonic-rewrite-squared)

---

## 一、概述

### 1.1 目标

将当前单一噪声地形升级为**真实感多群系地形系统**，实现类似 Minecraft Tectonic 风格的：

- **大陆/海洋** 自然划分
- **5 种基础生物群系**：海洋、平原、沙漠、森林、山脉
- **自然过渡**：群系之间无硬边界，由海拔+温度+植被自然决定
- **地表河流**：地形切割型河流，真实蜿蜒
- **连通洞穴网络**：多层地下洞穴系统

### 1.2 设计原则

| 原则 | 说明 |
|------|------|
| **确定性** | 相同种子 + 相同坐标 = 相同结果，支持跨区块一致性 |
| **性能优先** | 异步生成，噪声缓存，查表优化 |
| **渐进式** | 4 个阶段迭代，每阶段可独立验证 |
| **可扩展** | 新群系只需添加噪声阈值映射，不改核心管线 |

### 1.3 坐标系统

| 参数 | 当前值 | 新值 | 说明 |
|------|--------|------|------|
| Y 轴范围 | -256 ~ 256 | **-64 ~ 320** | 仿照 Minecraft，后续可扩展 |
| 海平面 | Y=80 | **Y=63** | 与 Tectonic 对齐 |
| 基础地形高度 | Y=96 | **Y=80** | 陆地平均海拔 |
| XZ 生成范围 | 无限 | **无限** | 保持现有 ChunkManager |

---

## 二、噪声管线架构

### 2.1 数据流

```
世界坐标 (x, y, z)
    │
    ├─→ [噪声层 1] continentalness  → 大陆性值 [-1, 1]
    ├─→ [噪声层 2] erosion          → 侵蚀值 [-1, 1]
    ├─→ [噪声层 3] ridge            → 山脊值 [-1, 1]
    ├─→ [噪声层 4] temperature      → 温度值 [-1, 1]
    └─→ [噪声层 5] vegetation       → 植被值 [-1, 1]
    │
    ├─→ [生物群系判定] temp × veg × elevation → BiomeType
    │
    ├─→ [基础地形] base_terrain(continentalness, erosion, ridge)
    │       ├─ 海洋地形（continentalness < ocean_threshold）
    │       ├─ 平原地形（中等海拔，低起伏）
    │       ├─ 沙漠地形（中等海拔，沙丘噪声）
    │       ├─ 森林地形（中等海拔，微起伏）
    │       └─ 山脉地形（高海拔，山脊噪声）
    │
    ├─→ [侵蚀修饰] erosion_modifier(base_terrain, erosion)
    │       ├─ 山顶削平
    │       └─ 河谷切割
    │
    ├─→ [河流注入] river_system(terrain, erosion)
    │       └─ 地表河流：沿侵蚀低洼处注入水
    │
    ├─→ [洞穴挖空] cave_system(x, y, z)
    │       ├─ Cheese 洞穴（大型空腔）
    │       ├─ Spaghetti 洞穴（蜿蜒隧道）
    │       └─ Noodle 洞穴（细小裂隙）
    │
    └─→ [最终方块] min(terrain_density, caves) → BlockId
```

### 2.2 噪声层参数

参考 Tectonic 的噪声参数，适配到 `noise` crate：

| 噪声层 | 类型 | 频率 | 八度 | 持续度 | 拉acunarity | 用途 |
|--------|------|------|------|--------|-------------|------|
| continentalness | Fbm\<Simplex\> | 0.0008 | 6 | 0.5 | 2.0 | 大陆/海洋划分 |
| erosion | Fbm\<Simplex\> | 0.0015 | 6 | 0.5 | 2.0 | 地形侵蚀程度 |
| ridge | Ridged\<Simplex\> | 0.001 | 4 | 0.5 | 2.0 | 山脊线生成 |
| temperature | Fbm\<Simplex\> | 0.0006 | 4 | 0.5 | 2.0 | 气候温度 |
| vegetation | Fbm\<Simplex\> | 0.0006 | 4 | 0.5 | 2.0 | 植被湿度 |

> **频率说明**：Tectonic 使用 `firstOctave=-10`（即极低频），这里用 0.0006~0.0015 模拟相同效果。
> 频率越低，群系越大。后期可通过配置文件调整。

### 2.3 大陆性系统

```rust
/// 大陆性阈值（参考 Tectonic ocean_offset = -0.8）
const OCEAN_THRESHOLD: f64 = -0.3;      // continentalness < 此值 → 海洋
const DEEP_OCEAN_THRESHOLD: f64 = -0.5; // continentalness < 此值 → 深海
const CONTINENT_THRESHOLD: f64 = 0.2;   // continentalness > 此值 → 内陆

/// 根据大陆性值确定基础地形类型
fn terrain_type_from_continentalness(c: f64) -> TerrainType {
    if c < DEEP_OCEAN_THRESHOLD { TerrainType::DeepOcean }
    else if c < OCEAN_THRESHOLD { TerrainType::Ocean }
    else if c < CONTINENT_THRESHOLD { TerrainType::Coast }
    else { TerrainType::Inland }
}
```

---

## 三、生物群系系统

### 3.1 三轴映射

生物群系由三个连续噪声值共同决定：

```
温度(temperature) × 植被(vegetation) × 海拔(elevation) → BiomeType
```

#### 温度分级（5 级）

| 索引 | 范围 | 含义 |
|------|------|------|
| 1 | < -0.48 | 极寒 |
| 2 | -0.42 ~ -0.18 | 寒冷 |
| 3 | -0.12 ~ 0.17 | 温和 |
| 4 | 0.23 ~ 0.52 | 温暖 |
| 5 | > 0.58 | 炎热 |

#### 植被分级（5 级）

| 索引 | 范围 | 含义 |
|------|------|------|
| 1 | < -0.38 | 干旱 |
| 2 | -0.32 ~ -0.13 | 干燥 |
| 3 | -0.07 ~ 0.07 | 中等 |
| 4 | 0.13 ~ 0.27 | 湿润 |
| 5 | > 0.33 | 茂盛 |

#### 海拔分级（4 级）

| 索引 | 范围 | 含义 |
|------|------|------|
| 1 | < WATER_LEVEL (63) | 海洋/低洼 |
| 2 | 63 ~ 100 | 平原 |
| 3 | 100 ~ 180 | 丘陵 |
| 4 | > 180 | 山脉 |

### 3.2 群系判定矩阵

```
                    植被 1(干旱)  植被 2(干燥)  植被 3(中等)  植被 4(湿润)  植被 5(茂盛)
温度 1(极寒)        雪原         雪原          针叶林        针叶林        针叶林
温度 2(寒冷)        草原         草原          森林          森林          森林
温度 3(温和)        平原         平原          森林          森林          森林
温度 4(温暖)        沙漠         沙漠          平原          森林          森林
温度 5(炎热)        沙漠         沙漠          沙漠          平原          森林
```

**海拔覆盖规则**：
- 海拔 4（山脉）→ 强制为 **山脉** 群系（覆盖温度/植被）
- 海拔 1（海洋）→ 强制为 **海洋** 群系
- 海拔 2-3 → 使用温度×植被矩阵

### 3.3 群系类型定义

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BiomeType {
    Ocean,      // 海洋
    DeepOcean,  // 深海
    Plains,     // 平原
    Desert,     // 沙漠
    Forest,     // 森林
    Mountains,  // 山脉
    SnowPlains, // 雪原
    // 后期扩展：
    // Swamp,   // 沼泽
    // Jungle,  // 丛林
    // Tundra,  // 冻原
}
```

### 3.4 自然过渡机制

群系之间的过渡由噪声的**连续性**天然保证：

1. **温度/植被噪声**：低频（0.0006），变化缓慢，相邻区块温度差异极小
2. **海拔过渡**：大陆性噪声连续变化，海岸→平原→丘陵→山脉 是渐变的
3. **方块替换**：地表方块由 BiomeType 决定，相邻群系的地表方块自然交替

**无硬边界的关键**：不使用区块级群判定，而是**逐方块**计算 BiomeType。

---

## 四、地形生成公式

### 4.1 基础地形密度函数

参考 Tectonic 的 `sloped_cheese` 管线：

```rust
/// 计算世界坐标 (wx, wy, wz) 处的地形密度
/// 密度 > 0 → 实体方块，密度 <= 0 → 空气/水
fn compute_terrain_density(wx: f64, wy: f64, wz: f64) -> f64 {
    // 1. 采样噪声层
    let continentalness = continentalness_noise.get([wx, wz]);
    let erosion = erosion_noise.get([wx, wz]);
    let ridge = ridge_noise.get([wx, wz]);

    // 2. 计算基础高度
    let base_height = compute_base_height(continentalness, erosion, ridge);

    // 3. 密度 = 基础高度 - 当前Y（正值=实体，负值=空气）
    let density = (base_height - wy as f64) / 64.0; // 归一化

    // 4. 应用侵蚀修饰
    apply_erosion(density, erosion, wy, base_height)
}
```

### 4.2 基础高度计算

```rust
fn compute_base_height(c: f64, e: f64, r: f64) -> f64 {
    // 海洋区域：c < ocean_threshold
    if c < OCEAN_THRESHOLD {
        // 海底深度由大陆性决定
        let depth = lerp(c, DEEP_OCEAN_THRESHOLD, OCEAN_THRESHOLD, -80.0, 0.0);
        return SEA_LEVEL as f64 + depth;
    }

    // 陆地基础高度
    let land_base = SEA_LEVEL as f64 + 20.0; // 海平面以上 20 格

    // 山脊贡献：ridge > 0 时增加高度
    let ridge_height = if r > 0.0 {
        r.powf(1.5) * 300.0 // 非线性山脊，陡峭
    } else {
        r * 50.0 // 负值区域轻微凹陷
    };

    // 侵蚀调制：高侵蚀值削平山顶
    let erosion_factor = 1.0 - (e.max(0.0) * 0.6); // e>0 时削弱高度

    // 最终高度
    land_base + ridge_height * erosion_factor
}
```

### 4.3 侵蚀效果

```rust
/// 中等侵蚀：山顶削平 + 简单河谷切割
fn apply_erosion(density: f64, erosion: f64, wy: i32, base_height: f64) -> f64 {
    let mut d = density;

    // 山顶削平：当 erosion > 0.3 时，降低高于 base_height 的区域
    if erosion > 0.3 && wy as f64 > base_height - 50.0 {
        let flat_factor = (erosion - 0.3) / 0.7; // [0, 1]
        d -= flat_factor * 0.3; // 削平效果
    }

    // 河谷切割：当 erosion < -0.3 时，在中等海拔处加深
    if erosion < -0.3 {
        let cut_factor = (-erosion - 0.3) / 0.7; // [0, 1]
        let altitude_factor = 1.0 - ((wy as f64 - SEA_LEVEL as f64).abs() / 100.0).min(1.0);
        d += cut_factor * altitude_factor * 0.2; // 切割效果
    }

    d
}
```

### 4.4 群系特化地形

不同群系在基础地形上有不同的修饰：

| 群系 | 地形特征 | 实现方式 |
|------|---------|---------|
| 海洋 | 海床平坦，深度随大陆性变化 | 基础高度压低到海平面以下 |
| 平原 | 平坦草地，微起伏 | 细节噪声振幅 ±5 |
| 沙漠 | 沙丘起伏，干燥 | 叠加正弦波沙丘噪声 |
| 森林 | 中等起伏，树密 | 细节噪声振幅 ±15 |
| 山脉 | 高耸陡峭，岩石裸露 | ridge 噪声主导，振幅 ±300 |

```rust
/// 群系特化：修改细节噪声振幅
fn biome_detail_amplitude(biome: BiomeType) -> f64 {
    match biome {
        BiomeType::Ocean => 3.0,
        BiomeType::DeepOcean => 2.0,
        BiomeType::Plains => 5.0,
        BiomeType::Desert => 8.0, // 沙丘
        BiomeType::Forest => 15.0,
        BiomeType::Mountains => 30.0,
        BiomeType::SnowPlains => 10.0,
    }
}
```

---

## 五、方块系统

### 5.1 方块 ID 分配

```rust
// 现有方块（保持不变）
pub const AIR: BlockId = 0;
pub const GRASS: BlockId = 1;
pub const STONE: BlockId = 2;
pub const DIRT: BlockId = 3;
pub const SAND: BlockId = 4;
pub const WATER: BlockId = 5;
pub const TREE_TRUNK: BlockId = 6;
pub const TREE_LEAVES: BlockId = 7;

// 新增方块
pub const SANDSTONE: BlockId = 8;    // 沙石（沙漠地下层）
pub const SNOW_GRASS: BlockId = 9;   // 雪地草（寒冷群系地表）
pub const GRAVEL: BlockId = 10;      // 砂砾（河床/山脚）
pub const ROCK: BlockId = 11;        // 岩石变体（山脉裸露）
pub const MUD: BlockId = 12;         // 泥土变体（河岸/湿地）
```

### 5.2 群系 → 地表方块映射

```rust
fn surface_block(biome: BiomeType, world_y: i32) -> BlockId {
    match biome {
        BiomeType::Ocean | BiomeType::DeepOcean => {
            if world_y == SEA_LEVEL - 1 { SAND } else { STONE }
        }
        BiomeType::Plains | BiomeType::Forest => GRASS,
        BiomeType::Desert => SAND,
        BiomeType::Mountains => {
            if world_y > 200 { ROCK } else { STONE }
        }
        BiomeType::SnowPlains => SNOW_GRASS,
    }
}

fn subsurface_block(biome: BiomeType, depth: i32) -> BlockId {
    match biome {
        BiomeType::Desert => {
            if depth < 4 { SAND } else { SANDSTONE }
        }
        BiomeType::Mountains => {
            if depth < 2 { GRAVEL } else { STONE }
        }
        _ => {
            if depth < DIRT_LAYER_DEPTH { DIRT } else { STONE }
        }
    }
}
```

---

## 六、河流系统

### 6.1 设计思路

地表河流沿**侵蚀低洼处**生成，使用独立的河流噪声确定路径：

```
河流噪声(river_noise) 在特定值附近 → 河道位置
河道位置 + 地形高度 → 挖掘河道 + 注入水
```

### 6.2 河流噪声

```rust
/// 河流路径噪声：使用 ridged 噪声产生蜿蜒的河流线
/// 当 |noise_value| < river_width 时，该位置是河道
static RIVER_NOISE: OnceLock<Ridged<Simplex>> = OnceLock::new();

fn get_river_noise() -> &'static Ridged<Simplex> {
    RIVER_NOISE.get_or_init(|| {
        Ridged::<Simplex>::new(TERRAIN_SEED.wrapping_add(100))
            .set_octaves(3)
            .set_frequency(0.0008) // 低频 → 长河流
            .set_lacunarity(2.0)
            .set_persistence(0.5)
    })
}

/// 判断世界坐标 (wx, wz) 是否在河道中
/// 返回：None = 不在河道，Some(depth) = 河道深度
fn river_at(wx: f64, wz: f64, continentalness: f64) -> Option<i32> {
    // 海洋中不生成河流
    if continentalness < OCEAN_THRESHOLD {
        return None;
    }

    let river_val = get_river_noise().get([wx, wz]).abs();
    let river_width = 0.02; // 河道宽度阈值

    if river_val < river_width {
        // 河道深度：越靠近中心越深
        let depth_factor = 1.0 - (river_val / river_width);
        let depth = (depth_factor * 8.0) as i32 + 2; // 2~10 格深
        Some(depth)
    } else {
        None
    }
}
```

### 6.3 河道挖掘流程

```rust
/// 在 fill_terrain 中应用河流
fn apply_river(
    chunk: &mut ChunkData,
    coord: &ChunkCoord,
    x: usize, z: usize,
    wx: f64, wz: f64,
    surface_height: i32,
    continentalness: f64,
) {
    if let Some(river_depth) = river_at(wx, wz, continentalness) {
        let river_bottom = surface_height - river_depth;
        let chunk_oy = coord.cy * CHUNK_SIZE as i32;

        for y in 0..CHUNK_SIZE {
            let wy = chunk_oy + y as i32;

            if wy > surface_height && wy <= SEA_LEVEL {
                // 水面以上、海平面以下 → 注入水
                chunk.set(x, y, z, WATER);
            } else if wy <= surface_height && wy > river_bottom {
                // 河道范围内的实体方块 → 挖空
                chunk.set(x, y, z, AIR);
            } else if wy == river_bottom {
                // 河床 → 砂砾
                chunk.set(x, y, z, GRAVEL);
            }
        }
    }
}
```

---

## 七、洞穴系统

### 7.1 三层洞穴网络

参考 Tectonic 的 Cheese + Spaghetti + Noodle 三层系统：

| 层级 | 类型 | 特征 | Y 范围 |
|------|------|------|--------|
| 浅层 | Spaghetti | 蜿蜒隧道，2-4 格宽 | Y=40 ~ 80 |
| 中层 | Cheese | 大型空腔，10-30 格 | Y=-20 ~ 40 |
| 深层 | Noodle | 细小裂隙，1-2 格宽 | Y=-64 ~ 0 |

### 7.2 洞穴密度函数

```rust
/// 洞穴噪声配置
struct CaveNoiseConfig {
    cheese: Fbm<Simplex>,    // 大型空腔
    spaghetti: Fbm<Simplex>, // 蜿蜒隧道
    noodle: Fbm<Simplex>,    // 细小裂隙
}

impl CaveNoiseConfig {
    fn new(seed: u32) -> Self {
        Self {
            cheese: Fbm::<Simplex>::new(seed.wrapping_add(200))
                .set_octaves(3)
                .set_frequency(0.02)
                .set_lacunarity(2.0)
                .set_persistence(0.5),
            spaghetti: Fbm::<Simplex>::new(seed.wrapping_add(201))
                .set_octaves(4)
                .set_frequency(0.03)
                .set_lacunarity(2.0)
                .set_persistence(0.5),
            noodle: Fbm::<Simplex>::new(seed.wrapping_add(202))
                .set_octaves(2)
                .set_frequency(0.05)
                .set_lacunarity(2.0)
                .set_persistence(0.5),
        }
    }

    /// 计算洞穴密度（正值 = 挖空）
    fn cave_density(&self, wx: f64, wy: f64, wz: f64) -> f64 {
        let depth_from_surface = ...; // 需要地表高度
        let depth_factor = (depth_from_surface as f64 / 64.0).clamp(0.0, 1.0);

        // Cheese：大型空腔
        let cheese_val = self.cheese.get([wx * 0.7, wy as f64 * 0.8, wz * 0.7]);
        let cheese = if cheese_val > 0.6 - depth_factor * 0.3 {
            cheese_val
        } else {
            0.0
        };

        // Spaghetti：蜿蜒隧道
        let spaghetti_val = self.spaghetti.get([wx, wy as f64 * 1.2, wz]);
        let spaghetti = if spaghetti_val.abs() < 0.15 {
            0.5 - spaghetti_val.abs() * 3.0
        } else {
            0.0
        };

        // Noodle：细小裂隙
        let noodle_val = self.noodle.get([wx * 1.5, wy as f64 * 2.0, wz * 1.5]);
        let noodle = if noodle_val.abs() < 0.08 {
            0.3 - noodle_val.abs() * 3.0
        } else {
            0.0
        };

        // 合并：取最大值
        (cheese.max(spaghetti).max(noodle)) * depth_factor
    }
}
```

### 7.3 洞穴入口

```rust
/// 地表洞穴入口：在特定位置生成通向地表的竖井
fn cave_entrance_at(wx: f64, wz: f64) -> bool {
    // 使用独立噪声决定入口位置
    let entrance_noise = ...;
    entrance_noise > 0.85 // 约 5% 的位置有入口
}
```

---

## 八、实现阶段

### 阶段 S1：噪声框架（基础地形）

**目标**：实现 5 层噪声管线，生成有高低起伏的地形（无群系区分）

**变更文件**：
- `src/chunk.rs` — 重构 `fill_terrain()`，添加噪声层
- `src/terrain_noise.rs` — **新建**，噪声层管理器

**具体任务**：
1. 创建 `TerrainNoiseManager` 结构体，管理 5 个噪声函数
2. 实现 `compute_base_height()` 函数
3. 实现 `compute_terrain_density()` 函数
4. 重构 `fill_terrain()` 使用新管线
5. 添加 `SEA_LEVEL`、`OCEAN_THRESHOLD` 等常量
6. 验证：地形有海洋、平原、山脉的自然起伏

**预计产出**：
- 海洋区域自然凹陷
- 陆地有丘陵和山脉
- 海岸线自然过渡

---

### 阶段 S2：生物群系系统

**目标**：实现温度×植被×海拔三轴映射，5 种群系各有独特外观

**变更文件**：
- `src/biome.rs` — **新建**，生物群系判定逻辑
- `src/chunk.rs` — 根据群系选择地表方块
- `src/tree_gen.rs` — 森林群系树木密度调整

**具体任务**：
1. 实现 `BiomeType` 枚举和 `get_biome()` 函数
2. 实现温度/植被噪声分级
3. 实现群系→方块映射（地表/地下层）
4. 添加新方块 ID（沙石、雪草、砂砾、岩石、泥土变体）
5. 调整树木生成：森林群系密，沙漠无树
6. 验证：不同区域有不同的地表材质

**预计产出**：
- 沙漠区域：沙地 + 沙丘 + 无树
- 森林区域：草地 + 密集树木
- 山脉区域：岩石裸露 + 高海拔
- 平原区域：平坦草地 + 稀疏树木
- 雪原区域：雪地草 + 针叶树

---

### 阶段 S3：地表河流

**目标**：河流沿侵蚀低洼处蜿蜒，切割地形并注入水

**变更文件**：
- `src/terrain_noise.rs` — 添加河流噪声
- `src/chunk.rs` — 河道挖掘 + 水填充逻辑

**具体任务**：
1. 添加河流 ridged 噪声
2. 实现 `river_at()` 路径判定
3. 实现河道挖掘逻辑（挖空 + 注入水 + 砂砾河床）
4. 确保河流不在海洋中生成
5. 处理河流与洞穴的交互（阶段 S4 后完善）
6. 验证：河流蜿蜒穿过地形，有自然深度

**预计产出**：
- 河流在陆地中蜿蜒
- 河道有自然深度变化
- 河床是砂砾方块
- 河流在入海口与海洋自然汇合

---

### 阶段 S4：洞穴系统

**目标**：多层地下洞穴网络，有入口和连通性

**变更文件**：
- `src/terrain_noise.rs` — 添加洞穴噪声
- `src/chunk.rs` — 洞穴挖空逻辑

**具体任务**：
1. 添加 3 层洞穴噪声（Cheese/Spaghetti/Noodle）
2. 实现 `cave_density()` 函数
3. 在 `fill_terrain()` 中应用洞穴挖空
4. 实现地表洞穴入口
5. 确保洞穴不影响地表方块（只在地下）
6. 验证：地下有连通洞穴，有入口可达

**预计产出**：
- 地下有大型空腔（Cheese）
- 有蜿蜒隧道（Spaghetti）
- 有细小裂隙（Noodle）
- 地表有洞穴入口
- 洞穴内为普通石头

---

## 九、性能考虑

### 9.1 噪声缓存

- 所有噪声函数使用 `OnceLock` 全局缓存（已有的模式）
- 大陆性/侵蚀/山脊噪声可预计算到 Chunk 级精度（每个 Chunk 只需 32×32 次采样）

### 9.2 生成性能预估

| 阶段 | 噪声采样次数/方块 | 相对耗时 |
|------|-------------------|---------|
| 当前 | 2 次（粗+细节） | 1.0x |
| S1 | 3 次（大陆+侵蚀+山脊） | 1.5x |
| S2 | 5 次（+温度+植被） | 2.5x |
| S3 | 6 次（+河流） | 3.0x |
| S4 | 9 次（+3 层洞穴） | 4.5x |

> 当前地形生成是异步的，4.5x 耗时在可接受范围内。
> 如果性能不足，可对噪声层进行预计算缓存。

### 9.3 LOD 兼容

新地形系统需要与现有 LOD 系统兼容：
- LOD0：完整地形 + 洞穴
- LOD1-3：简化噪声（跳过细节层和洞穴）

---

## 十、配置参数

```rust
/// 地形系统配置（可通过配置文件调整）
pub struct TerrainConfig {
    // 种子
    pub seed: u32,

    // 噪声频率（越低 = 群系越大）
    pub continentalness_freq: f64,  // 默认 0.0008
    pub erosion_freq: f64,          // 默认 0.0015
    pub ridge_freq: f64,            // 默认 0.001
    pub temperature_freq: f64,      // 默认 0.0006
    pub vegetation_freq: f64,       // 默认 0.0006
    pub river_freq: f64,            // 默认 0.0008

    // 阈值
    pub ocean_threshold: f64,       // 默认 -0.3
    pub deep_ocean_threshold: f64,  // 默认 -0.5
    pub continent_threshold: f64,   // 默认 0.2

    // 高度
    pub sea_level: i32,             // 默认 63
    pub terrain_base_height: i32,   // 默认 80
    pub terrain_max_height: i32,    // 默认 320
    pub terrain_min_y: i32,         // 默认 -64

    // 河流
    pub river_width: f64,           // 默认 0.02
    pub river_max_depth: i32,       // 默认 10

    // 洞穴
    pub caves_enabled: bool,        // 默认 true
    pub cheese_threshold: f64,      // 默认 0.6
    pub spaghetti_threshold: f64,   // 默认 0.15
    pub noodle_threshold: f64,      // 默认 0.08
}
```

---

## 十一、文件结构

```
src/
├── chunk.rs              # [重构] 方块存储 + 地形生成入口
├── terrain_noise.rs      # [新建] 噪声层管理器（5层+河流+洞穴）
├── biome.rs              # [新建] 生物群系判定系统
├── tree_gen.rs           # [调整] 树木生成适配群系
├── chunk_manager.rs      # [不变] 区块生命周期
├── async_mesh.rs         # [不变] 异步网格生成
├── lod.rs                # [调整] LOD 兼容新地形
└── ...

docs/
├── 多群系真实感地形系统Spec.md  # 本文档
└── ...
```

---

## 十二、风险与缓解

| 风险 | 影响 | 缓解措施 |
|------|------|---------|
| 性能下降 | 生成变慢 4-5x | 异步生成已有，噪声预计算缓存 |
| 群系边界生硬 | 视觉不自然 | 逐方块计算 BiomeType，噪声连续性保证过渡 |
| 洞穴地表穿透 | 地表出现不自然空洞 | depth_factor 渐变，地表附近洞穴密度趋零 |
| 河流断裂 | 河流不连续 | ridged 噪声天然连续，低频保证长距离连贯 |
| 跨区块不一致 | 区块边界出现裂缝 | 纯函数设计，相同坐标 = 相同结果 |

---

## 十三、验收标准

### S1 验收
- [ ] 地形有明显海洋、陆地、山脉
- [ ] 海岸线自然过渡（非直线）
- [ ] 山脉有山脊线特征
- [ ] 无跨区块裂缝

### S2 验收
- [ ] 5 种群系可视觉区分
- [ ] 沙漠区域无树木，森林区域密集树木
- [ ] 群系之间无硬边界
- [ ] 新方块正确渲染

### S3 验收
- [ ] 河流在陆地中蜿蜒可见
- [ ] 河流有自然深度变化
- [ ] 河流不在海洋中生成
- [ ] 河床为砂砾方块

### S4 验收
- [ ] 地下有可探索的洞穴
- [ ] 有地表入口可进入
- [ ] 洞穴不影响地表美观
- [ ] 洞穴内为普通石头
