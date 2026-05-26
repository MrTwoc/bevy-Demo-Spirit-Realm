# Bevy 官方 Demo 借鉴指南

本文档整理 Bevy 官方 Demo 中与体素渲染优化相关的可借鉴内容，按功能分类。

---

## 📁 Demo 文件位置

```
项目参考/Bevy官方Demo/
├── shader/                          # 基础示例（17个）
│   ├── compute_shader_game_of_life.rs   ← 计算着色器核心参考
│   ├── storage_buffer.rs                 ← Storage Buffer 使用模式
│   ├── gpu_readback.rs                   ← GPU 数据回读机制
│   ├── automatic_instancing.rs            ← 自动实例化
│   ├── array_texture.rs                   ← 纹理数组
│   ├── shader_prepass.rs                  ← Prepass
│   ├── extended_material.rs              ← 材质扩展
│   ├── shader_defs.rs                    ← Shader Defs
│   ├── shader_material_bindless.rs        ← Bindless 材质
│   ├── animate_shader.rs                  ← 动画着色器
│   └── ... (共17个)
│
├── shader_advanced/                  # 高级示例（10个）
│   ├── custom_shader_instancing.rs       ← 自定义实例化渲染
│   ├── specialized_mesh_pipeline.rs     ← 自定义渲染管线
│   ├── texture_binding_array.rs          ← 纹理数组绑定
│   ├── custom_vertex_attribute.rs        ← 自定义顶点属性
│   ├── custom_post_processing.rs         ← 后处理效果
│   ├── render_depth_to_texture.rs        ← 深度纹理复制
│   ├── fullscreen_material.rs            ← 全屏着色器
│   └── ... (共10个)
│
└── shaders/                         # WGSL 着色器源码
    ├── game_of_life.wgsl
    ├── storage_buffer.wgsl
    └── ...
```

---

## 🎯 1. 计算着色器（GPU 视锥体剔除）

### 核心参考 Demo
| 文件 | 说明 |
|------|------|
| `shader/compute_shader_game_of_life.rs` | **主要参考**：完整的计算着色器集成方案 |
| `shaders/game_of_life.wgsl` | WGSL 计算着色器源码 |

### 关键借鉴点

#### 1.1 资源定义与提取
```rust
// 主世界资源定义
#[derive(Resource, Clone, ExtractResource)]
struct GameOfLifeImages {
    texture_a: Handle<Image>,
    texture_b: Handle<Image>,
}

// Uniform 定义（与 GPU ShaderType 对齐）
#[derive(Resource, Clone, ExtractResource, ShaderType)]
struct GameOfLifeUniforms {
    alive_color: LinearRgba,
}
```

#### 1.2 Bind Group 布局配置
```rust
let texture_bind_group_layout = BindGroupLayoutDescriptor::new(
    "GameOfLifeImages",
    &BindGroupLayoutEntries::sequential(
        ShaderStages::COMPUTE,
        (
            texture_storage_2d(TextureFormat::Rgba32Float, StorageTextureAccess::ReadOnly),
            texture_storage_2d(TextureFormat::Rgba32Float, StorageTextureAccess::WriteOnly),
            uniform_buffer::<GameOfLifeUniforms>(false),
        ),
    ),
);
```

#### 1.3 多入口点 Compute Pipeline
```rust
// 初始化 Pipeline（入口点：init）
let init_pipeline = pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
    layout: vec![texture_bind_group_layout.clone()],
    shader: shader.clone(),
    entry_point: Some(Cow::from("init")),
    ..default()
});

// 更新 Pipeline（入口点：update）
let update_pipeline = pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
    layout: vec![texture_bind_group_layout.clone()],
    shader,
    entry_point: Some(Cow::from("update")),
    ..default()
});
```

#### 1.4 Render Graph 集成
```rust
// 添加到 Render Graph
let mut render_graph = render_app.world_mut().resource_mut::<RenderGraph>();
render_graph.add_node(GameOfLifeLabel, GameOfLifeNode::default());
render_graph.add_node_edge(GameOfLifeLabel, CameraDriverLabel);

// 实现 Node trait
impl render_graph::Node for GameOfLifeNode {
    fn run(&self, graph: &mut RenderGraphContext, render_context: &mut RenderContext, world: &World) -> Result<(), NodeRunError> {
        let mut pass = render_context.command_encoder().begin_compute_pass(&ComputePassDescriptor::default());
        pass.set_pipeline(update_pipeline);
        pass.set_bind_group(0, &bind_groups[index], &[]);
        pass.dispatch_workgroups(SIZE.x / WORKGROUP_SIZE, SIZE.y / WORKGROUP_SIZE, 1);
        Ok(())
    }
}
```

