# 区块渲染问题修复 - 第二次尝试

## 问题描述

1. 区块消失问题仍然存在
2. `chunk_regions` 中的区块数量达到了 2048 的限制
3. 当破坏或放置方块时，区块可以正常显示

## 问题原因

### 1. 卸载检测机制问题

原来的 `chunk_unload_detection_system` 使用帧间比较来检测卸载：
- 比较当前帧和上一帧的 `loaded.entries`
- 如果区块在上一帧存在但本帧不存在，就认为已卸载

但是这个机制有问题：
- 如果区块在同一帧内被卸载和重新加载，可能检测不到
- `last_loaded_chunks` 在第一帧时是空的，导致误判

### 2. 时序问题

- `chunk_unload_detection_system` 在 `chunk_loader_system` 之后运行
- 但是 `update_voxel_render_state` 在另一个系统中运行
- 两个系统之间的时序可能导致问题

## 修复方案

### 直接在 update_voxel_render_state 中检测卸载

移除 `chunk_unload_detection_system`，直接在 `update_voxel_render_state` 中检测卸载：

```rust
pub fn update_voxel_render_state(
    mut render_state: ResMut<VoxelRenderState>,
    mut buffers: ResMut<VoxelBuffers>,
    render_queue: Res<bevy::render::renderer::RenderQueue>,
    loaded: Res<LoadedChunks>,  // 添加 loaded 参数
) {
    // 检测已卸载的区块
    // 遍历 chunk_regions，移除不在 loaded.entries 中的区块
    let coords_to_remove: Vec<_> = buffers.chunk_regions.keys()
        .filter(|coord| !loaded.entries.contains_key(coord))
        .cloned()
        .collect();
    
    for coord in coords_to_remove {
        buffers.remove_chunk(&coord);
        debug!("Removed unloaded chunk from render system: {:?}", coord);
    }

    // ... 其他逻辑
}
```

### 优势

1. **更可靠**：直接检查 `loaded.entries`，不依赖帧间比较
2. **更简单**：不需要额外的系统和状态
3. **更及时**：在更新渲染状态时立即检测卸载

## 修改的文件

1. `src/voxel_render/plugin.rs`
   - 添加 `loaded: Res<LoadedChunks>` 参数
   - 在 `update_voxel_render_state` 中检测卸载
   - 移除对 `chunk_unload_detection_system` 的依赖

2. `src/voxel_render/bridge.rs`
   - 移除 `chunk_unload_detection_system`
   - 简化 `RenderBridgePlugin`

## 预期效果

1. **区块正确显示**：所有区块都能正常渲染
2. **Buffer 区域正确释放**：已卸载的区块会被及时移除
3. **不再有重复上传**：每个区块只上传一次
4. **性能稳定**：区块数量保持在合理范围内

## 测试建议

1. 运行游戏，观察区块是否都能正常显示
2. 检查控制台，确认不再有大量警告
3. 测试区块加载和卸载是否正常
4. 测试破坏和放置方块是否正常

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
