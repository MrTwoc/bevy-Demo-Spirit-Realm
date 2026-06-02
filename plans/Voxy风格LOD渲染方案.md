# Voxy 风格 LOD 渲染方案

> 严格借鉴 Minecraft Voxy 模组实现，适配 Bevy ECS 架构

---

## 一、Voxy 核心架构分析

### 1.1 Voxy 关键发现

通过阅读 Voxy 源码（`voxy-dev`），发现其核心特点：

| 特性 | Voxy 实现 | 说明 |
|------|-----------|------|
| **LOD 管理** | 八叉树（Octree） | 每个节点 16 字节，层级遍历 |
| **Chunk 合并** | **不合并** | 每个 Section (32³) 独立生成 Mesh |
| **剔除方式** | GPU Compute Shader | HiZ 遮挡 + 视锥体剔除 |
| **渲染方式** | Multi Draw Indirect | 批量绘制调用 |
| **Mesh 格式** | 紧凑 Quad (8字节) | 极致压缩，减少 GPU 带宽 |

### 1.2 Voxy 数据结构

```
┌─────────────────────────────────────────────────────────────┐
│                    Node 结构 (16 bytes)                      │
├─────────────────────────────────────────────────────────────┤
│  rawPos (uvec2)    │ meshPtr (24bit) │ childPtr (24bit)     │
│  位置 + LOD级别     │ 几何体索引       │ 子节点列表指针        │
│                    │ flags (8bit)    │ flags (8bit)         │
└─────────────────────────────────────────────────────────────┘
```

### 1.3 Voxy 渲染管线

```
┌─────────────────────────────────────────────────────────────┐
│                    Voxy 渲染管线                              │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  Compute Shader (traversal_dev.comp)                        │
│  ├── 从顶层节点开始遍历                                       │
│  ├── 视锥体剔除 (6平面测试)                                   │
│  ├── HiZ 遮挡剔除 (Hierarchical Z-Buffer)                    │
│  ├── 屏幕空间误差判断 (shouldDecend)                          │
│  └── 输出渲染队列 + 请求队列                                  │
│                                                             │
│  渲染阶段                                                    │
│  ├── 读取渲染队列                                            │
│  ├── 对每个 section 执行 Indirect Draw                       │
│  └── 使用 MDIC (Multi Draw Indirect Count)                   │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

---

## 二、适配 Bevy 的设计方案

### 2.1 设计原则

**严格借鉴 Voxy，但适配 Bevy 架构**：

1. **不合并 Chunk** — 每个 Chunk 独立生成 Mesh（与 Voxy 一致）
2. **层级遍历** — 使用八叉树或空间索引进行 LOD 遍历
3. **GPU 剔除** — 尽可能将剔除逻辑移到 GPU 端
4. **批量渲染** — 使用 Bevy 的批量渲染能力减少 DrawCall

### 2.2 架构对比

| 组件 | Voxy (Java/OpenGL) | Spirit Realm (Bevy/Rust) |
|------|-------------------|--------------------------|
| LOD 数据结构 | 八叉树 NodeStore | LodManager + HashMap |
| 遍历方式 | GPU Compute Shader | CPU 系统 + GPU 剔除 |
| 剔除方式 | HiZ + 视锥体 (GPU) | SVO + 视锥体 (CPU) |
| 渲染方式 | Multi Draw Indirect | Bevy Mesh + PBR |
| Mesh 格式 | 紧凑 Quad (8字节) | 标准顶点缓冲 |

---

## 三、核心模块设计

### 3.1 模块结构

```
src/
├── lod_octree/
│   ├── mod.rs              # 模块入口
│   ├── node.rs             # 节点数据结构（借鉴 Voxy NodeStore）
│   ├── store.rs            # 节点存储管理
│   ├── traverse.rs         # CPU 端遍历逻辑
│   └── request.rs          # 节点请求管理
├── gpu_cull/
│   ├── mod.rs              # GPU 剔除模块
│   ├── hiz.rs              # HiZ Buffer 实现
│   ├── traversal.wgsl      # 遍历 Compute Shader
│   └── cull.wgsl           # 剔除 Compute Shader
├── render_queue/
│   ├── mod.rs              # 渲染队列管理
│   ├── batch.rs            # 批量渲染逻辑
│   └── indirect.rs         # Indirect Draw 支持
├── section_mesh/
│   ├── mod.rs              # Section Mesh 生成
│   ├── compact.rs          # 紧凑格式（借鉴 Voxy 8字节 quad）
│   └── factory.rs          # Mesh 工厂
└── lod.rs                  # 现有 LOD 系统（保留）
```

### 3.2 节点数据结构（借鉴 Voxy NodeStore）

```rust
/// 借鉴 Voxy 的节点存储设计
/// 每个节点 16 字节，紧凑存储
#[repr(C)]
#[derive(Clone, Copy)]
pub struct OctreeNode {
    /// 位置 + LOD 级别（8 字节）
    pub raw_pos: u64,
    /// Mesh 指针 (24bit) + 子节点指针 (24bit) + flags (16bit)
    pub data: u64,
}

