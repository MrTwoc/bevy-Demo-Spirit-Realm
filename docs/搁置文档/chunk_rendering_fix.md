# 区块渲染问题修复总结

## 问题描述

1. 有些区块看不见，但破坏或放置方块后就能显示
2. 控制台输出大量的 "Maximum chunk count reached, skipping upload" 警告

## 问题原因

### 1. 重复上传问题

原来的代码中，`upload_chunk_mesh` 函数没有检查区块是否已经存在：
- 同一个区块被多次上传
- 每次上传都会分配新的 Buffer 区域
- 旧的 Buffer 区域没有被释放
- 导致 Buffer 区域泄漏

### 2. 卸载检测问题

原来的 `chunk_unload_detection_system` 使用 `Vec` 存储已加载的区块：
- `Vec.contains()` 的时间复杂度是 O(n)
- 每帧都要遍历所有区块
- 性能较差

### 3. 去重逻辑缺失

`render_bridge_system` 没有检查是否已经在上传队列中：
- 同一个区块可能被多次添加到上传队列
- 导致重复上传

## 修复方案

### 1. 修复重复上传

在 `buffers.rs` 的 `upload_chunk_mesh` 中添加：

```rust
// 如果区块已存在，先释放旧的Buffer区域
if let Some(old_region) = self.chunk_regions.remove(&mesh_data.coord) {
    self.allocator.free(old_region);
}
```

### 2. 优化卸载检测

在 `bridge.rs` 中使用 `HashSet` 替代 `Vec`：

```rust
use std::collections::HashSet;

pub fn chunk_unload_detection_system(
    loaded: Res<LoadedChunks>,
    mut render_state: ResMut<VoxelRenderState>,
    mut last_loaded_chunks: Local<HashSet<ChunkCoord>>,  // 使用 HashSet
) {
    // 获取当前已加载的区块坐标
    let current_chunks: HashSet<ChunkCoord> = loaded.entries.keys().cloned().collect();

    // 检测已卸载的区块
    for coord in last_loaded_chunks.iter() {
        if !current_chunks.contains(coord) {
            render_state.remove_queue.push(*coord);
            render_state.dirty = true;
        }
    }

    // 更新上一帧的区块列表
    *last_loaded_chunks = current_chunks;
}
```

### 3. 添加去重逻辑

在 `bridge.rs` 的 `render_bridge_system` 中添加去重：

```rust
// 检查是否已经在上传队列中（去重）
let already_queued = render_state
    .upload_queue
    .iter()
    .any(|m| m.coord == result.coord);
if already_queued {
    // 更新已存在的条目
    if let Some(existing) = render_state
        .upload_queue
        .iter_mut()
        .find(|m| m.coord == result.coord)
    {
        *existing = ChunkMeshData { ... };
    }
    continue;
}
```

## 修改的文件

1. `src/voxel_render/buffers.rs`
   - 添加重复区块处理逻辑
   - 释放旧的 Buffer 区域
   - 优化警告信息

2. `src/voxel_render/bridge.rs`
   - 使用 HashSet 替代 Vec
   - 添加去重逻辑
   - 优化卸载检测

## 预期效果

1. **区块正确显示**：所有区块都能正常渲染
2. **不再有重复上传**：每个区块只上传一次
3. **Buffer 区域正确释放**：旧区块的 Buffer 区域会被复用
4. **性能提升**：卸载检测使用 HashSet，效率更高

## 测试建议

1. 运行游戏，观察区块是否都能正常显示
2. 检查控制台，确认不再有大量警告
3. 测试区块加载和卸载是否正常

```bash
cargo run
```

## 后续优化

1. **优化 Buffer 管理**：
   - 实现内存池复用
   - 实现增量更新

2. **优化渲染性能**：
   - 实现真正的 MultiDrawIndirect
   - 实现 GPU 端剔除

3. **优化区块加载**：
   - 实现优先级队列
   - 实现预测加载
