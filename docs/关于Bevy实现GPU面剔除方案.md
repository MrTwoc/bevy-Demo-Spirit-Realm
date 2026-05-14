# 在 Bevy 0.18.1 中利用 GPU 进行体素渲染面剔除的详细实现方案

## 1. Bevy 0.18.1 渲染架构概览

在深入体素面剔除的实现之前，先理解 Bevy 0.18.1 的渲染架构至关重要。Bevy 采用**双世界架构**（Dual-World Architecture），即主应用世界（Main World）和渲染世界（Render World）分离运行。渲染过程遵循严格的**四阶段执行模型**：`Extract` → `Prepare` → `Queue` → `Render`。Extract 阶段从主世界同步必要数据，Prepare 阶段将数据写入 GPU 资源，Queue 阶段组织渲染阶段创建 bind groups 并设置绘制调用，Render 阶段通过 RenderGraph 执行 GPU 命令提交和呈现。

Bevy 内置了 GPU frustum culling 支持。在 0.18 版本中，可以通过为相机添加 `GpuCulling` 组件来启用 GPU 视锥体剔除，该实现基于 `#12773` 的基础设施，通过 `NoCpuCulling` 组件可以禁用 CPU 端的 frustum culling。启用 GPU culling 后，视图进入间接绘制模式，所有绘制调用变为间接调用，由网格预处理着色器动态分配实例槽位。

## 2. 体素渲染面剔除的核心概念

### 2.1 问题定义

体素渲染中，面剔除的核心目标是在渲染之前判断并剔除（即跳过渲染）体素中不可见的三角形面。主要包括以下剔除类型：

| 剔除类型 | 说明 | GPU 实现策略 |
|---------|------|------------|
| **背面剔除 (Back-face Culling)** | 剔除法线背向相机的三角形面 | 利用光栅化器内置功能，或在 Compute Shader 中主动判断 |
| **内部面剔除 (Occlusion/Face Culling)** | 剔除相邻实心体素之间的隐藏面 | 这是体素引擎最核心的优化，需要在 Compute Shader 中判断 |
| **视锥体剔除 (Frustum Culling)** | 剔除完全在相机视锥体之外的体素 | 利用 `ViewUniform.frustum` 逐体素或逐 chunk 判断 |
| **遮挡剔除 (Occlusion Culling)** | 剔除被前方完全不透明物体完全挡住的体素 | 使用 Hi-Z 或两阶段深度预处理 |

### 2.2 为什么要在 GPU 上做面剔除

传统的 CPU 端面剔除需要为每个体素检查六个面的邻居情况，在大规模场景下（例如 256³ 的 chunk），这会产生数千万次判断，严重影响 CPU 帧率。将剔除逻辑移至 GPU Compute Shader 具有以下优势：

- **大规模并行处理**：GPU 拥有数千个核心，可同时处理数百万个体素
- **减少 CPU-GPU 数据传输**：只需上传体素数据，无需传输完整网格数据
- **与渲染管线更紧密集成**：可直接将剔除结果写入间接绘制缓冲区（Indirect Draw Buffer）

## 3. 实现方案总体架构

### 3.1 数据流概览

整个 GPU 面剔除管线按照 Bevy 的 Extract → Prepare → Queue → Render 四个阶段组织：

- **主世界 → Extract**：以 chunk 为单位从主世界提取体素数据（含 `Aabb`、`GlobalTransform` 和体素类型数组），并利用 `ViewUniform.frustum` 字段将相机视锥体提取到渲染世界。
- **渲染世界 → Prepare**：为每个 chunk 准备 GPU buffer，包括体素数据 Storage Buffer、ChunkUniform Uniform Buffer、间接绘制参数 Indirect Buffer 以及实例数据 Instance Output Buffer。同时创建 Compute Pipeline 和 Render Pipeline 的管线缓存。
- **渲染世界 → Queue**：将自定义体素渲染阶段项入队到 `BinnedRenderPhase` 中。
- **渲染世界 → Render**：分两步完成：先执行 Compute 面剔除 dispatch，然后执行间接渲染绘制。所有步骤需要确保正确的管线屏障（barrier）。

## 4. 体素数据存储与准备

### 4.1 体素数据结构