impl OctreeNode {
    /// 获取 LOD 级别
    pub fn lod_level(&self) -> u32 {
        ((self.raw_pos >> 60) & 0xF) as u32
    }
    
    /// 获取位置
    pub fn position(&self) -> [i32; 3] {
        let x = ((self.raw_pos >> 40) & 0xFFFFF) as i32;
        let y = ((self.raw_pos >> 20) & 0xFFFFF) as i32;
        let z = (self.raw_pos & 0xFFFFF) as i32;
        [x, y, z]
    }
    
    /// 获取 Mesh 指针
    pub fn mesh_ptr(&self) -> Option<u32> {
        let ptr = (self.data & 0xFFFFFF) as u32;
        if ptr == 0xFFFFFF { None } else { Some(ptr) }
    }
    
    /// 获取子节点指针
    pub fn child_ptr(&self) -> Option<u32> {
        let ptr = ((self.data >> 24) & 0xFFFFFF) as u32;
        if ptr == 0xFFFFFF { None } else { Some(ptr) }
    }
    
    /// 是否有 Mesh
    pub fn has_mesh(&self) -> bool {
        self.mesh_ptr().is_some()
    }
    
    /// 是否有子节点
    pub fn has_children(&self) -> bool {
        self.child_ptr().is_some()
    }
}

/// 节点存储管理器（借鉴 Voxy NodeStore）
pub struct NodeStore {
    nodes: Vec<OctreeNode>,
    allocation_set: HierarchicalBitSet,
}

impl NodeStore {
    pub fn new(max_nodes: u32) -> Self {
        Self {
            nodes: vec![OctreeNode::EMPTY; max_nodes as usize],
            allocation_set: HierarchicalBitSet::new(max_nodes),
        }
    }
    
    /// 分配新节点
    pub fn allocate(&mut self) -> Option<u32> {
        self.allocation_set.allocate_next()
    }
    
    /// 释放节点
    pub fn free(&mut self, node_id: u32) {
        self.allocation_set.free(node_id);
        self.nodes[node_id as usize] = OctreeNode::EMPTY;
    }
    
    /// 获取节点
    pub fn get(&self, node_id: u32) -> &OctreeNode {
        &self.nodes[node_id as usize]
    }
    
    /// 设置节点
    pub fn set(&mut self, node_id: u32, node: OctreeNode) {
        self.nodes[node_id as usize] = node;
    }
}
```

### 3.3 遍历系统（借鉴 Voxy HierarchicalOcclusionTraverser）

```rust
/// 遍历状态
pub struct TraversalState {
    /// 待处理队列
    queue: Vec<u32>,
    /// 渲染队列
    render_queue: Vec<u32>,
    /// 请求队列（需要加载的节点）
    request_queue: Vec<u32>,
}

/// 遍历系统（CPU 端实现，借鉴 Voxy GPU 遍历逻辑）
pub fn traverse_octree(
    node_store: &NodeStore,
    camera: &Camera,
    render_distance: f32,
) -> TraversalState {
    let mut state = TraversalState::new();
    
    // 从顶层节点开始遍历
    for &top_node in node_store.top_level_nodes() {
        state.queue.push(top_node);
    }
    
    while let Some(node_id) = state.queue.pop() {
        let node = node_store.get(node_id);
        
        // 检查是否在渲染距离内
        if !is_within_render_distance(node, camera, render_distance) {
            continue;
        }
        
        // 视锥体剔除
        if is_outside_frustum(node, camera) {
            continue;
        }
        
        // 计算屏幕空间误差
        let should_descend = should_descend(node, camera);
        
        if should_descend && node.has_children() {
            // 下降到子节点
            let child_ptr = node.child_ptr().unwrap();
            let child_count = node.child_count();
            for i in 0..child_count {
                state.queue.push(child_ptr + i);
            }
        } else if node.has_mesh() {
            // 添加到渲染队列
            state.render_queue.push(node_id);
        } else {
            // 请求加载 Mesh
            state.request_queue.push(node_id);
            
            // 如果有子节点，也遍历子节点
            if node.has_children() {
                let child_ptr = node.child_ptr().unwrap();
                let child_count = node.child_count();
                for i in 0..child_count {
                    state.queue.push(child_ptr + i);
                }
            }
        }
    }
    
    state
}

