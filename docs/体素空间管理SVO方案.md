# 体素空间管理方案：三维区块 + SVO

> 本文档描述如何使用 **三维区块（3D Chunking）** 搭配 **稀疏八叉树（SVO）** 实现高效的空间管理和渲染。
> 可作为 `体素管理方案.md` 的扩展补充。

---

## 1. 核心思想

传统方案的问题：
- **Flat array（扁平数组）**：所有 32³ 体素全量存储，内存浪费严重
- **单纯 Chunk 方案**：跨区块查询（如射线检测、空间范围查询）需要遍历多个 Chunk，O(n) 复杂度

我们的目标：
- **内存效率**：只存储"有内容的"体素，空区域不占内存
- **查询效率**：跨区块/跨大范围查询（射线、碰撞、范围搜索）控制在 O(log n)
- **渲染友好**：天然支持 LoD，根节点 = 低精度，叶子 = 高精度

---

## 2. 分层结构设计

```
World
└── Chunk Grid（32³ per chunk）
    └── 每个 Chunk 内部是一棵 SVO
```

### 2.1 为什么这样分层？

| 层级 | 粒度 | 作用 |
|------|------|------|
| Chunk Grid | 32³ | 世界的宏观划分，方便按需加载/卸载、并发处理 |
| SVO（每 Chunk 内） | 1~32³ 可变 | Chunk 内部的空间管理，实现内存效率和快速查询 |

这样既有 Flat Array 的局部连续性（单 Chunk 访问快），又有 Octree 的稀疏表达能力。

---

## 3. SVO 数据结构

### 3.1 八叉树节点

```rust
/// SVO 节点
/// 分支节点：8 个子节点不全为空
/// 叶子节点：8 个子节点全为空（可能存储实际体素数据）
pub enum SvoNode {
    /// 分支节点：8 个孩子的指针（或索引）
    Branch([Option<NodeIndex>; 8]),
    /// 叶子节点：存储该 8³ 子区域内的体素数据
    Leaf([BlockId; 8 * 8 * 8]),
}

pub struct NodeIndex(u32); // 节点在节点池中的索引
```

### 3.2 逐层展开示意

```
根节点（覆盖整个 32³ Chunk）
├── 分支节点（覆盖 16³）
│   ├── 分支节点（覆盖 8³）
│   │   ├── 叶子节点（8³ 实际体素）
│   │   └── ...
│   └── 叶子节点（16³ 全空气，直接跳过）
└── 分支节点（16³）
    └── ...
```

### 3.3 体素获取（O(log 8) = O(1)）

```rust
fn get_voxel(root: NodeIndex, x: u8, y: u8, z: u8) -> BlockId {
    let mut node = root;
    let mut size = 32u8;
    let mut offset = (0u8, 0u8, 0u8);

    while let SvoNode::Branch(children) = node_pool[node] {
        size /= 2;
        let octant = compute_octant(x, y, z, offset, size);
        match children[octant] {
            Some(child) => node = child,
            None => return AIR, // 全空气子树
        }
        offset = offset + octant_offset(octant, size);
    }

    // 到达叶子，从叶子体素数组中取
    if let SvoNode::Leaf(data) = node_pool[node] {
        let local_idx = ((x % 8) * 8 + (y % 8)) * 8 + (z % 8);
        return data[local_idx]
    }

    unreachable!()
}
```

---

## 4. SVO 的优势

### 4.1 内存效率

```
场景对比：

Flat array（32³ = 32768 体素）：
- 全空气区块：32768 字节（全要分配）

SVO：
- 全空气区块：1 个根节点，约 4 字节（指向空）
- 稀疏地形：仅对"有内容"的节点分配内存
```

实际游戏中，90%+ 的空间是空气，SVO 可以把内存降到原来的 1/10 甚至更低。

### 4.2 射线检测（Ray Marching）

SVO 的天然优势——沿射线逐层推进：

```rust
fn raycast(root: NodeIndex, origin: Vec3, dir: Vec3) -> Option<Hit> {
    let mut t = 0.0;
    let mut pos = origin;
    let mut node = root;
    let mut size = 32.0f32;

    loop {
        match node_pool[node] {
            SvoNode::Branch(children) => {
                // 找到射线穿过的下一个子节点
                let (next_node, next_t, next_size) = traverse_branch(
                    children, pos, dir, t, size
                );
                t = next_t;
                size = next_size;
                node = next_node;
            }
            SvoNode::Leaf(voxels) => {
                // 在叶子 8³ 中精确检测
                return raycast_leaf(voxels, pos, dir);
            }
        }
    }
}
```

每步跳进一个子节点，复杂度 O(log 32) ≈ **6 步**，远快于遍历 32768 个体素。