在 CPU 端（主世界），每个 chunk 包含一个压缩的体素数据数组，用 `u8` 或 `u16` 表示体素类型：

```rust
// 主世界组件
#[derive(Component)]
pub struct VoxelChunk {
    pub chunk_size: u32,          // 如 32
    pub voxels: Vec<u8>,          // 体素类型数组 (0 = 空气)
}

#[derive(Component, Clone)]
pub struct ChunkAabb {
    pub aabb: Aabb,               // Chunk 的轴对齐包围盒
}

#[derive(Bundle)]
pub struct VoxelChunkBundle {
    pub chunk: VoxelChunk,
    pub aabb: ChunkAabb,
    pub spatial: SpatialBundle,   // Transform + Visibility
}
```

在 GPU 端（WGSL），对应的数据结构如下：

```wgsl
// 单个体素的 GPGPU 表示
struct VoxelData {
    block_type: u32,     // 体素类型 ID
    flags: u32,          // 标志位（光照、可见性等）
}

// Chunk 级别的 uniform 数据
struct ChunkUniform {
    world_from_local: mat4x4<f32>,  // 从 chunk 局部空间到世界空间的变换
    chunk_size: vec3<u32>,          // chunk 尺寸 (如 32, 32, 32)
    voxel_data_offset: u32,         // 在全局 voxel buffer 中的偏移
}

// GPU 上单个可见面的输出
struct VisibleFace {
    instance_id: u32,     // 实例 ID
    face_index: u32,      // 面索引 (0-5: +X, -X, +Y, -Y, +Z, -Z)
    voxel_index: u32,     // 该体素在 chunk 内的线性索引
    block_type: u32,      // 体素类型
}

// 间接绘制参数
struct IndirectDrawArgs {
    vertex_count: u32,
    instance_count: u32,   // 由 compute shader 填充
    first_vertex: u32,
    first_instance: u32,
}
```

### 4.2 提取阶段

在 `ExtractSchedule` 中，将主世界的体素数据复制到渲染世界：

```rust
// 系统在 ExtractSchedule 中运行
pub fn extract_voxel_chunks(
    mut commands: Commands,
    chunks: Extract<Query<(Entity, &VoxelChunk, &ChunkAabb, &GlobalTransform, &ViewVisibility)>>,
) {
    for (entity, chunk, aabb, transform, visibility) in chunks.iter() {
        if !visibility.get() {
            continue;
        }
        commands.get_or_spawn(entity).insert((
            ExtractedVoxelChunk {
                chunk_size: chunk.chunk_size,
                voxel_count: chunk.voxels.len() as u32,
            },
            ExtractedChunkAabb(aabb.aabb),
            *transform,
        ));
    }
}
```

### 4.3 准备阶段 —— 创建 GPU 缓冲区

在 `RenderSet::Prepare` 阶段，为提取的数据创建 GPU buffer：

```rust
#[derive(Resource)]
pub struct VoxelFaceCullingBuffers {
    // 体素数据存储缓冲区
    pub voxel_data_buffer: StorageBuffer<Vec<u32>>,
    // 可见面输出缓冲区
    pub visible_faces_buffer: StorageBuffer<Vec<VisibleFaceGpu>>,
    // 间接绘制参数缓冲区
    pub indirect_draw_buffer: IndirectParametersBuffer,
    // Chunk uniform 缓冲区
    pub chunk_uniform_buffer: UniformBuffer<ChunkUniformGpu>,
    // 面实例数据（每个面 4 个顶点）
    pub face_instance_buffer: StorageBuffer<Vec<FaceInstanceData>>,
}

pub fn prepare_voxel_culling_buffers(
    render_device: Res<RenderDevice>,
    render_queue: Res<RenderQueue>,
    // ...
) {
    // 初始化各 GPU buffer 的逻辑...
}
```

## 5. Compute Shader 面剔除实现

这是整个方案的核心——在 GPU 上判断每个体素的每个面是否需要渲染。

### 5.1 面剔除判定逻辑

对于体素场景，一个体素面（如 +X 面）需要被渲染的充分必要条件是：

1. **该体素本身是实心体素**（`block_type != 0`）。
2. **该体素在该方向的相邻体素是空气**（或在 chunk 边界外且相邻 chunk 的对应体素为空气）。
3. **（可选）该面不在相机后方**（背面剔除）。