/// 屏幕空间误差判断（借鉴 Voxy shouldDecend）
fn should_descend(node: &OctreeNode, camera: &Camera) -> bool {
    let node_pos = node.position();
    let lod_level = node.lod_level();
    
    // 计算节点在屏幕上的像素大小
    let node_size = 32 << lod_level; // 每个 LOD 级别大小翻倍
    let distance = distance_to_camera(node_pos, camera);
    
    // 如果节点在屏幕上大于阈值像素，应该下降
    let pixel_size = (node_size as f32 / distance) * camera.projection_scale();
    pixel_size > SCREEN_PIXEL_THRESHOLD
}
```

### 3.4 渲染队列与批量渲染

```rust
/// 渲染队列（借鉴 Voxy renderQueue）
#[derive(Resource)]
pub struct RenderQueue {
    /// 待渲染的 Mesh ID 列表
    pub mesh_ids: Vec<u32>,
    /// 每帧最大渲染数量
    pub max_per_frame: usize,
}

/// 批量渲染系统
pub fn batch_render(
    render_queue: Res<RenderQueue>,
    mesh_store: Res<MeshStore>,
    mut render_pass: ResMut<RenderPass>,
) {
    // 按材质分组
    let mut by_material: HashMap<Handle<Material>, Vec<u32>> = HashMap::new();
    
    for &mesh_id in &render_queue.mesh_ids {
        let mesh = mesh_store.get(mesh_id);
        by_material
            .entry(mesh.material.clone())
            .or_default()
            .push(mesh_id);
    }
    
    // 对每个材质组执行批量渲染
    for (material, mesh_ids) in by_material {
        render_pass.set_material(material);
        
        // 使用 DrawIndexed 批量绘制
        for &mesh_id in &mesh_ids {
            let mesh = mesh_store.get(mesh_id);
            render_pass.draw_indexed(
                mesh.index_offset,
                mesh.index_count,
                mesh.vertex_offset,
            );
        }
    }
}
```

---

## 四、Mesh 生成（借鉴 Voxy RenderDataFactory）

### 4.1 Section Mesh 生成

```rust
/// Section Mesh 数据（借鉴 Voxy BuiltSection）
pub struct SectionMesh {
    /// 位置
    pub position: [i32; 3],
    /// 子节点存在性掩码
    pub child_existence: u8,
    /// AABB
    pub aabb: Aabb,
    /// 几何体数据
    pub geometry: GeometryBuffer,
    /// 各方向的偏移量
    pub offsets: [u32; 8],
}

/// 几何体缓冲区
pub struct GeometryBuffer {
    /// 顶点数据（紧凑格式）
    pub vertices: Vec<CompactVertex>,
    /// 索引数据
    pub indices: Vec<u32>,
}

/// 紧凑顶点格式（借鉴 Voxy 8字节 quad）
#[repr(C)]
#[derive(Clone, Copy)]
pub struct CompactVertex {
    /// 位置 (3 * 10bit = 30bit) + 法线方向 (3bit) + 模型ID (13bit) + 光照 (4bit)
    pub data0: u32,
    /// UV 坐标 (2 * 8bit = 16bit) + 生物群系颜色 (16bit)
    pub data1: u32,
}

impl CompactVertex {
    pub fn new(
        x: u16, y: u16, z: u16,
        normal: u8,
        model_id: u16,
        light: u8,
        u: u8, v: u8,
        biome_color: u16,
    ) -> Self {
        Self {
            data0: (x as u32 & 0x3FF)
                | ((y as u32 & 0x3FF) << 10)
                | ((z as u32 & 0x3FF) << 20)
                | ((normal as u32 & 0x7) << 30),
            data1: (u as u32 & 0xFF)
                | ((v as u32 & 0xFF) << 8)
                | ((biome_color as u32 & 0xFFFF) << 16),
        }
    }
}