#### 1.5 双缓冲（Ping-Pong）模式
```rust
// 两个 BindGroup 交替使用
let bind_group_0 = render_device.create_bind_group(
    None,
    &pipeline_cache.get_bind_group_layout(&pipeline.texture_bind_group_layout),
    &BindGroupEntries::sequential((
        &view_a.texture_view,   // 读取 A
        &view_b.texture_view,   // 写入 B
        &uniform_buffer,
    )),
);

// 状态切换
enum GameOfLifeState {
    Update(usize),  // 0 或 1，指示当前使用的 buffer
}
```

### WGSL 示例
```wgsl
@group(0) @binding(0) var input: texture_storage_2d<rgba32float, read>;
@group(0) @binding(1) var output: texture_storage_2d<rgba32float, write>;
@group(0) @binding(2) var<uniform> config: GameOfLifeUniforms;

@compute @workgroup_size(8, 8, 1)
fn init(@builtin(global_invocation_id) invocation_id: vec3<u32>) {
    let location = vec2<i32>(i32(invocation_id.x), i32(invocation_id.y));
    textureStore(output, location, color);
}

@compute @workgroup_size(8, 8, 1)
fn update(@builtin(global_invocation_id) invocation_id: vec3<u32>) {
    // 生命游戏规则更新
}
```

---

## 📦 2. Storage Buffer 数据传递

### 核心参考 Demo
| 文件 | 说明 |
|------|------|
| `shader/storage_buffer.rs` | 使用 `AsBindGroup` 的 Storage Buffer 模式 |
| `shaders/storage_buffer.wgsl` | WGSL 着色器源码 |

### 关键借鉴点

#### 2.1 Rust 端：`AsBindGroup` trait
```rust
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
struct CustomMaterial {
    #[storage(0, read_only)]
    colors: Handle<ShaderStorageBuffer>,
}

impl Material for CustomMaterial {
    fn vertex_shader() -> ShaderRef {
        SHADER_ASSET_PATH.into()
    }
    fn fragment_shader() -> ShaderRef {
        SHADER_ASSET_PATH.into()
    }
}
```

#### 2.2 WGSL 端：声明 Storage Buffer
```wgsl
#import bevy_pbr::{
    mesh_functions,
    view_transformations::position_world_to_clip
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) 
var<storage, read> colors: array<vec4<f32>, 5>;

struct Vertex {
    @builtin(instance_index) instance_index: u32,
    @location(0) position: vec3<f32>,
};

@vertex
fn vertex(vertex: Vertex) -> VertexOutput {
    let tag = mesh_functions::get_tag(vertex.instance_index);
    out.color = colors[tag];
}
```

---

## 🔄 3. GPU 数据回读机制

### 核心参考 Demo
| 文件 | 说明 |
|------|------|
| `shader/gpu_readback.rs` | `Readback` 组件 + `ReadbackComplete` 事件 |
| `shaders/gpu_readback.wgsl` | WGSL 着色器源码 |

### 关键借鉴点

#### 3.1 启用 COPY_SRC 标志
```rust
let mut buffer = ShaderStorageBuffer::from(buffer);
buffer.buffer_description.usage |= BufferUsages::COPY_SRC;
```

#### 3.2 Readback 组件挂载
```rust
commands
    .spawn(Readback::buffer(buffer.clone()))
    .observe(|event: On<ReadbackComplete>| {
        let data: Vec<u32> = event.to_shader_type();
        info!("Buffer {:?}", data);
    });
```

#### 3.3 支持部分回读
```rust
commands.spawn(Readback::buffer_range(
    buffer.clone(),
    4 * u32::SHADER_SIZE.get(), // skip first 4 elements
    8 * u32::SHIFT_SIZE.get(),   // read 8 elements
));
```

---

## 🏭 4. 自动实例化渲染（减少 Draw Call）

### 核心参考 Demo
| 文件 | 说明 |
|------|------|
| `shader/automatic_instancing.rs` | `MeshTag` + 自动实例化 |
| `shaders/automatic_instancing.wgsl` | WGSL 着色器源码 |