在 Compute Shader 中，每个工作项处理一个体素的一个面。对于 32³ 的 chunk，一共 32768 个体素，每个体素 6 个面，总共 196608 个工作项。

### 5.2 完整 Compute Shader 代码

```wgsl
// voxel_face_culling.wgsl
// Compute shader 对每个体素的每个面进行可见性判断

struct VoxelData {
    block_type: u32,
}

struct ChunkUniform {
    world_from_local: mat4x4<f32>,
    chunk_size: vec3<u32>,
}

struct VisibleFace {
    face_index: u32,
    voxel_index: u32,
    block_type: u32,
}

// 面方向向量
const FACE_DIRS: array<vec3<i32>, 6> = array<vec3<i32>, 6>(
    vec3<i32>( 1,  0,  0),  // +X
    vec3<i32>(-1,  0,  0),  // -X
    vec3<i32>( 0,  1,  0),  // +Y
    vec3<i32>( 0, -1,  0),  // -Y
    vec3<i32>( 0,  0,  1),  // +Z
    vec3<i32>( 0,  0, -1),  // -Z
);

// 面的法线方向（世界空间）
const FACE_NORMALS: array<vec3<f32>, 6> = array<vec3<f32>, 6>(
    vec3<f32>( 1.0,  0.0,  0.0),
    vec3<f32>(-1.0,  0.0,  0.0),
    vec3<f32>( 0.0,  1.0,  0.0),
    vec3<f32>( 0.0, -1.0,  0.0),
    vec3<f32>( 0.0,  0.0,  1.0),
    vec3<f32>( 0.0,  0.0, -1.0),
);

@group(0) @binding(0)
var<storage, read> voxel_data: array<VoxelData>;

@group(0) @binding(1)
var<uniform> chunk: ChunkUniform;

@group(0) @binding(2)
var<storage, read_write> visible_faces: array<VisibleFace>;

@group(0) @binding(3)
var<uniform> view: ViewUniform;  // Bevy 内置，包含 frustum 和相机位置

@group(0) @binding(4)
var<storage, read_write> indirect_args: IndirectDrawArgs;

@group(0) @binding(5)
var<storage, read_write> face_count: atomic<u32>;

// 将 3D 体素坐标转换为线性索引
fn voxel_index_3d_to_1d(pos: vec3<u32>, size: vec3<u32>) -> u32 {
    return pos.x + pos.y * size.x + pos.z * size.x * size.y;
}

// 检查邻居体素是否为空气
fn is_neighbor_air(
    pos: vec3<i32>,
    face_dir: vec3<i32>,
    chunk_size: vec3<u32>,
) -> bool {
    let neighbor = pos + face_dir;

    // 边界检查：chunk 边界外的体素视为空气
    if neighbor.x < 0 || neighbor.x >= i32(chunk_size.x) ||
       neighbor.y < 0 || neighbor.y >= i32(chunk_size.y) ||
       neighbor.z < 0 || neighbor.z >= i32(chunk_size.z) {
        return true;
    }

    let idx = voxel_index_3d_to_1d(
        vec3<u32>(u32(neighbor.x), u32(neighbor.y), u32(neighbor.z)),
        chunk_size,
    );
    return voxel_data[idx].block_type == 0u;
}

// 背面剔除：检查面法线是否朝向相机
fn is_back_face(
    face_normal_world: vec3<f32>,
    voxel_center_world: vec3<f32>,
) -> bool {
    // 从体素中心到相机的方向
    let to_camera = view.world_position - voxel_center_world;
    // 如果面法线与视线方向点积 <= 0，则为背面
    return dot(face_normal_world, to_camera) <= 0.0;
}

// AABB-Frustum 相交测试（简化版，利用 Bevy 的 ViewUniform.frustum 数据）
fn intersects_frustum(chunk_aabb_min: vec3<f32>, chunk_aabb_max: vec3<f32>) -> bool {
    // Bevy 的 frustum 半空间：法线指向 frustum 内部
    for (var i: u32 = 0u; i < 6u; i++) {
        let half_space = view.frustum[i];

        // 根据半空间法线选择 AABB 的顶点
        var p: vec3<f32>;
        if half_space.x > 0.0 {
            p.x = chunk_aabb_max.x;
        } else {
            p.x = chunk_aabb_min.x;
        }
        if half_space.y > 0.0 {
            p.y = chunk_aabb_max.y;
        } else {
            p.y = chunk_aabb_min.y;
        }
        if half_space.z > 0.0 {
            p.z = chunk_aabb_max.z;
        } else {
            p.z = chunk_aabb_min.z;
        }

        // 如果该顶点在半空间外部，则 AABB 在 frustum 外部
        let d = half_space.x * p.x + half_space.y * p.y + half_space.z * p.z + half_space.w;
        if d < 0.0 {
            return false;
        }
    }
    return true;
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let total_voxels = chunk.chunk_size.x * chunk.chunk_size.y * chunk.chunk_size.z;
    let task_id = global_id.x;

    // 每个工作项处理一个体素的一个面
    let face_idx = task_id % 6u;
    let voxel_idx = task_id / 6u;

    if voxel_idx >= total_voxels {
        return;
    }

    let voxel = voxel_data[voxel_idx];
    if voxel.block_type == 0u {
        return; // 空气体素，跳过
    }

    // 计算体素的 3D 坐标
    let sx = chunk.chunk_size.x;
    let sy = chunk.chunk_size.y;
    let vz = voxel_idx / (sx * sy);
    let remainder = voxel_idx % (sx * sy);
    let vy = remainder / sx;
    let vx = remainder % sx;

    let voxel_pos = vec3<i32>(i32(vx), i32(vy), i32(vz));

    // 检查邻居体素是否为空气
    if !is_neighbor_air(voxel_pos, FACE_DIRS[face_idx], chunk.chunk_size) {
        return; // 该面被遮挡，跳过
    }

    // 背面剔除
    let voxel_center_local = vec3<f32>(
        f32(vx) + 0.5,
        f32(vy) + 0.5,
        f32(vz) + 0.5,
    );
    let voxel_center_world = (chunk.world_from_local * vec4<f32>(voxel_center_local, 1.0)).xyz;
    let face_normal_world = (chunk.world_from_local * vec4<f32>(FACE_NORMALS[face_idx], 0.0)).xyz;
    if is_back_face(face_normal_world, voxel_center_world) {
        return;
    }

    // 视锥体剔除（可选，提升性能）
    // 对单个面的 AABB 或体素做 frustum 检测
    // （此处可加入 intersects_frustum 检查）

    // 该面可见：原子递增计数并写入可见面列表
    let output_idx = atomicAdd(&face_count, 1u);
    visible_faces[output_idx] = VisibleFace(
        face_idx,
        voxel_idx,
        voxel.block_type,
    );
}
```

