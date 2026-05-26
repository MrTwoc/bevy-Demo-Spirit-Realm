# 缓冲区溢出错误修复总结

## 问题描述

游戏运行时出现两个问题：
1. 有些区块看不到，好像被整个剔除了
2. 游戏崩溃，错误信息：`Copy of 0..41160 would end up overrunning the bounds of the Destination buffer of size 40960`

## 问题原因

1. **缓冲区溢出**：
   - `chunk_regions` 中的区块数量超过了 `MAX_CHUNKS` (2048)
   - `update_indirect_buffer` 函数没有检查区块数量限制
   - 写入的数据超过了预分配的缓冲区大小

2. **区块卸载检测缺失**：
   - 没有检测已卸载的区块
   - `chunk_regions` 中的旧区块没有被移除
   - 导致区块数量持续增长

## 修复方案

### 1. 修复缓冲区溢出

在 `buffers.rs` 中添加了以下保护：

```rust
// 上传区块时检查数量限制
if self.chunk_regions.len() >= super::config::MAX_CHUNKS {
    warn!("Maximum chunk count reached, skipping upload");
    return;
}

// 更新Indirect命令时限制数量
let max_commands = super::config::MAX_CHUNKS;
for (coord, region) in &self.chunk_regions {
    if commands.len() >= max_commands {
        warn!("Maximum command count reached, truncating");
        break;
    }
    // ...
}

// 写入缓冲区时检查大小
let max_bytes = max_commands * std::mem::size_of::<IndirectCommand>();
let bytes_to_write = command_bytes.len().min(max_bytes);
render_queue.write_buffer(indirect_buffer, 0, &command_bytes[..bytes_to_write]);
```

### 2. 添加区块卸载检测

在 `bridge.rs` 中添加了 `chunk_unload_detection_system`：

```rust
pub fn chunk_unload_detection_system(
    loaded: Res<LoadedChunks>,
    mut render_state: ResMut<VoxelRenderState>,
    mut last_loaded_chunks: Local<Vec<crate::chunk::ChunkCoord>>,
) {
    // 获取当前已加载的区块坐标
    let current_chunks: Vec<crate::chunk::ChunkCoord> = loaded.entries.keys().cloned().collect();

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

### 3. 优化处理顺序

在 `plugin.rs` 中调整了处理顺序：

```rust
pub fn update_voxel_render_state(
    mut render_state: ResMut<VoxelRenderState>,
    mut buffers: ResMut<VoxelBuffers>,
    render_queue: Res<bevy::render::renderer::RenderQueue>,
) {
    if !render_state.dirty {
        return;
    }

    // 先处理删除请求
    for coord in render_state.remove_queue.drain(..) {
        buffers.remove_chunk(&coord);
    }

    // 再上传新的Mesh数据
    for mesh_data in render_state.upload_queue.drain(..) {
        buffers.upload_chunk_mesh(&render_queue, &mesh_data);
    }

    // 最后更新Indirect命令缓冲区
    buffers.update_indirect_buffer(&render_queue);

    render_state.dirty = false;
}
```

## 修改的文件

1. `src/voxel_render/buffers.rs`
   - 添加区块数量限制检查
   - 添加命令数量限制
   - 添加缓冲区大小检查
   - 添加 `remove_chunk` 方法

2. `src/voxel_render/bridge.rs`
   - 添加 `chunk_unload_detection_system`
   - 注册新的系统

3. `src/voxel_render/plugin.rs`
   - 优化处理顺序
   - 先删除后添加

## 预期效果

1. **不再崩溃**：缓冲区溢出错误已修复
2. **区块正确卸载**：已卸载的区块会从渲染系统中移除
3. **性能稳定**：区块数量保持在限制范围内

## 测试建议

1. 运行游戏，观察是否还会崩溃
2. 检查区块是否正确加载和卸载
3. 观察控制台输出，确认警告信息

```bash
cargo run
```

## 后续优化

1. **优化卸载检测**：当前使用 Vec 比较，性能较差
   - 可以使用 HashSet 提高查找效率
   - 或者直接在 chunk_manager 中通知渲染系统

2. **优化内存使用**：
   - 实现内存池复用
   - 实现增量更新

3. **优化渲染性能**：
   - 实现真正的 MultiDrawIndirect
   - 实现 GPU 端剔除