### 关键借鉴点

#### 4.1 共享 Mesh 和 Material Handle
```rust
let mesh_handle = meshes.add(Cuboid::from_size(Vec3::splat(0.01)));
let material_handle = materials.add(CustomMaterial { ... });

for index in 0..total_pixels {
    commands.spawn((
        Mesh3d(mesh_handle.clone()),          // 相同 handle → 自动合批
        MeshMaterial3d(material_handle.clone()),
        MeshTag(index),                        // 实例索引
        Transform::from_xyz(world_x, world_y, 0.0),
    ));
}
```

#### 4.2 顶点着色器中获取实例索引
```wgsl
@group(#{MATERIAL_BIND_GROUP}) @binding(0) var texture: texture_2d<f32>;

struct Vertex {
    @builtin(instance_index) instance_index: u32,
    @location(0) position: vec3<f32>,
};

@vertex
fn vertex(vertex: Vertex) -> VertexOutput {
    let tag = mesh_functions::get_tag(vertex.instance_index);
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
    out.world_position = mesh_functions::mesh_position_local_to_world(world_from_local, vec4(vertex.position, 1.0));
    out.clip_position = position_world_to_clip(out.world_position.xyz);
    
    let texel_coord = vec2<u32>(tag % tex_dim.x, tag / tex_dim.x);
    out.color = textureLoad(texture, texel_coord, 0);
}
```

---

## ⚙️ 5. 自定义实例化渲染（高级）

### 核心参考 Demo
| 文件 | 说明 |
|------|------|
| `shader_advanced/custom_shader_instancing.rs` | 低级渲染 API 自定义实例化 |
| `shaders/instancing.wgsl` | WGSL 着色器源码 |

### 关键借鉴点

#### 5.1 Instance Buffer 准备
```rust
fn prepare_instance_buffers(
    mut commands: Commands,
    query: Query<(Entity, &InstanceMaterialData)>,
    render_device: Res<RenderDevice>,
) {
    for (entity, instance_data) in &query {
        let buffer = render_device.create_buffer_with_data(&BufferInitDescriptor {
            label: Some("instance data buffer"),
            contents: bytemuck::cast_slice(instance_data.as_slice()),
            usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
        });
        commands.entity(entity).insert(InstanceBuffer {
            buffer,
            length: instance_data.len(),
        });
    }
}
```

#### 5.2 自定义渲染命令
```rust
type DrawCustom = (
    SetItemPipeline,
    SetMeshViewBindGroup<0>,
    SetMeshViewBindingArrayBindGroup<1>,
    SetMeshBindGroup<2>,
    DrawMeshInstanced,
);

impl<P: PhaseItem> RenderCommand<P> for DrawMeshInstanced {
    type Param = (SRes<RenderAssets<RenderMesh>>, SRes<RenderMeshInstances>, SRes<MeshAllocator>);
    
    fn render<'w>(item: &P, _: (), instance_buffer: Option<&'w InstanceBuffer>, ...) -> RenderCommandResult {
        pass.draw_indexed(
            index_buffer_slice.range.start..(index_buffer_slice.range.start + count),
            vertex_buffer_slice.range.start as i32,
            0..instance_buffer.length as u32,  // 实例数量
        );
    }
}
```

---

## 🎨 6. 自定义渲染管线

### 核心参考 Demo
| 文件 | 说明 |
|------|------|
| `shader_advanced/specialized_mesh_pipeline.rs` | `SpecializedMeshPipeline` 自定义管线 |
| `shaders/specialized_mesh_pipeline.wgsl` | WGSL 着色器源码 |

### 关键借鉴点

#### 6.1 实现 SpecializedMeshPipeline trait
```rust
impl SpecializedMeshPipeline for CustomMeshPipeline {
    type Key = MeshPipelineKey;

    fn specialize(
        &self,
        key: Self::Key,
        layout: &MeshVertexBufferLayoutRef,
    ) -> Result<RenderPipelineDescriptor, SpecializedMeshPipelineError> {
        let mut descriptor = self.mesh_pipeline.specialize(key, layout)?;
        descriptor.vertex.shader = self.shader.clone();
        descriptor.vertex.buffers.push(VertexBufferLayout {
            array_stride: size_of::<InstanceData>() as u64,
            step_mode: VertexStepMode::Instance,  // 实例步进模式
            attributes: vec![...],
        });
        Ok(descriptor)
    }
}
```

