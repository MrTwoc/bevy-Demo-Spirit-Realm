//! Voxel Indirect Draw 渲染管线
//!
//! 管理 BindGroupLayout、RenderPipeline、View Uniform Buffer。
//! 所有 GPU 资源在渲染世界中创建（RenderStartup 阶段）。

use std::borrow::Cow;

use bevy::{
    prelude::*,
    render::{
        render_resource::{
            BindGroup, BindGroupLayout, BindGroupLayoutDescriptor, BindGroupLayoutEntry,
            BindGroupEntry, BindingResource, BufferBinding, BufferBindingType,
            BufferInitDescriptor, BufferSize, BufferUsages, CachedRenderPipelineId,
            ColorTargetState, ColorWrites, CompareFunction, DepthBiasState, DepthStencilState,
            Face, FrontFace, MultisampleState, Operations, PolygonMode, PrimitiveState,
            PrimitiveTopology, RenderPipelineDescriptor, StencilState, TextureFormat,
            VertexState,
        },
        renderer::RenderDevice,
        view::ViewTarget,
    },
};

use bevy::render::render_resource::{BindingType, PipelineCache, ShaderStages};

/// Indirect Draw 渲染管线资源（渲染世界）
#[derive(Resource)]
pub struct VoxelRenderPipeline {
    /// 管线 ID（用于从 PipelineCache 获取已编译管线）
    pub pipeline_id: CachedRenderPipelineId,
    /// 绑定组布局
    pub layout: BindGroupLayout,
}

/// 每帧创建的绑定组
#[derive(Resource)]
pub struct VoxelBindGroup {
    /// 绑定组（Group 0: b0=vertex, b1=index, b2=offset, b3=view uniform）
    pub bind_group: BindGroup,
}