/// Section Mesh 生成器（借鉴 Voxy RenderDataFactory）
pub struct SectionMeshFactory {
    /// 方块模型查询
    model_queries: ModelQueries,
    /// 邻居数据缓冲区
    neighbor_buffer: NeighborBuffer,
}

impl SectionMeshFactory {
    /// 生成 Section Mesh
    pub fn generate(
        &mut self,
        section: &WorldSection,
        neighbors: &ChunkNeighbors,
    ) -> SectionMesh {
        // 1. 准备数据：将方块 ID 转换为模型 ID
        let section_data = self.prepare_section_data(section);
        
        // 2. 获取邻居面数据
        let neighbor_faces = self.acquire_neighbor_data(neighbors);
        
        // 3. 生成各方向的面
        let mut geometry = GeometryBuffer::new();
        
        // YZ 平面
        self.generate_yz_faces(&section_data, &neighbor_faces, &mut geometry);
        
        // X 平面
        self.generate_x_faces(&section_data, &neighbor_faces, &mut geometry);
        
        // 4. 计算偏移量
        let offsets = self.calculate_offsets(&geometry);
        
        SectionMesh {
            position: section.position(),
            child_existence: section.child_existence(),
            aabb: section.aabb(),
            geometry,
            offsets,
        }
    }
    
    /// 生成 YZ 平面的面
    fn generate_yz_faces(
        &self,
        section_data: &[u64],
        neighbor_faces: &[u64],
        geometry: &mut GeometryBuffer,
    ) {
        // 使用 ScanMesher2D 算法合并相邻同类型面
        for x in 0..32 {
            for z in 0..32 {
                // 扫描 Y 方向，合并连续的同类型方块
                let mut y = 0;
                while y < 32 {
                    let block_id = get_block_id(section_data, x, y, z);
                    if block_id == 0 {
                        y += 1;
                        continue;
                    }
                    
                    // 找到连续的同类型方块
                    let mut length = 1;
                    while y + length < 32 
                        && get_block_id(section_data, x, y + length, z) == block_id 
                    {
                        length += 1;
                    }
                    
                    // 生成面
                    if self.should_generate_face(section_data, x, y, z, FaceDirection::Y) {
                        self.emit_quad(
                            geometry,
                            [x, y, z],
                            FaceDirection::Y,
                            length as u16,
                            1, // width
                            block_id,
                        );
                    }
                    
                    y += length;
                }
            }
        }
    }
}
```

---

## 五、GPU 剔除（借鉴 Voxy HierarchicalOcclusionTraverser）

### 5.1 HiZ Buffer 实现

```rust
/// HiZ Buffer（借鉴 Voxy HiZBuffer）
pub struct HiZBuffer {
    /// 深度纹理 mip 链
    depth_mips: Vec<Handle<Image>>,
    /// 当前 mip 级别
    current_mip: u32,
}

impl HiZBuffer {
    /// 从主深度缓冲生成 HiZ
    pub fn generate(
        &mut self,
        depth_texture: &Handle<Image>,
        render_pass: &mut RenderPass,
    ) {
        // 逐 mip 级别降采样
        for mip in 0..self.depth_mips.len() {
            render_pass.set_pipeline(self.downsample_pipeline.clone());
            render_pass.set_bind_group(0, &self.create_bind_group(depth_texture, mip));
            render_pass.dispatch_workgroups(
                (self.width >> mip).div_ceil(64),
                self.height >> mip,
                1,
            );
        }
    }
    
    /// 查询 HiZ 深度
    pub fn sample(&self, screen_pos: Vec2, mip_level: u32) -> f32 {
        // 从对应 mip 级别采样
        texture_sample(&self.depth_mips[mip_level as usize], screen_pos)
    }
}
```

### 5.2 GPU 遍历 Compute Shader（借鉴 Voxy traversal_dev.comp）

```wgsl
// traversal.wgsl - 借鉴 Voxy 的 GPU 遍历逻辑

struct SceneUniform {
    mvp: mat4x4<f32>,
    cam_sec_pos: vec3<i32>,
    packed_hiz_size: u32,
    cam_sub_sec_pos: vec3<f32>,
    min_sss: f32,
    frustum: Frustum,
    render_queue_max_size: u32,
    frame_id: u32,
    request_queue_size: u32,
    render_distance: f32,
};

struct Frustum {
    planes: array<vec4<f32>, 6>,
};

struct Node {
    raw_pos: u64,
    data: u64,
};

struct UnpackedNode {
    node_id: u32,
    pos: vec3<i32>,
    lod_level: u32,
    mesh_ptr: u32,
    child_ptr: u32,
    flags: u32,
};