---

## ⚙️ 7. 纹理数组绑定（Bindless Texture）

### 核心参考 Demo
| 文件 | 说明 |
|------|------|
| `shader_advanced/texture_binding_array.rs` | `binding_array` + GPU 特性检测 |
| `shaders/texture_binding_array.wgsl` | WGSL 着色器源码 |

### 关键借鉴点

#### 7.1 GPU 特性检测
```rust
fn verify_required_features(render_device: Res<RenderDevice>) {
    if !render_device.features().contains(
        WgpuFeatures::SAMPLED_TEXTURE_AND_STORAGE_BUFFER_ARRAY_NON_UNIFORM_INDEXING
    ) {
        error!("Render device doesn't support required feature");
        exit(1);
    }
}
```

#### 7.2 纹理数组绑定
```rust
fn bind_group_layout_entries(_: &RenderDevice, _: bool) -> Vec<BindGroupLayoutEntry> {
    BindGroupLayoutEntries::with_indices(
        ShaderStages::FRAGMENT,
        (
            // @group(0) @binding(0) var textures: binding_array<texture_2d<f32>>;
            (
                0,
                texture_2d(TextureSampleType::Float { filterable: true })
                    .count(NonZero::<u32>::new(MAX_TEXTURE_COUNT as u32).unwrap()),
            ),
            // Sampler
            (1, sampler(SamplerBindingType::Filtering)),
        ),
    )
}
```

#### 7.3 手动创建 Bind Group（用于纹理数组）
```rust
fn as_bind_group(&self, layout: &BindGroupLayoutDescriptor, ...) -> Result<PreparedBindGroup, ...> {
    let fallback_image = &fallback_image.d2;
    let textures = vec![&fallback_image.texture_view; MAX_TEXTURE_COUNT];
    
    // 填充实际纹理
    for (id, image) in images.into_iter().enumerate() {
        textures[id] = &*image.texture_view;
    }
    
    let bind_group = render_device.create_bind_group(
        Self::label(),
        &pipeline_cache.get_bind_group_layout(layout),
        &BindGroupEntries::sequential((&textures[..], &fallback_image.sampler)),
    );
}
```

---

## 🎨 8. 自定义顶点属性

### 核心参考 Demo
| 文件 | 说明 |
|------|------|
| `shader_advanced/custom_vertex_attribute.rs` | 自定义顶点属性注册 |
| `shaders/custom_vertex_attribute.wgsl` | WGSL 着色器源码 |

### 关键借鉴点

#### 8.1 定义自定义属性
```rust
// 高 ID 用于避免冲突
const ATTRIBUTE_BLEND_COLOR: MeshVertexAttribute =
    MeshVertexAttribute::new("BlendColor", 988540917, VertexFormat::Float32x4);

let mesh = Mesh::from(Cuboid::default())
    .with_inserted_attribute(
        ATTRIBUTE_BLEND_COLOR,
        vec![[1.0, 0.0, 0.0, 1.0]; 24],  // 每个顶点一个值
    );
```

#### 8.2 在 Material::specialize 中注册
```rust
fn specialize(
    _pipeline: &MaterialPipeline,
    descriptor: &mut RenderPipelineDescriptor,
    layout: &MeshVertexBufferLayoutRef,
    _key: MaterialPipelineKey<Self>,
) -> Result<(), SpecializedMeshPipelineError> {
    let vertex_layout = layout.0.get_layout(&[
        Mesh::ATTRIBUTE_POSITION.at_shader_location(0),
        ATTRIBUTE_BLEND_COLOR.at_shader_location(1),  // 自定义属性
    ])?;
    descriptor.vertex.buffers = vec![vertex_layout];
    Ok(())
}
```

---

## 🔄 9. 自定义渲染阶段项目

### 核心参考 Demo
| 文件 | 说明 |
|------|------|
| `shader_advanced/custom_phase_item.rs` | 完整自定义渲染项目实现 |

### 关键借鉴点

#### 9.1 RenderCommand 实现
```rust
struct DrawCustomPhaseItem;

impl<P> RenderCommand<P> for DrawCustomPhaseItem
where P: PhaseItem,
{
    type Param = SRes<CustomPhaseItemBuffers>;
    type ViewQuery = ();
    type ItemQuery = ();

    fn render<'w>(item: &P, ... custom_phase_item_buffers: SystemParamItem<...>, pass: &mut TrackedRenderPass<'w>) -> RenderCommandResult {
        pass.set_vertex_buffer(0, custom_phase_item_buffers.vertices.buffer().unwrap().slice(..));
        pass.set_index_buffer(...);
        pass.draw_indexed(0..3, 0, 0..1);  // 绘制
        RenderCommandResult::Success
    }
}
```