### 5.3 两个关键数学判断的说明

**邻居空气判断**：对于每个体素的 6 个面，检查该方向的相邻体素是否为空气（`block_type == 0`）。如果邻居是实心体素，则当前面被完全遮挡，无需渲染。对于 chunk 边界，如果相邻体素位于当前 chunk 之外，则视其可能为空气（需要跨 chunk 查询或简化处理）。

**背面剔除**：通过计算面法线与视线方向（相机位置 → 体素中心）的点积，若 `dot(normal, to_camera) <= 0`，说明该面背对相机，可以安全剔除。

## 6. 渲染管线集成

### 6.1 间接绘制

剔除完成后，面计数通过原子操作写入 `indirect_args` 缓冲区，该缓冲区随后被用于间接绘制调用。间接绘制允许 GPU 在运行时确定实例数量，避免了 CPU-GPU 回读延迟：

```rust
// 在渲染命令中设置间接绘制
render_pass.multi_draw_indirect(
    &indirect_draw_buffer,
    0,  // offset
    1,  // draw count
);
```

### 6.2 自定义渲染阶段

通过自定义 `RenderCommand` 将体素渲染入队到 Bevy 的渲染阶段。参考 Bevy 的 `custom_phase_item` 示例，需要创建一个自定义的 Phase Item，实现 `CachedRenderPipelinePhaseItem`，然后构建 `DrawFunction`。Bevy 使用 `BinnedRenderPhase` 来管理批量渲染，渲染阶段分为 `Opaque3d`（不透明物体）和 `Transparent3d`（透明物体）等。

