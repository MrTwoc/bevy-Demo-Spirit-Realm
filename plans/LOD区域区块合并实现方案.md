# LOD 区域区块合并实现方案

> 基于《关于LOD区域区块合并.md》技术讨论，结合现有 Spirit Realm 代码架构设计

---

## 一、目标与背景

### 1.1 问题现状

当前系统中，每个 Chunk 独立生成 Mesh，即使在 LOD1+ 远距离区域也如此。这导致：

- **DrawCall 过多**：LOD1 区域（9-16 区块距离）仍有大量独立 Chunk，每个 Chunk 至少 1 个 DrawCall
- **GPU 批处理效率低**：相同材质的 Chunk 无法合并渲染
- **内存碎片化**：每个 Chunk 独立的 Mesh Handle 和 GPU Buffer

### 1.2 优化目标

将 LOD1 及更远区域的 Chunk 合并为**环形 Mesh（Ring Mesh）**，实现：

| 指标 | 当前 | 目标 |
|------|------|------|
| LOD1 DrawCall | ~数百个 | 12-24 个（环形扇区） |
| LOD2 DrawCall | ~数百个 | 12-24 个 |
| LOD3 DrawCall | ~数十个 | 12-24 个 |
| Mesh 重建频率 | 每个 Chunk 独立 | 仅移动后新进入的扇区 |

---

## 二、技术方案选型

### 2.1 方案对比

| 方案 | 复杂度 | 缝补效果 | 动态更新 | 适用场景 |
|------|--------|----------|----------|----------|
| **Geometry Clipmaps** | 中 | 优秀 | 扇区增量 | 平面/球面地形 |
| **CDLOD + Geomorphing** | 高 | 完美 | 需要 shader | 通用地形 |
| **Skirt（裙边）** | 低 | 一般 | 全量重建 | 快速原型 |
| **dexyfex 邻接 Blending** | 中 | 良好 | 扇区增量 | **体素地形 ✓** |
| **Nanite / GPU-driven** | 极高 | N/A | GPU 驱动 | 极致性能 |

### 2.2 推荐方案：dexyfex 邻接 Blending + 扇区增量更新

**选择理由**：

1. **体素地形适配**：dexyfex 方案专为体素设计，通过邻接信息在 shader 中做顶点偏移，天然适配方块世界
2. **增量更新友好**：配合扇区划分，仅重建移动后新进入视野的扇区
3. **实现复杂度可控**：不需要修改底层渲染管线，主要在 Mesh 生成层实现
4. **与现有 LOD 系统兼容**：可复用现有的 `LodLevel`、`LodManager` 等基础设施

---

## 三、核心设计

### 3.1 环形扇区划分

```
                    ┌─────────────────────────┐
                    │         LOD3 Ring        │
                    │   ┌─────────────────┐   │
                    │   │    LOD2 Ring    │   │
                    │   │   ┌─────────┐   │   │
                    │   │   │ LOD1    │   │   │
                    │   │   │  Ring   │   │   │
                    │   │   │ ┌─────┐ │   │   │
                    │   │   │ │LOD0 │ │   │   │
                    │   │   │ │(独立)│ │   │   │
                    │   │   │ └─────┘ │   │   │
                    │   │   └─────────┘   │   │
                    │   └─────────────────┘   │
                    └─────────────────────────┘
                    
        每个 Ring 划分为 N 个扇区（Sector），例如 16 个
        每个扇区 = 1 个 Mesh = 1 个 DrawCall
```

### 3.2 数据结构设计

```rust
/// 环形扇区坐标
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RingSectorCoord {
    /// LOD 级别
    pub lod: LodLevel,
    /// 扇区索引 (0..SECTOR_COUNT)
    pub sector_index: u32,
    /// 环形层级（距离玩家的环数）
    pub ring_level: u32,
}

/// 环形扇区 Mesh 数据
pub struct RingSectorMesh {
    /// 扇区坐标
    pub coord: RingSectorCoord,
    /// 合并后的 Mesh 数据
    pub mesh_data: SubMeshData,
    /// 包含的原始 Chunk 坐标列表（用于脏标记追踪）
    pub source_chunks: Vec<ChunkCoord>,
    /// Bevy Mesh Handle
    pub mesh_handle: Handle<Mesh>,
    /// ECS 实体
    pub entity: Entity,
}

/// 环形 Mesh 管理器
#[derive(Resource)]
pub struct RingMeshManager {
    /// 按 LOD 级别和扇区索引存储的 Mesh
    sectors: HashMap<RingSectorCoord, RingSectorMesh>,
    /// 上次更新时的玩家位置
    last_player_chunk: ChunkCoord,
    /// 扇区角度数量（例如 16 = 每个扇区 22.5°）
    sector_count: u32,
    /// 每个 LOD 级别的环数配置
    ring_config: HashMap<LodLevel, u32>,
}
```