#### 9.2 RawBufferVec 使用
```rust
#[derive(Resource)]
struct CustomPhaseItemBuffers {
    vertices: RawBufferVec<Vertex>,
    indices: RawBufferVec<u32>,
}

impl FromWorld for CustomPhaseItemBuffers {
    fn from_world(world: &mut World) -> Self {
        let render_device = world.resource::<RenderDevice>();
        let render_queue = world.resource::<RenderQueue>();
        
        let mut vbo = RawBufferVec::new(BufferUsages::VERTEX);
        for vertex in &VERTICES { vbo.push(*vertex); }
        vbo.write_buffer(render_device, render_queue);  // 上传到 GPU
        
        CustomPhaseItemBuffers { vertices: vbo, indices: ibo }
    }
}
```

---

## 🌅 10. 后处理效果

### 核心参考 Demo
| 文件 | 说明 |
|------|------|
| `shader_advanced/custom_post_processing.rs` | 后处理 Pass 集成 |
| `shaders/post_processing.wgsl` | WGSL 着色器源码 |

### 关键借鉴点

#### 10.1 ViewNode trait 实现
```rust
impl ViewNode for PostProcessNode {
    type ViewQuery = (&'static ViewTarget, &'static PostProcessSettings, &'static DynamicUniformIndex<PostProcessSettings>);
    
    fn run(&self, ... (view_target, _post_process_settings, settings_index): QueryItem<Self::ViewQuery>, world: &World) -> Result<(), NodeRunError> {
        let post_process_pipeline = world.resource::<PostProcessPipeline>();
        let pipeline_cache = world.resource::<PipelineCache>();
        
        // 获取后处理纹理（源/目标自动翻转）
        let post_process = view_target.post_process_write();
        
        // 创建 Bind Group
        let bind_group = render_context.render_device().create_bind_group(...);
        
        // 开始渲染 Pass
        let mut render_pass = render_context.begin_tracked_render_pass(RenderPassDescriptor {
            color_attachments: &[Some(RenderPassColorAttachment {
                view: post_process.destination,
                ...
            })],
            ...
        });
        
        render_pass.set_render_pipeline(pipeline);
        render_pass.set_bind_group(0, &bind_group, &[settings_index.index()]);
        render_pass.draw(0..3, 0..1);
    }
}
```

#### 10.2 渲染图集成
```rust
render_app
    .add_render_graph_node::<ViewNodeRunner<PostProcessNode>>(Core3d, PostProcessLabel)
    .add_render_graph_edges(
        Core3d,
        (Node3d::Tonemapping, PostProcessLabel, Node3d::EndMainPassPostProcessing),
    );
```

---

## 📦 11. 自定义渲染阶段（分批）

### 核心参考 Demo
| 文件 | 说明 |
|------|------|
| `shader_advanced/custom_render_phase.rs` | SortedPhaseItem + 批处理 |

### 关键借鉴点

#### 11.1 实现 SortedPhaseItem
```rust
struct Stencil3d {
    pub sort_key: FloatOrd,
    pub entity: (Entity, MainEntity),
    pub pipeline: CachedRenderPipelineId,
    pub draw_function: DrawFunctionId,
    pub batch_range: Range<u32>,
    pub extra_index: PhaseItemExtraIndex,
    pub indexed: bool,
}

impl PhaseItem for Stencil3d { ... }
impl SortedPhaseItem for Stencil3d {
    type SortKey = FloatOrd;
    fn sort_key(&self) -> Self::SortKey { self.sort_key }
    fn sort(items: &mut [Self]) { items.sort_by_key(SortedPhaseItem::sort_key); }
}
```

#### 11.2 GetBatchData 实现
```rust
impl GetBatchData for StencilPipeline {
    type Param = (SRes<RenderMeshInstances>, SRes<RenderAssets<RenderMesh>>, SRes<MeshAllocator>);
    type CompareData = AssetId<Mesh>;
    type BufferData = MeshUniform;
    
    fn get_batch_data(...) -> Option<(Self::BufferData, Option<Self::CompareData>)> {
        // 返回 MeshUniform 用于 GPU 批次
    }
}
```