@group(0) @binding(0) var<uniform> scene: SceneUniform;
@group(0) @binding(1) var<storage, read_write> node_data: array<Node>;
@group(0) @binding(2) var<storage, read_write> render_queue: array<u32>;
@group(0) @binding(3) var<storage, read_write> request_queue: array<u32>;
@group(0) @binding(4) var hiz_texture: texture_2d<f32>;
@group(0) @binding(5) var hiz_sampler: sampler;

// 解包节点（借鉴 Voxy unpackNode）
fn unpack_node(node_id: u32) -> UnpackedNode {
    let raw = node_data[node_id];
    var node: UnpackedNode;
    node.node_id = node_id;
    node.lod_level = u32((raw.raw_pos >> 60) & 0xF);
    node.pos = vec3<i32>(
        i32((raw.raw_pos >> 40) & 0xFFFFF),
        i32((raw.raw_pos >> 20) & 0xFFFFF),
        i32(raw.raw_pos & 0xFFFFF)
    );
    node.mesh_ptr = u32(raw.data & 0xFFFFFF);
    node.child_ptr = u32((raw.data >> 24) & 0xFFFFFF);
    node.flags = u32((raw.data >> 48) & 0xFFFF);
    return node;
}

// 视锥体测试（借鉴 Voxy frustum test）
fn is_outside_frustum(node: UnpackedNode) -> bool {
    let scale = f32(1u << (node.lod_level + 5u));
    let center = vec3<f32>(node.pos) * scale - vec3<f32>(scene.cam_sec_pos) * 32.0;
    
    for (var i = 0u; i < 6u; i++) {
        let plane = scene.frustum.planes[i];
        let dist = dot(plane.xyz, center) + plane.w;
        if dist < -scale * 1.5 {
            return true;
        }
    }
    return false;
}

// HiZ 遮挡测试（借鉴 Voxy isCulledByHiz）
fn is_culled_by_hiz(node: UnpackedNode) -> bool {
    let scale = f32(1u << (node.lod_level + 5u));
    let center = vec3<f32>(node.pos) * scale - vec3<f32>(scene.cam_sec_pos) * 32.0;
    
    // 投影到屏幕空间
    let clip_pos = scene.mvp * vec4<f32>(center, 1.0);
    let ndc_pos = clip_pos.xyz / clip_pos.w;
    let screen_pos = ndc_pos.xy * 0.5 + 0.5;
    
    // 计算合适的 mip 级别
    let mip_level = u32(log2(scale));
    
    // 采样 HiZ 深度
    let hiz_depth = textureSampleLevel(hiz_texture, hiz_sampler, screen_pos, f32(mip_level)).r;
    
    // 如果节点最近点深度大于 HiZ 深度，被遮挡
    let nearest_depth = clip_pos.z - scale;
    return nearest_depth > hiz_depth;
}

// 屏幕空间误差判断（借鉴 Voxy shouldDecend）
fn should_descend(node: UnpackedNode) -> bool {
    let scale = f32(1u << (node.lod_level + 5u));
    let center = vec3<f32>(node.pos) * scale - vec3<f32>(scene.cam_sec_pos) * 32.0;
    let distance = length(center);
    
    // 计算屏幕像素大小
    let pixel_size = (scale / distance) * scene.min_sss;
    return pixel_size > 1.0;
}

// 遍历主函数（借鉴 Voxy traverse）
fn traverse(node_id: u32) {
    let node = unpack_node(node_id);
    
    // 检查渲染距离
    if !is_within_render_distance(node) {
        return;
    }
    
    // 视锥体剔除
    if is_outside_frustum(node) {
        return;
    }
    
    // HiZ 遮挡剔除
    if is_culled_by_hiz(node) {
        return;
    }
    
    // 屏幕空间误差判断
    if node.lod_level > 0u && should_descend(node) {
        // 下降到子节点
        if node.child_ptr != 0xFFFFFF {
            let child_count = u32((node.flags >> 2) & 0x7) + 1;
            for (var i = 0u; i < child_count; i++) {
                // 将子节点加入队列（需要原子操作）
                let idx = atomicAdd(&render_queue_idx, 1);
                if idx < scene.render_queue_max_size {
                    render_queue[idx] = node.child_ptr + i;
                }
            }
        }
    } else if node.mesh_ptr != 0xFFFFFF {
        // 添加到渲染队列
        let idx = atomicAdd(&render_queue_idx, 1);
        if idx < scene.render_queue_max_size {
            render_queue[idx] = node.mesh_ptr;
        }
    }
}