```rust
// 自定义体素 Phase Item
#[derive(PhaseItem)]
pub struct VoxelPhaseItem {
    pub entity: Entity,
    pub pipeline: CachedRenderPipelineId,
    pub draw_function: DrawFunctionId,
    pub batch_range: Range<u32>,
    pub extra_index: PhaseItemExtraIndex,
}

// 实现 CachedRenderPipelinePhaseItem
impl CachedRenderPipelinePhaseItem for VoxelPhaseItem {
    // ...
}

// 定义绘制命令集
pub type DrawVoxelCommands = (
    SetItemPipeline,
    SetVoxelViewBindGroup<0>,
    SetVoxelChunkBindGroup<1>,
    DrawIndirect,
);

// 在 Queue 阶段将体素入队
pub fn queue_voxel_chunks(
    // ...
    mut views: Query<&mut RenderPhase<VoxelPhaseItem>>,
) {
    // ...
}
```

### 6.3 与 Bevy 内置 GPU Culling 的集成

Bevy 0.18 的 `GpuCulling` 组件是为通用网格渲染设计的，主要处理 frustum culling 和 occlusion culling。在体素场景下，可以利用 Bevy 的内置 GPU frustum culling 来判断每个 chunk 是否在视锥体内，作为第一层粗粒度剔除：

```rust
// 为体素相机添加 GpuCulling
commands.spawn((
    Camera3d::default(),
    GpuCulling,        // 启用 GPU frustum culling
    NoCpuCulling,      // 禁用 CPU frustum culling
    // ...
));
```

需要注意的是，Bevy 的 `MeshCullingData` 结构体包含 `aabb_center` 和 `aabb_half_extents` 字段，用于 GPU 端基于 AABB 的剔除。在体素场景下，体素的 AABB 数据同样可以作为 chunk 级别的 frustum culling 输入。

### 6.4 Compute Pass 与 Render Pass 的组织

在 `RenderGraph` 中添加自定义节点来组织 Compute Pass（面剔除）和 Render Pass（体素渲染）：

```rust
// 自定义 Render Graph 节点
pub struct VoxelCullingNode {
    // 用于 Compute Pass 的 view 数据查询
    pub view_query: QueryState<(
        &'static ExtractedView,
        &'static VoxelGpuCullingData,
    )>,
}

impl render_graph::Node for VoxelCullingNode {
    fn run(
        &self,
        _graph: &mut RenderGraphContext,
        render_context: &mut RenderContext,
        world: &World,
    ) -> Result<(), NodeRunError> {
        // 1. 执行 Compute Pass：面剔除
        // 2. 执行 Compute Pass：原子计数清零
        // 3. 执行 Compute Pass：实例数据填充
        // 4. 插入 UAV Barrier 确保 Compute 写入对后续 Render 可见
        // 5. 执行 Render Pass：间接绘制体素面
        // ...
    }
}
```

### 6.5 Compute Shader 与 Render Shader 间的管线屏障（Barrier）管理

在 WebGPU/WGPU 中，当 Compute Shader 写入缓冲区后被后续的 Render Pass 读取时，必须显式插入管线屏障（Pipeline Barrier）。这确保 Compute 写入对后续的间接绘制调用可见。

```rust
// 在 Compute Pass 结束后，Render Pass 开始前
// WGPU 会自动处理 RenderPass 和 ComputePass 之间的依赖
// 但如果使用 RenderCommandEncoder 之外的 wgpu 命令编码器，需要手动处理：

// 方法 1: 通过 wgpu 命令编码器手动插入屏障
encoder.insert_barrier(
    wgpu::ComputePassDescriptor::default(),
    wgpu::RenderPassDescriptor::default(),
);

// 方法 2: 将 Compute 和 Render 放在不同的 Graph Node 中
// Bevy 的 RenderGraph 会自动在节点之间插入必要的屏障
```

## 7. ViewUniform 和 Frustum 数据

Bevy 0.18.1 的 `ViewUniform` 提供了完整的渲染视图数据，对于 GPU 面剔除特别重要的是 `frustum` 字段，它是一个包含 6 个 `Vec4` 的数组，每个 `Vec4` 表示视锥体的一个半空间（half-space），法线指向视锥体内部。