---

## 🖥️ 12. 全屏着色器材质

### 核心参考 Demo
| 文件 | 说明 |
|------|------|
| `shader_advanced/fullscreen_material.rs` | FullscreenMaterial trait |
| `shaders/fullscreen_effect.wgsl` | WGSL 着色器源码 |

### 关键借鉴点

```rust
#[derive(Component, ExtractComponent, Clone, Copy, ShaderType, Default)]
struct FullscreenEffect {
    intensity: f32,
}

impl FullscreenMaterial for FullscreenEffect {
    fn fragment_shader() -> ShaderRef {
        "shaders/fullscreen_effect.wgsl".into()
    }
    
    fn node_edges() -> Vec<InternedRenderLabel> {
        vec![
            Node3d::Tonemapping.intern(),
            Self::node_label().intern(),
            Node3d::EndMainPassPostProcessing.intern(),
        ]
    }
}
```

---

## 📋 13. 手动材质管理

### 核心参考 Demo
| 文件 | 说明 |
|------|------|
| `shader_advanced/manual_material.rs` | 手动 Bind Group 管理 |

### 关键借鉴点

#### 13.1 ErasedRenderAsset 实现
```rust
impl ErasedRenderAsset for ImageMaterial {
    type SourceAsset = ImageMaterial;
    type ErasedAsset = PreparedMaterial;
    
    fn prepare_asset(source_asset: Self::SourceAsset, asset_id: AssetId<Self::SourceAsset>, ...) -> Result<Self::ErasedAsset, PrepareAssetError<...>> {
        let unprepared = UnpreparedBindGroup {
            bindings: BindingResources(vec![
                (0, OwnedBindingResource::TextureView(...)),
                (1, OwnedBindingResource::Sampler(...)),
            ]),
        };
        let binding = bind_group_allocator.allocate_unprepared(unprepared, &material_layout);
        // ...
    }
}
```

---

## 📷 14. 深度纹理渲染

### 核心参考 Demo
| 文件 | 说明 |
|------|------|
| `shader_advanced/render_depth_to_texture.rs` | 深度纹理复制到材质 |

### 关键借鉴点

#### 14.1 深度相机设置
```rust
commands.spawn((
    Camera3d::default(),
    Camera {
        RenderTarget::None { size: UVec2::splat(DEPTH_TEXTURE_SIZE) },
        order: -1,  // 在主相机之前渲染
        ..
    },
    DepthPrepass,  // 启用深度预pass
    Msaa::Off,
));
```

#### 14.2 复制深度纹理
```rust
impl ViewNode for CopyDepthTextureNode {
    type ViewQuery = (Read<ExtractedCamera>, Read<ViewDepthTexture>);
    
    fn run(&self, ... (camera, depth_texture): QueryItem<...>, world: &World) -> Result<(), NodeRunError> {
        if camera.order >= 0 { return Ok(()); }  // 只处理深度相机
        
        render_context.add_command_buffer_generation_task(move |render_device| {
            let mut encoder = render_device.create_command_encoder(&CommandEncoderDescriptor {...});
            encoder.copy_texture_to_texture(
                TexelCopyTextureInfo { texture: &depth_texture.texture, aspect: TextureAspect::DepthOnly, ... },
                TexelCopyTextureInfo { texture: &demo_depth_image.texture, aspect: TextureAspect::DepthOnly, ... },
                Extent3d { width: DEPTH_TEXTURE_SIZE, height: DEPTH_TEXTURE_SIZE, ... },
            );
        });
    }
}
```

---

## 📊 功能对照表（更新）

| 功能需求 | 推荐参考 Demo | 关键 API/模式 |
|----------|---------------|---------------|
| **GPU 视锥体剔除** | `compute_shader_game_of_life.rs` | `ComputePipeline` + `RenderGraph Node` |
| **计算结果传递给 Shader** | `storage_buffer.rs` | `AsBindGroup` + `#[storage]` |
| **GPU 数据回读 CPU** | `gpu_readback.rs` | `Readback` + `ReadbackComplete` |
| **减少 Draw Call** | `automatic_instancing.rs` | `MeshTag` + 共享 Handle |
| **高级实例化控制** | `custom_shader_instancing.rs` | `InstanceBuffer` + `DrawMeshInstanced` |
| **自定义渲染管线** | `specialized_mesh_pipeline.rs` | `SpecializedMeshPipeline` |
| **纹理数组绑定** | `texture_binding_array.rs` | `binding_array<texture_2d>` |
| **自定义顶点属性** | `custom_vertex_attribute.rs` | `MeshVertexAttribute` + `at_shader_location` |
| **后处理效果** | `custom_post_processing.rs` | `ViewNode` + `post_process_write` |
| **深度纹理复制** | `render_depth_to_texture.rs` | `copy_texture_to_texture` |
| **全屏着色器** | `fullscreen_material.rs` | `FullscreenMaterial` trait |