@compute @workgroup_size(64)
fn cs_main(@builtin(global_invocation_id) gid: vec3u) {
    let node_id = gid.x;
    if node_id < arrayLength(&node_data) {
        traverse(node_id);
    }
}
```

---

## 六、与现有系统集成

### 6.1 系统调度顺序

```rust
// main.rs 中修改系统调度
app.add_systems(Update, (
    // 阶段 1：LOD 更新（现有）
    manage_chunk_load_state,
    spawn_entities_from_prepare,
    
    // 阶段 2：八叉树遍历（新增）
    update_octree,                    // 更新八叉树结构
    traverse_octree,                  // CPU 端遍历，生成渲染队列
    
    // 阶段 3：GPU 剔除（新增）
    generate_hiz_buffer,              // 生成 HiZ Buffer
    gpu_cull_nodes,                   // GPU 端剔除
    
    // 阶段 4：Mesh 生成（现有，但不合并）
    rebuild_dirty_chunks,             // 每个 Chunk 独立生成 Mesh
    
    // 阶段 5：批量渲染（新增）
    batch_render_sections,            // 批量渲染
).chain());
```

### 6.2 关键修改点

| 文件 | 修改内容 |
|------|----------|
| `lod.rs` | 添加 OctreeNode 结构 |
| `chunk_manager.rs` | 移除环形合并逻辑，保留独立 Chunk |
| `async_mesh.rs` | 添加紧凑 Mesh 格式支持 |
| `main.rs` | 添加 GPU 剔除系统 |
| 新增 `lod_octree/` | 八叉树遍历模块 |
| 新增 `gpu_cull/` | GPU 剔除模块 |

---

## 七、性能预估

### 7.1 DrawCall 优化

| 优化方式 | 当前 | 优化后 | 减少 |
|----------|------|--------|------|
| 剔除方式 | CPU 视锥体 | GPU HiZ + 视锥体 | - |
| 渲染方式 | 逐 Chunk Draw | 批量渲染 | - |
| Mesh 格式 | 标准顶点 | 紧凑格式 (8字节) | 50% 带宽 |

### 7.2 内存优化

| 优化方式 | 当前 | 优化后 |
|----------|------|--------|
| 节点存储 | HashMap | 紧凑数组 (16字节/节点) |
| Mesh 数据 | 标准格式 | 紧凑格式 |
| 剔除状态 | 每帧重新计算 | 增量更新 |

---

## 八、实现步骤

### Phase 1：八叉树基础（2-3 天）

1. 实现 `OctreeNode` 和 `NodeStore`
2. 实现八叉树构建和更新
3. 实现 CPU 端遍历逻辑

### Phase 2：GPU 剔除（3-4 天）

4. 实现 HiZ Buffer 生成
5. 编写遍历 Compute Shader
6. 集成到 Bevy 渲染管线

### Phase 3：紧凑 Mesh（2-3 天）

7. 实现 `CompactVertex` 格式
8. 修改 `SectionMeshFactory` 生成紧凑格式
9. 实现批量渲染

### Phase 4：集成与优化（2-3 天）

10. 集成到现有系统
11. 性能测试和调优
12. Bug 修复和文档

---

## 九、风险与对策

| 风险 | 影响 | 对策 |
|------|------|------|
| Bevy 不支持 Indirect Draw | 无法使用 MDIC | 使用 Bevy 的批量渲染 API |
| GPU Compute Shader 兼容性 | 部分设备不支持 | 提供 CPU 回退路径 |
| 紧凑格式精度不足 | 视觉瑕疵 | 可选标准格式 |
| 与现有 SVO 系统冲突 | 数据不一致 | 逐步迁移，保持兼容 |

---

## 十、参考文献

1. **Voxy 源码**: `项目参考/优化参考/voxy-dev/`
2. **Voxy NodeStore**: `client/core/rendering/hierachical/NodeStore.java`
3. **Voxy 遍历 Shader**: `resources/assets/voxy/shaders/lod/hierarchical/traversal_dev.comp`
4. **Voxy Mesh 生成**: `client/core/rendering/building/RenderDataFactory.java`

---

*方案制定时间：2026-06-02*
*适用项目：Spirit Realm 体素游戏引擎*
*严格借鉴：Minecraft Voxy 模组*