/// 在渲染世界初始化阶段创建管线
pub fn create_voxel_render_pipeline(world: &mut World) {
    let render_device = world.resource::<RenderDevice>();
    let pipeline_cache = world.resource::<PipelineCache>();

    // ── 缓冲区大小 ────────────────────────────────────────────
    let vertex_size = std::mem::size_of::<super::buffers::PackedVertex>() as u64;
    let offset_size = std::mem::size_of::<super::buffers::ChunkOffset>() as u64;
    let view_size =
        std::mem::size_of::<super::extract::ViewUniformRaw>() as u64;

    // ── 绑定组布局（单 group 4 个 binding）────────────────────
    //
    // Group(0):
    //   b0: vertex_buffer   (storage/read)  - ShaderStages::VERTEX
    //   b1: index_buffer    (storage/read)  - ShaderStages::VERTEX
    //   b2: chunk_offsets   (storage/read)  - ShaderStages::VERTEX
    //   b3: view_uniform    (uniform)       - ShaderStages::VERTEX
    //
    let layout_entries = vec![
        BindGroupLayoutEntry {
            binding: 0,
            visibility: ShaderStages::VERTEX,
            ty: BindingType::Buffer {
                ty: BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: BufferSize::new(vertex_size),
            },
            count: None,
        },
        BindGroupLayoutEntry {
            binding: 1,
            visibility: ShaderStages::VERTEX,
            ty: BindingType::Buffer {
                ty: BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
        BindGroupLayoutEntry {
            binding: 2,
            visibility: ShaderStages::VERTEX,
            ty: BindingType::Buffer {
                ty: BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: BufferSize::new(offset_size),
            },
            count: None,
        },
        BindGroupLayoutEntry {
            binding: 3,
            visibility: ShaderStages::VERTEX,
            ty: BindingType::Buffer {
                ty: BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: BufferSize::new(view_size),
            },
            count: None,
        },
    ];

    let layout_descriptor = BindGroupLayoutDescriptor {
        label: Cow::from("voxel_indirect_layout"),
        entries: layout_entries,
    };

    // ── Shader ───────────────────────────────────────────────
    let shader_asset_path = "shaders/voxel_indirect.wgsl";
    let shader: Handle<Shader> = world.resource::<AssetServer>().load(shader_asset_path);

    // ── 渲染管线 ─────────────────────────────────────────────
    let pipeline_id = pipeline_cache.queue_render_pipeline(RenderPipelineDescriptor {
        label: Some(Cow::from("voxel_indirect_pipeline")),
        layout: vec![layout_descriptor.clone()],
        vertex: VertexState {
            shader: shader.clone(),
            shader_defs: vec![],
            entry_point: Some(Cow::from("vertex")),
            buffers: vec![],
        },
        primitive: PrimitiveState {
            topology: PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: FrontFace::Ccw,
            cull_mode: Some(Face::Back),
            unclipped_depth: false,
            polygon_mode: PolygonMode::Fill,
            conservative: false,
        },
        depth_stencil: Some(DepthStencilState {
            format: TextureFormat::Depth32Float,
            depth_write_enabled: true,
            depth_compare: CompareFunction::Less,
            stencil: StencilState::default(),
            bias: DepthBiasState::default(),
        }),
        multisample: MultisampleState::default(),
        fragment: Some(bevy::render::render_resource::FragmentState {
            shader: shader.clone(),
            shader_defs: vec![],
            entry_point: Some(Cow::from("fragment_debug")),
            targets: vec![Some(ColorTargetState {
                format: ViewTarget::TEXTURE_FORMAT_HDR,
                blend: None,
                write_mask: ColorWrites::ALL,
            })],
        }),
        push_constant_ranges: vec![],
        zero_initialize_workgroup_memory: false,
    });

    // 从管线缓存获取 BindGroupLayout（用描述符在缓存中查找/创建）
    let layout = pipeline_cache.get_bind_group_layout(&layout_descriptor);

    world.insert_resource(VoxelRenderPipeline {
        pipeline_id,
        layout,
    });
}

/// 每帧创建绑定组（在 RenderSet::PrepareBindGroups 中调用）
pub fn prepare_voxel_bind_groups(
    mut commands: Commands,
    pipeline: Res<VoxelRenderPipeline>,
    extracted: Res<super::extract::ExtractedVoxelBuffers>,
    view_data: Res<super::extract::ExtractedViewData>,
    render_device: Res<RenderDevice>,
) {
    if !view_data.is_valid {
        return;
    }
    let Some(ref vertex_buffer) = extracted.vertex_buffer else { return };
    let Some(ref index_buffer) = extracted.index_buffer else { return };
    let Some(ref offset_buffer) = extracted.offset_buffer else { return };

    // 每帧创建 view uniform buffer
    let view_uniform = render_device.create_buffer_with_data(
        &BufferInitDescriptor {
            label: Some("voxel_view_uniform_buffer"),
            contents: bytemuck::bytes_of(&view_data.view_uniform),
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        },
    );

    // Group(0): b0=vertex, b1=index, b2=offset, b3=view uniform
    // 使用原始 wgpu BindGroupEntry 构造（Buffer 实现了 Deref<Target = wgpu::Buffer>）
    let bind_group: BindGroup = render_device.create_bind_group(
        "voxel_indirect_bind_group",
        &pipeline.layout,
        &[
            BindGroupEntry {
                binding: 0,
                resource: BindingResource::Buffer(BufferBinding {
                    buffer: &**vertex_buffer,
                    offset: 0,
                    size: None,
                }),
            },
            BindGroupEntry {
                binding: 1,
                resource: BindingResource::Buffer(BufferBinding {
                    buffer: &**index_buffer,
                    offset: 0,
                    size: None,
                }),
            },
            BindGroupEntry {
                binding: 2,
                resource: BindingResource::Buffer(BufferBinding {
                    buffer: &**offset_buffer,
                    offset: 0,
                    size: None,
                }),
            },
            BindGroupEntry {
                binding: 3,
                resource: BindingResource::Buffer(BufferBinding {
                    buffer: &view_uniform,
                    offset: 0,
                    size: None,
                }),
            },
        ],
    );

    commands.insert_resource(VoxelBindGroup { bind_group });
}