---

## 🎬 15. 动画着色器（Time Uniform）

### 核心参考 Demo
| 文件 | 说明 |
|------|------|
| `shader/animate_shader.rs` | 使用全局时间 uniform |
| `shaders/animate_shader.wgsl` | WGSL 着色器源码 |

### 关键借鉴点
```wgsl
#import bevy_pbr::mesh_view_bindings::Globals

// Globals 包含 time, delta_time, frame_count 等
@group(0) @binding(0) var<uniform> globals: Globals;

fn fragment(...) -> vec4<f32> {
    let time = globals.time;  // 运行时间
    // 使用 time 做动画效果
}
```

---

## 🖼️ 16. 纹理数组

### 核心参考 Demo
| 文件 | 说明 |
|------|------|
| `shader/array_texture.rs` | `texture_2d_array` 使用 |

### 关键借鉴点

#### 16.1 加载纹理数组
```rust
let array_texture = asset_server.load_with_settings(
    "textures/array_texture.png",
    |settings: &mut ImageLoaderSettings| {
        settings.array_layout = Some(ImageArrayLayout::RowCount {
            rows: TEXTURE_COUNT,
        });
    },
);

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
struct ArrayTextureMaterial {
    #[texture(0, dimension = "2d_array")]
    #[sampler(1)]
    array_texture: Handle<Image>,
}
```

---

## 🔧 17. Shader Defs（条件编译）

### 核心参考 Demo
| 文件 | 说明 |
|------|------|
| `shader/shader_defs.rs` | 运行时 shader 变体选择 |
| `shaders/shader_defs.wgsl` | WGSL 着色器源码 |

### 关键借鉴点

#### 17.1 定义 MaterialKey
```rust
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
#[bind_group_data(CustomMaterialKey)]
struct CustomMaterial {
    #[uniform(0)]
    color: LinearRgba,
    is_red: bool,
}

#[repr(C)]
#[derive(Eq, PartialEq, Hash, Copy, Clone)]
struct CustomMaterialKey {
    is_red: bool,
}
```

#### 17.2 在 specialize 中注入 shader_defs
```rust
fn specialize(...) -> Result<(), SpecializedMeshPipelineError> {
    if key.bind_group_data.is_red {
        let fragment = descriptor.fragment.as_mut().unwrap();
        fragment.shader_defs.push("IS_RED".into());
    }
    Ok(())
}
```

---

## 🔌 18. Prepass（深度/法线/运动向量）

### 核心参考 Demo
| 文件 | 说明 |
|------|------|
| `shader/shader_prepass.rs` | Depth/Normal/MotionVector Prepass |

### 关键借鉴点

#### 18.1 启用 Prepass
```rust
commands.spawn((
    Camera3d::default(),
    DepthPrepass,           // 深度缓冲
    NormalPrepass,          // 世界法线
    MotionVectorPrepass,    // 运动向量
));
```

---

## 📦 19. Extended Material（材质扩展）

### 核心参考 Demo
| 文件 | 说明 |
|------|------|
| `shader/extended_material.rs` | StandardMaterial 扩展 |
| `shaders/extended_material.wgsl` | WGSL 着色器源码 |

### 关键借鉴点
```rust
use bevy::pbr::{ExtendedMaterial, MaterialExtension, OpaqueRendererMethod};

app.add_plugins(MaterialPlugin::<
    ExtendedMaterial<StandardMaterial, MyExtension>,
>::default());

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone, Default)]
struct MyExtension {
    #[uniform(100)]  // 避开 StandardMaterial 的绑定槽
    quantize_steps: u32,
}

impl MaterialExtension for MyExtension {
    fn fragment_shader() -> ShaderRef { SHADER_ASSET_PATH.into() }
}
```

---

## 🔗 20. Bindless Material（无绑定材质）

