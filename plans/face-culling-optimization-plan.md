# 区块间无效拼接面剔除 — 实施计划

## 一、概述

### 1.1 业务场景
玩家在体素世界中探索地下区域时，当前渲染引擎生成了大量**区块间内部拼接面**。例如：石头(2)与泥土(3)相邻处、泥土(3)与草地(1)相邻处的地层过渡边界。这些面虽然相邻方块类型不同，但**两者都是实体方块、彼此完全遮挡、玩家不可见**，却占用了大量 GPU 三角面预算，导致帧率下降。

### 1.2 用户角色
| 角色 | 关注点 |
|------|--------|
| 玩家 | 游戏流畅度（FPS）、视觉正确性（不应看到内部面闪动） |
| 开发者 | 代码可维护性、修改影响范围可控、可验证 |

### 1.3 技术栈约束
- **语言**：Rust（无 `unsafe` 代码，纯安全 Rust）
- **引擎**：Bevy 0.15+
- **网格生成**：纯 CPU 端面剔除算法（无 GPU culling）
- **并发**：异步网格生成使用 `mpsc` 通道 + 后台线程
- **渲染管线**：自定义 `VoxelMaterial`（纹理数组）

---

## 二、功能模块与修改范围

| 模块 | 文件 | 修改类型 | 修改说明 |
|------|------|----------|----------|
| **M1 - 核心工具函数** | `src/chunk.rs` | 新增 `pub fn is_block_solid()` | 定义实体方块判定规则，供所有网格生成模块共享 |
| **M2 - 同步面剔除** | `src/chunk.rs` | 重写 `is_face_visible()` | 将 `neighbor_id != current_id` 改为 `!is_block_solid(neighbor_id)` |
| **M3 - 异步面剔除** | `src/async_mesh.rs` | 重写 `is_face_visible_async()` | 同上逻辑迁移，修正 import |
| **M4 - LOD 面剔除** | `src/lod.rs` | 重写 `is_face_visible_lod()` | 同上逻辑迁移，修正 import |
| **M5 - 脏标记修复** | `src/chunk_manager.rs` | 移除 `NEIGHBOR_DIRTY_PER_FRAME` 限制 | 确保所有新加载区块的邻居都被正确标记重建 |

---

## 三、详细技术方案

### 3.1 实体方块定义 (`is_block_solid`)

```rust
/// 判断方块 ID 是否为实体（不透明）方块。
///
/// 实体方块：草地(1)、石头(2)、泥土(3)、沙(4) — 完全遮挡相邻面。
/// 非实体方块：空气(0)、水(5) — 不遮挡面。
///
/// # 返回值
/// - `true`：该方块是实心的，可以遮挡相邻面
/// - `false`：该方块是透明的，不遮挡相邻面
#[inline]
pub fn is_block_solid(block_id: BlockId) -> bool {
    match block_id {
        0 | 5 => false, // 空气 / 水 → 非实体
        _ => true,      // 草地(1)、石头(2)、泥土(3)、沙(4) → 实体
    }
}
```

### 3.2 面可见性判断规则（核心算法）

```
输入：当前方块ID、邻居方块ID
输出：是否应渲染此面

规则：
  if 当前方块是空气(0) → 不渲染（循环中已跳过 air）
  if 邻居方块是空气(0) 或 水(5) → 渲染（暴露在透明面）
  if 邻居方块是实体(1/2/3/4) → 不渲染（被完全遮挡）
  【注意】不再关心 current_id 和 neighbor_id 是否相同！
```

### 3.3 脏标记修复

**问题**：`NEIGHBOR_DIRTY_PER_FRAME = 16` 在加载 16 个区块时有最多 96 个邻居需标记，但限制为 16，导致超出部分永久遗漏。

**修复**：移除数量限制，`dirty_neighbors` 收集所有邻居，批量应用。

```rust
// 修改前（有速率限制）：
let mut neighbor_dirty_remaining = NEIGHBOR_DIRTY_PER_FRAME;
for (dx, dy, dz) in NEIGHBOR_OFFSETS.iter() {
    if neighbor_dirty_remaining == 0 { break; }
    // ... 收集脏标记 ...
    neighbor_dirty_remaining -= 1;
}

// 修改后（无速率限制，全量标记）：
for (dx, dy, dz) in NEIGHBOR_OFFSETS.iter() {
    // ... 收集脏标记（无限制）...
}
```

---

## 四、交付标准