### 4.3 天然支持 LoD

```
深度 0（根）：32³ 精度极低，全节点 = 1 个三角形
深度 1：16³ 精度
深度 2：8³ 精度
...
深度 5（叶子）：1³ 全精度
```

渲染远处时，直接用浅层节点即可，不用构建额外的 LOD 网格。

---

## 5. 区块 + SVO 混合架构

### 5.1 区块内 SVO 的存储

```rust
pub struct ChunkSvo {
    /// 节点池，类似 vector 存储所有节点
    nodes: Vec<SvoNode>,
    /// 根节点索引
    root: NodeIndex,
    /// 元数据
    metadata: ChunkMetadata,
}

pub struct ChunkMetadata {
    /// 区块坐标
    coord: ChunkCoord,
    /// 是否全空气（可用于快速跳过）
    is_empty: bool,
    /// 是否全均匀（可用 Uniform 优化）
    uniform_block: Option<BlockId>,
    /// 最后访问时间（用于缓存淘汰）
    last_access: u64,
}
```

### 5.2 修改操作

```rust
/// 设置体素（O(log 32) = O(1)）
fn set_voxel(chunk: &mut ChunkSvo, x: u8, y: u8, z: u8, block_id: BlockId) {
    // 找到对应叶子，修改体素数组
    // 如果叶子变为全空气，可删除该叶子节点
    // 如果整个分支全空气，可删除整个分支
}
```

### 5.3 面提取（Mesh Generation）

仍然需要遍历所有"有内容的"节点，提取暴露面。但 SVO 结构让这个过程更高效：

```rust
fn extract_faces(chunk: &ChunkSvo) -> MeshData {
    let mut positions = Vec::new();
    let mut uvs = Vec::new();
    let mut normals = Vec::new();
    let mut indices = Vec::new();

    extract_faces_recursive(chunk, chunk.root, 32, 0, 0, 0, &mut positions, &mut uvs, &mut normals, &mut indices);

    return MeshData { positions, uvs, normals, indices };
}

fn extract_faces_recursive(
    chunk: &ChunkSvo,
    node_idx: NodeIndex,
    size: u8,
    x: u8, y: u8, z: u8,
    out: &mut MeshData,
) {
    match &chunk.nodes[node_idx] {
        SvoNode::Branch(children) => {
            let half = size / 2;
            for (i, child) in children.iter().enumerate() {
                if let Some(child_idx) = child {
                    let (cx, cy, cz) = octant_offset(i, half);
                    extract_faces_recursive(chunk, *child_idx, half, x+cx, y+cy, z+cz, out);
                }
            }
        }
        SvoNode::Leaf(voxels) => {
            // 对 8³ 体素逐个检查 6 个面
            extract_leaves_faces(voxels, x, y, z, out);
        }
    }
}
```

---

## 6. 与当前实现的对比

| | 当前方案（Vec 数组）| + SVO |
|---|---|---|
| 内存（全空气 Chunk） | 0 字节（Empty） | 0 字节（Empty） |
| 内存（稀疏 Chunk） | 32³ u8 = 32 KB | 按需分配，远小于 32 KB |
| 射线检测 | O(32768) 遍历 | O(log 32) ≈ O(1) |
| 空间范围查询 | O(n) 遍历 | O(log n) |
| 实现复杂度 | 低 | 中高 |
| 适合场景 | 单区块、方块数少 | 无限世界、密集场景 |

---

## 7. 实现路径建议

### Phase 1：先把 SVO 引入单 Chunk（不改整体架构）

- [ ] `src/svo.rs` 模块
- [ ] `SvoNode` 枚举 + `NodePool`
- [ ] `get_voxel` / `set_voxel`
- [ ] 用 SVO 替代 `ChunkData::Mixed(Vec<u8>)`（保持 Chunk Grid 结构不变）
- [ ] 实现基于 SVO 的射线检测

### Phase 2：整合 Chunk 调度

- [ ] `ChunkManager` 管理 SVO Chunk 的加载/卸载
- [ ] 与 `DirtyChunk` 系统联动

### Phase 3：渲染优化

- [ ] 利用 SVO 节点做视锥剔除
- [ ] 利用 SVO 节点做 LOD
- [ ] 考虑 GPU 侧 SVO 遍历

---

## 8. 参考资料

- C++ voxel engine using octrees：[https://0xc0de.dev/](https://0xc0de.dev/)
- Octree-based voxel rendering paper
- Minecraft's chunk loading system（Chunk Loading - Minecraft Wiki）
- Bevy 生态：`bevy_voxel` / `smooth voxlation` 相关 crate