### 核心参考 Demo
| 文件 | 说明 |
|------|------|
| `shader/shader_material_bindless.rs` | `#[bindless]` 属性 |

### 关键借鉴点
```rust
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
#[uniform(0, BindlessMaterialUniform, binding_array(10))]
#[bindless(limit(4))]  // 每 4 个材质一组
struct BindlessMaterial {
    color: LinearRgba,
    #[texture(1)]
    #[sampler(2)]
    color_texture: Option<Handle<Image>>,
}
```

---

## 📋 完整功能对照表

| 功能需求 | 推荐参考 Demo | 关键 API/特性 |
|----------|---------------|---------------|
| **GPU 视锥体剔除** | `compute_shader_game_of_life.rs` | ComputePipeline + RenderGraph Node |
| **Storage Buffer** | `storage_buffer.rs` | `AsBindGroup` + `#[storage]` |
| **GPU 回读** | `gpu_readback.rs` | `Readback` + `ReadbackComplete` |
| **减少 Draw Call** | `automatic_instancing.rs` | `MeshTag` + 共享 Handle |
| **高级实例化** | `custom_shader_instancing.rs` | `InstanceBuffer` |
| **自定义渲染管线** | `specialized_mesh_pipeline.rs` | `SpecializedMeshPipeline` |
| **纹理数组** | `array_texture.rs` | `texture_2d_array` |
| **自定义顶点属性** | `custom_vertex_attribute.rs` | `MeshVertexAttribute` |
| **后处理** | `custom_post_processing.rs` | `ViewNode` + `post_process_write` |
| **深度纹理** | `render_depth_to_texture.rs` | `copy_texture_to_texture` |
| **全屏着色器** | `fullscreen_material.rs` | `FullscreenMaterial` |
| **动画着色器** | `animate_shader.rs` | `Globals.time` |
| **Shader Defs** | `shader_defs.rs` | `shader_defs.push()` |
| **Prepass** | `shader_prepass.rs` | `DepthPrepass` / `NormalPrepass` |
| **材质扩展** | `extended_material.rs` | `ExtendedMaterial` + `MaterialExtension` |
| **Bindless** | `shader_material_bindless.rs` | `#[bindless]` |

---

## 🏗️ 推荐的体素渲染优化架构

```
┌─────────────────────────────────────────────────────────────┐
│                    自动实例化渲染                            │
│  - 所有可见区块共享 1 个 Mesh + 1 个 Material                 │
│  - MeshTag 传递区块索引                                     │
│  - 实例数量 = 可见区块数                                    │
└─────────────────────────────────────────────────────────────┘
                            ↓
┌─────────────────────────────────────────────────────────────┐
│                   Storage Buffer 数据传递                    │
│  - chunk_data: array<vec4<u32>>  // 区块体素数据             │
│  - visibility: array<u32>       // 可见性掩码               │
└─────────────────────────────────────────────────────────────┘
                            ↓
┌─────────────────────────────────────────────────────────────┐
│               GPU 视锥体剔除（Compute Shader）                │
│  @compute @workgroup_size(64)                               │
│  fn cull_chunks(...) {                                       │
│      // 视锥体测试，更新 visibility buffer                    │
│  }                                                          │
└─────────────────────────────────────────────────────────────┘
                            ↓
┌─────────────────────────────────────────────────────────────┐
│                  单次 DrawCallInstanced                      │
│  draw_indexed(index_buf, 0, instance_count)                 │
│                                                             │
│  在顶点着色器中：                                            │
│  - 根据 instance_index 从 visibility 获取可见性              │
│  - 根据 instance_index 从 chunk_data 读取区块数据             │
│  - 生成最终顶点位置                                          │
└─────────────────────────────────────────────────────────────┘
```

### Draw Call 优化效果
```
优化前：5445 区块 × 2 DrawCall = 10,890+ Draw Calls
优化后：1 DrawCallInstanced + 少量批次 = <10 Draw Calls
```

---

## 📝 注意事项

1. **视锥体剔除的时机**：需要在相机渲染前执行，因此必须集成到 `RenderGraph`
2. **Storage Buffer 大小**：每个区块 32³ = 32768 体素，需要规划好 buffer 大小和分块策略
3. **实例数量限制**：GPU 有最大实例数量限制（约 65535），需要分批次渲染
4. **数据布局**：`#[repr(C)]` 确保 Rust 和 GPU 数据结构对齐