### 4.1 功能验收标准
| 检查项 | 预期结果 | 验证方式 |
|--------|----------|----------|
| 地表草地-空气面 | 正常渲染，不变 | 肉眼观察、三角面计数 |
| 石头侧壁紧贴泥土 | 不渲染内部拼接面 | 三角面计数减少 |
| 泥土紧贴草地 | 不渲染内部拼接面 | 三角面计数减少 |
| 完全地下全石头区块 | 三角面数降至 ~0 | 三角面计数归零 |
| 水与实体方块界面 | 正常渲染（水边可见） | 肉眼观察水下洞穴边缘 |
| 跨区块边界面 | 正确剔除 | Debug 模式下确认 |
| LOD 网格 | 同样优化，无异常 | 远距离观察无闪烁 |

### 4.2 性能指标
| 指标 | 优化前（典型值） | 优化后（预期） | 要求 |
|------|-----------------|---------------|------|
| 完整地形总三角面数 | ~2000万 | ~500万 | 减少 ≥ 70% |
| 地下区块平均三角面数 | ~3000 | ~0（全遮挡） | 减少 ≥ 95% |
| 地层过渡区块面数 | ~4000 | ~1000 | 减少 ≥ 75% |
| 加载时间（同区块数） | 基准 | 不变或略优 | 不增加 |
| 帧率（FPS） | 基准 +20% | - | 显著提升 |

### 4.3 代码质量标准
- `cargo build` 零警告通过
- `cargo clippy` 零 lint 警告（原有警告除外）
- 所有新增函数必须有 `///` doc comment
- 面剔除函数必须使用 `#[inline]` 标记
- 逻辑变更必须附带注释说明优化原理

---

## 五、错误处理规范

### 5.1 边界条件
| 条件 | 处理方式 |
|------|----------|
| 邻居区块不存在（未加载） | `get_neighbor_block` 返回 0（空气），视为可见面 |
| 坐标越界（`x >= CHUNK_SIZE`） | 回退到邻居查询，邻居不存在则返回 0 |
| 空区块（`ChunkData::Empty`） | `generate_chunk_mesh` 入口处已 `return`，无需特殊处理 |
| Uniform 区块（全同一种方块） | `is_block_solid` 按 ID 正常判断 |

### 5.2 降级策略
如果优化后地形渲染出现异常（如水边面丢失），最大可能是 `is_block_solid(5)` 返回了 `true`。立即检查 `is_block_solid` 中 BlockId=5 的分支是否返回 `false`。

---

## 六、实施步骤

```
步骤 1 ── 新增 is_block_solid()
         ├─ src/chunk.rs: 在 BlockId 类型别名后插入
         └─ 验证：cargo build 通过且可被 use 导入

步骤 2 ── 重写 is_face_visible()
         ├─ src/chunk.rs: 修改第 276-308 行
         └─ 逻辑：neighbor_id != current_id → !is_block_solid(neighbor_id)

步骤 3 ── 重写 is_face_visible_async()
         ├─ src/async_mesh.rs: 修改第 370-401 行
         ├─ 修正 import（添加 is_block_solid）
         └─ 逻辑同上

步骤 4 ── 重写 is_face_visible_lod()
         ├─ src/lod.rs: 修改第 316-364 行
         ├─ 修正 import（添加 is_block_solid）
         └─ 逻辑同上，current_id 参数保留但不再使用

步骤 5 ── 修复脏标记
         ├─ src/chunk_manager.rs: 修改第 337-438 行
         ├─ 移除 NEIGHBOR_DIRTY_PER_FRAME 计数限制
         └─ 移除 neighbor_dirty_remaining 相关代码

步骤 6 ── 编译验证与性能评测
         ├─ cargo build （零错误/零新警告）
         ├─ cargo clippy （清理可修复问题）
         └─ 手动运行测试场景确认优化效果
```

---

## 七、风险与回滚

| 风险 | 概率 | 影响 | 缓解措施 |
|------|------|------|----------|
| 水边界面丢失 | 低 | 中等 | 单元验证 `is_block_solid(5)=false` |
| LOD 过渡闪烁 | 低 | 低 | LOD 降采样使用 `sample_dominant_block`，步长跨越的面已被 `is_block_solid` 覆盖 |
| 脏标记风暴 | 低 | 中等 | 移除速率限制后，新增区块每个最多 6 个邻居，可控 |
| 回滚策略 | - | - | git revert OR checkout backups/*.rs |