在 Compute Shader 中，可以利用 `view.frustum` 数据实现高效的 frustum culling。

```rust
// Rust 端准备 ViewUniform bind group
pub fn prepare_voxel_view_bind_group(
    render_device: Res<RenderDevice>,
    view_uniforms: Res<ViewUniforms>,
    voxel_pipeline: Res<VoxelRenderPipeline>,
) {
    // 将 ViewUniform 绑定到 @group(0) @binding(3)
    // ViewUniform 由 Bevy 自动填充和更新
}
```

**ViewUniform 关键字段说明**：

| 字段 | WGSL 类型 | 用途 |
|------|---------|-----|
| `world_position` | `vec3<f32>` | 相机世界空间位置，用于背面剔除的视线计算 |
| `frustum` | `array<Vec4, 6>` | 视锥体 6 个半空间参数，用于 frustum culling |
| `clip_from_world` | `mat4x4<f32>` | 世界到裁剪空间的变换矩阵 |
| `view_from_world` | `mat4x4<f32>` | 世界到视图空间的变换矩阵 |

## 8. 性能优化建议

### 8.1 Workgroup Size 优化

Compute Shader 的 workgroup size 对性能有显著影响。通常每个 workgroup 128 或 256 个工作项效果最佳，而非默认值 64。需要根据目标 GPU 的 `maxComputeWorkgroupInvocations` 限制进行调整：

```rust
// 查询 GPU 限制并动态选择 workgroup size
let limits = render_device.limits();
let max_invocations = limits.max_compute_invocations_per_workgroup;
let workgroup_size = max_invocations.min(256);
```

### 8.2 间接绘制与 GPU-Driven 渲染

使用间接绘制（Indirect Draw）可以完全避免 CPU 读取 GPU 缓冲区，将实例计数直接留在 GPU 上：

- **Buffer 上传策略**：体素数据只在 chunk 修改时重新上传
- **原子计数器**：使用 `atomic<u32>` 安全地递增可见面计数
- **多级剔除**：先做 chunk 级别的 frustum culling，再对可见 chunk 内的体素做面剔除
- **Bevy 的内置 GPU 剔除**：利用 Bevy 的 `GpuCulling` 和 `GpuInstanceBufferBuilder` 机制来优化数据传输路径

### 8.3 间接绘制批处理与 Bin 优化

Bevy 使用 `BinnedRenderPhase` 来管理批量渲染，通过 `BinKey` 将相同材质和 mesh 的渲染项分组，减少状态切换。体素面的顶点数据通常预定义为一个 6 面立方体的 24 个顶点（每个面 4 个），可以使用实例化绘制，每个实例的变换矩阵和纹理信息存储在实例缓冲区中。这种方式配合间接绘制可以大幅减少 CPU 绘制调用开销。

## 9. 总结

本方案利用 Bevy 0.18.1 的渲染架构，通过在 Compute Shader 中执行体素面可见性判断，结合间接绘制机制实现 GPU-Driven 的体素渲染。核心步骤包括：

1. **Extract**：从主世界提取体素 chunk 数据和相机视锥体
2. **Prepare**：准备 GPU buffer（体素数据、ChunkUniform、可见面输出、间接绘制参数）
3. **Queue**：将自定义体素渲染阶段项入队
4. **Render**：先执行 Compute 面剔除 dispatch，再执行间接渲染绘制

面剔除的判断逻辑在 Compute Shader 中执行，包括：邻居体素是否为空气、背面剔除、视锥体剔除。Bevy 的内置 `ViewUniform.frustum` 为 frustum culling 提供标准化的半空间数据，可直接在自定义 shader 中使用。Bevy 的 `GpuCulling` 组件可用于 chunk 级别的粗粒度 frustum culling，结合本方案的自定义 Compute Shader 面剔除和间接绘制，可构建高性能的大规模体素渲染系统。

对于社区参考实现，建议关注 Bevy 的 `custom_phase_item` 示例（自定义渲染阶段入队）、`specialized_mesh_pipeline` 示例（自定义网格管线）以及 `bevy_pbr` 源码中的 GPU preprocessing 和 indirect draw 实现。