### 3.3 扇区划分算法

```rust
impl RingMeshManager {
    /// 根据 Chunk 坐标计算所属扇区
    pub fn chunk_to_sector(
        &self,
        chunk: ChunkCoord,
        player_chunk: ChunkCoord,
    ) -> RingSectorCoord {
        let dx = chunk.cx - player_chunk.cx;
        let dz = chunk.cz - player_chunk.cz;
        
        // 计算距离（环级别）
        let dist = ((dx * dx + dz * dz) as f32).sqrt();
        let lod = LodLevel::from_chunk_distance(dist);
        let ring_level = self.dist_to_ring_level(dist, lod);
        
        // 计算角度（扇区索引）
        let angle = (dz as f32).atan2(dx as f32);
        let sector_index = ((angle + std::f32::consts::PI) 
            / (2.0 * std::f32::consts::PI) 
            * self.sector_count as f32) as u32 % self.sector_count;
        
        RingSectorCoord {
            lod,
            sector_index,
            ring_level,
        }
    }
    
    /// 距离转换为环级别（按 2^n 递增）
    fn dist_to_ring_level(&self, dist: f32, lod: LodLevel) -> u32 {
        let base = match lod {
            LodLevel::Lod0 => return 0, // LOD0 保持独立 Chunk
            LodLevel::Lod1 => 9.0,
            LodLevel::Lod2 => 17.0,
            LodLevel::Lod3 => 25.0,
        };
        ((dist - base) / lod.step() as f32).floor() as u32
    }
}
```

---

## 四、缝补策略

### 4.1 方案选择：Skirt（裙边）+ 深度测试

对于体素地形，推荐使用 **Skirt 方案**，原因：

1. **实现简单**：在每个扇区边界向下延伸一圈裙边
2. **效果够用**：配合雾效和距离淡化，视觉上几乎无感知
3. **性能开销低**：仅增加少量三角形

### 4.2 Skirt 实现

```rust
/// 为扇区 Mesh 添加 Skirt（裙边）
fn add_sector_skirt(
    mesh_data: &mut SubMeshData,
    sector_coord: RingSectorCoord,
    chunk_heights: &[(ChunkCoord, u8)], // 每个 Chunk 的最高方块高度
) {
    let step = sector_coord.lod.step();
    let skirt_depth = 8.0; // 裙边向下延伸 8 个体素
    
    // 遍历扇区边界的所有 Chunk
    for (chunk_coord, max_height) in chunk_heights {
        // 检查是否是扇区边界 Chunk
        if is_sector_boundary(*chunk_coord, sector_coord) {
            // 在边界面上添加向下延伸的四边形
            generate_skirt_quads(
                mesh_data,
                *chunk_coord,
                *max_height,
                skirt_depth,
                step,
            );
        }
    }
}

/// 生成裙边四边形
fn generate_skirt_quads(
    mesh_data: &mut SubMeshData,
    chunk: ChunkCoord,
    max_height: u8,
    depth: f32,
    step: usize,
) {
    let base_y = max_height as f32;
    let skirt_y = base_y - depth;
    
    // 根据边界方向生成四边形
    // ... 省略具体实现
}
```

### 4.3 LOD 边界缝补（进阶）

如果 Skirt 效果不满足需求，可升级为 **邻接 Blending**：

```rust
/// 存储 Chunk 邻接 LOD 信息
pub struct ChunkAdjacency {
    /// 6 个方向的邻居 LOD 级别
    pub neighbor_lods: [Option<LodLevel>; 6],
    /// 边界混合因子（0.0 = 完全当前 LOD，1.0 = 完全邻居 LOD）
    pub blend_factors: [f32; 6],
}

/// 在 Mesh 生成时，根据邻接信息调整边界顶点
fn adjust_boundary_vertices(
    positions: &mut Vec<[f32; 3]>,
    adjacency: &ChunkAdjacency,
    lod: LodLevel,
) {
    for (face_idx, neighbor_lod) in adjacency.neighbor_lods.iter().enumerate() {
        if let Some(neighbor) = neighbor_lod {
            if *neighbor != lod {
                // LOD 边界：将顶点 snap 到低 LOD 网格
                let blend = adjacency.blend_factors[face_idx];
                snap_vertices_to_lower_lod(
                    positions,
                    face_idx,
                    lod,
                    *neighbor,
                    blend,
                );
            }
        }
    }
}
```

---

## 五、增量更新机制

### 5.1 扇区脏标记系统

```rust
/// 扇区脏标记原因
#[derive(Clone, Debug)]
pub enum SectorDirtyReason {
    /// 玩家移动导致扇区重新划分
    PlayerMoved,
    /// 包含的 Chunk 数据变化
    ChunkDataChanged(ChunkCoord),
    /// LOD 级别变化
    LodChanged,
}

/// 扇区脏标记
#[derive(Component)]
pub struct DirtySector {
    pub reason: SectorDirtyReason,
    pub dirty_at: u64, // frame counter
}
```

### 5.2 增量更新流程

```
玩家移动
    │
    ▼
计算新旧玩家位置的扇区划分差异
    │
    ▼
标记新进入视野的扇区为 Dirty
    │
    ▼
异步重建 Dirty 扇区的 Mesh
    │
    ▼
卸载离开视野的扇区
```

### 5.3 更新系统实现

```rust
/// 环形 Mesh 更新系统
pub fn update_ring_meshes(
    mut ring_manager: ResMut<RingMeshManager>,
    player_query: Query<&Transform, With<Player>>,
    loaded_chunks: Res<LoadedChunks>,
    mut commands: Commands,
) {
    let player_transform = player_query.single();
    let player_chunk = ChunkCoord::from_world_pos(player_transform.translation);
    
    // 检查玩家是否移动
    if player_chunk == ring_manager.last_player_chunk {
        return;
    }
    
    let old_chunk = ring_manager.last_player_chunk;
    ring_manager.last_player_chunk = player_chunk;
    
    // 计算扇区差异
    let old_sectors = ring_manager.get_visible_sectors(old_chunk);
    let new_sectors = ring_manager.get_visible_sectors(player_chunk);
    
    // 找出需要新增和移除的扇区
    let to_add: Vec<_> = new_sectors.difference(&old_sectors).collect();
    let to_remove: Vec<_> = old_sectors.difference(&new_sectors).collect();
    
    // 移除离开视野的扇区
    for sector in to_remove {
        if let Some(mesh) = ring_manager.sectors.remove(sector) {
            commands.entity(mesh.entity).despawn();
        }
    }
    
    // 标记新进入视野的扇区为待重建
    for sector in to_add {
        ring_manager.mark_dirty(*sector, SectorDirtyReason::PlayerMoved);
    }
}

/// 异步重建脏扇区
pub fn rebuild_dirty_sectors(
    mut ring_manager: ResMut<RingMeshManager>,
    loaded_chunks: Res<LoadedChunks>,
    async_mesh: Res<AsyncMeshManager>,
    mut commands: Commands,
) {
    let dirty_sectors: Vec<_> = ring_manager.get_dirty_sectors()
        .take(SECTORS_PER_FRAME) // 每帧限制重建数量
        .collect();
    
    for sector_coord in dirty_sectors {
        // 收集扇区内所有 Chunk 的数据
        let chunks = ring_manager.get_chunks_in_sector(
            sector_coord, 
            &loaded_chunks
        );
        
        // 提交异步合并任务
        async_mesh.submit_merge_task(RingMergeTask {
            sector_coord,
            chunks,
        });
    }
}
```

---

## 六、与现有系统集成

### 6.1 模块结构

```
src/
├── ring_mesh/
│   ├── mod.rs              # 模块入口
│   ├── sector.rs           # 扇区坐标和划分算法
│   ├── manager.rs          # RingMeshManager 资源
│   ├── merge.rs            # Chunk 合并逻辑
│   ├── skirt.rs            # Skirt 裙边生成
│   └── dirty.rs            # 扇区脏标记系统
├── lod.rs                  # 现有 LOD 系统（复用）
├── chunk_manager.rs        # 现有 Chunk 管理（修改）
└── async_mesh.rs           # 现有异步 Mesh（扩展）
```

### 6.2 系统调度顺序

```rust
// main.rs 中添加环形 Mesh 系统
app.add_systems(Update, (
    // 阶段 1：LOD 更新（现有）
    manage_chunk_load_state,
    spawn_entities_from_prepare,
    
    // 阶段 2：环形 Mesh 更新（新增）
    update_ring_meshes,           // 检测玩家移动，标记脏扇区
    rebuild_dirty_sectors,        // 异步重建脏扇区
    collect_ring_mesh_results,    // 收集重建结果，上传 GPU
    
    // 阶段 3：脏块重建（现有，仅 LOD0）
    rebuild_dirty_chunks,
).chain());
```

### 6.3 LOD0 保留独立 Chunk

**关键设计决策**：LOD0 区域保持独立 Chunk 渲染，不参与环形合并。

原因：
1. LOD0 是玩家近距离区域，需要支持单个方块的破坏/放置
2. LOD0 的 Chunk 数量有限（~8 区块半径），DrawCall 开销可接受
3. 避免合并后再拆分的复杂性

---

## 七、性能预估

### 7.1 DrawCall 减少

| LOD 级别 | 当前 Chunk 数 | 合并后扇区数 | 减少比例 |
|----------|--------------|-------------|----------|
| LOD0 | ~500 | 500（保持独立） | 0% |
| LOD1 | ~800 | 16 | 98% |
| LOD2 | ~600 | 16 | 97% |
| LOD3 | ~200 | 16 | 92% |
| **总计** | ~2100 | ~548 | **74%** |

### 7.2 内存开销

- 每个扇区 Mesh 数据：~100KB-500KB（取决于地形复杂度）
- 16 扇区 × 3 LOD 级别 × 500KB = **24MB**（峰值）
- 相比当前方案减少约 40% 的 Mesh Handle 开销

### 7.3 重建开销

- 每次玩家移动：重建 1-3 个扇区（新增进入视野的）
- 每个扇区重建：~10-50ms（异步，不阻塞主线程）
- 帧率影响：**< 1ms**（仅主线程的扇区调度逻辑）

---

## 八、实现步骤

### Phase 1：基础框架（1-2 天）

1. 创建 `ring_mesh/` 模块结构
2. 实现 `RingSectorCoord` 和扇区划分算法
3. 实现 `RingMeshManager` 资源
4. 基本的扇区合并 Mesh 生成（无 Skirt）

### Phase 2：缝补与优化（1-2 天）

5. 实现 Skirt 裙边生成
6. 集成到异步 Mesh 系统
7. 实现扇区脏标记和增量更新
8. 性能测试和调优

### Phase 3：集成与测试（1 天）

9. 修改 `chunk_manager.rs`，LOD1+ 使用环形 Mesh
10. 修改 `lod.rs`，支持环形扇区的 LOD 切换
11. 端到端测试和 Bug 修复

---

## 九、风险与对策

| 风险 | 影响 | 对策 |
|------|------|------|
| Skirt 缝补效果不佳 | 视觉瑕疵 | 预留邻接 Blending 升级接口 |
| 扇区重建开销过大 | 帧率下降 | 限制每帧重建数量，使用 LOD 降级 |
| 内存占用过高 | OOM | 实现扇区 LRU 淘汰 |
| 与现有脏块系统冲突 | 数据不一致 | LOD0 使用旧系统，LOD1+ 使用新系统 |

---

## 十、参考文献

1. **Geometry Clipmaps**: GPU Gems 2, Chapter 2 - Terrain Rendering Using GPU-based Geometry Clipmaps
2. **CDLOD**: Filip Strugar, 2010 - Continuous Distance-dependent Level of Detail
3. **dexyfex Voxel LOD**: https://dexyfex.com/2016/07/14/voxels-and-seamless-lod-transitions
4. **Distant Horizons**: Minecraft Mod - Chunk 聚合生成 LOD Mesh

---

*方案制定时间：2026-06-01*
*适用项目：Spirit Realm 体素游戏引擎*
