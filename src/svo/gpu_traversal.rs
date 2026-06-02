//! GPU 八叉树遍历 (Compute Shader)
//!
//! Phase 1: 在 GPU 上对 top-level 节点执行距离 + 视锥剔除
//! 使用 wgpu BufferBinding 直接绑定原始 Buffer

use std::borrow::Cow;
use std::sync::Arc;

use bevy::{
    prelude::*,
    render::{
        extract_resource::{ExtractResource, ExtractResourcePlugin},
        graph::CameraDriverLabel,
        render_graph::{self, RenderGraph, RenderLabel, NodeRunError},
        render_resource::binding_types::{
            storage_buffer_read_only_sized, storage_buffer_sized, uniform_buffer_sized,
        },
        render_resource::*,
        renderer::{RenderContext, RenderDevice, RenderQueue},
        Extract, ExtractSchedule, Render, RenderApp, RenderStartup, RenderSystems,
    },
};

use crate::svo::node_manager::{NodeManager, GpuNode};

// ── 常量 ──────────────────────────────────────────────────────────────

const WORKGROUP_SIZE: u32 = 64;
const MAX_VISIBLE_NODES: u32 = 16384;
const SHADER_ASSET_PATH: &str = "shaders/svo_traversal.wgsl";

// ── CPU 侧数据 (Main World) ──────────────────────────────────────────

/// SVO 遍历提取数据
#[derive(Resource, Clone, ExtractResource, Default)]
pub struct SvoExtractData {
    /// 使用 Arc 避免 clone 整个 Vec（generation 未变时仅 O(1) 引用计数递增）
    pub node_data: Arc<Vec<GpuNode>>,
    pub node_count: u32,
    pub camera_world_x: f32,
    pub camera_world_y: f32,
    pub camera_world_z: f32,
    pub render_distance: f32,
    /// 节点数据是否有变化（自上次 GPU 上传以来）
    pub nodes_changed: bool,
}

// ── Camera Uniform (对齐 WGSL) ──────────────────────────────────────

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct CameraUniformRaw {
    camera_world_x: f32,
    camera_world_y: f32,
    camera_world_z: f32,
    render_distance: f32,
    frustum_planes: [FrustumPlane; 6],
    node_count: u32,
    // WGSL 要求 struct 大小是最大成员 (vec4<f32>) alignment (16) 的倍数
    // 前面的字段共 112 + 4 + 4 = 120 字节，需要 8 字节填充到 128
    _pad: u32,
    _pad2: u32,
    _pad3: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct FrustumPlane {
    x: f32, y: f32, z: f32, w: f32,
}

/// 从 Camera 的 clip_from_view 矩阵提取 6 个视锥体平面
fn extract_frustum_planes(camera: &Camera) -> [FrustumPlane; 6] {
    let matrix = camera.clip_from_view();
    let rows = [
        matrix.row(3) + matrix.row(0),
        matrix.row(3) - matrix.row(0),
        matrix.row(3) + matrix.row(1),
        matrix.row(3) - matrix.row(1),
        matrix.row(3) + matrix.row(2),
        matrix.row(3) - matrix.row(2),
    ];

    let mut planes = [FrustumPlane { x: 0.0, y: 0.0, z: 0.0, w: 0.0 }; 6];
    for (i, row) in rows.iter().enumerate() {
        let len = (row.x * row.x + row.y * row.y + row.z * row.z).sqrt();
        if len > 0.0 {
            let inv_len = 1.0 / len;  // 除法转乘法
            planes[i] = FrustumPlane {
                x: row.x * inv_len, y: row.y * inv_len,
                z: row.z * inv_len, w: row.w * inv_len,
            };
        }
    }
    planes
}

// ── GPU 侧资源 (Render World) ──────────────────────────────────────

#[derive(Resource)]
struct SvoGpuBuffers {
    node_buffer: Buffer,
    camera_buffer: Buffer,
    counter_buffer: Buffer,
    #[expect(unused, reason = "exclusively GPU-accessible via bind group")]
    visible_nodes_buffer: Buffer,
    bind_group: BindGroup,
}

#[derive(Resource)]
struct SvoRenderPipeline {
    pipeline_id: CachedComputePipelineId,
}

// ── 提取系统 (Main World → Render World) ──────────────────────────

fn extract_svo_data(
    mut data: ResMut<SvoExtractData>,
    node_manager: Extract<Res<NodeManager>>,
    camera_query: Query<&Transform>,
) {
    // ── GPU 节点数据：只在有脏节点时重建 ──
    let needs_update = node_manager.has_dirty_nodes();
    data.nodes_changed = needs_update;
    if needs_update {
        data.node_data = node_manager.gpu_node_data();
        data.node_count = node_manager.node_count();
    }
    // 没有脏节点时保留 node_data 为空 vec（ExtractResource 克隆只传空 vec）
    // prepare_gpu_resources 会跳过 upload

    // ── 相机数据（始终更新） ──
    // 主世界直接查询 Camera — Extract 包装的 Query 不支持 .get_single()
    if let Some(transform) = camera_query.iter().next() {
        data.camera_world_x = transform.translation.x;
        data.camera_world_y = transform.translation.y;
        data.camera_world_z = transform.translation.z;
    }
}

// ── 初始化管线 (Render Startup) ──────────────────────────────────────

fn init_render_pipeline(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    pipeline_cache: Res<PipelineCache>,
    render_device: Res<RenderDevice>,
) {
    let shader: Handle<Shader> = asset_server.load(SHADER_ASSET_PATH);

    let node_size = BufferSize::new(std::mem::size_of::<GpuNode>() as u64);
    let camera_size = BufferSize::new(std::mem::size_of::<CameraUniformRaw>() as u64).unwrap();
    let counter_size = BufferSize::new(4).unwrap();
    let visible_nodes_size = BufferSize::new(MAX_VISIBLE_NODES as u64 * 4).unwrap();

    // 构建布局描述 — BindGroupLayoutEntries 会 Deref 成 &[BindGroupLayoutEntry]
    let layout_entries = BindGroupLayoutEntries::sequential(
        ShaderStages::COMPUTE,
        (
            storage_buffer_read_only_sized(false, node_size),           // 0: nodes (只读)
            uniform_buffer_sized(false, Some(camera_size)),             // 1: camera
            storage_buffer_sized(false, Some(counter_size)),            // 2: counter
            storage_buffer_sized(false, Some(visible_nodes_size)),      // 3: visible nodes
        ),
    );

    // BindGroupLayoutDescriptor — ComputePipelineDescriptor.layout 需要这个
    let layout_descriptor = BindGroupLayoutDescriptor::new(
        "svo_traversal_layout",
        &layout_entries,
    );

    // 创建实际 BindGroupLayout 用于创建 BindGroup
    // RenderDevice::create_bind_group_layout(label, &[BindGroupLayoutEntry])
    let bind_group_layout = render_device.create_bind_group_layout(
        Some("svo_traversal_layout"),
        &layout_entries,
    );

    // ComputePipelineDescriptor.layout 接受 Vec<BindGroupLayoutDescriptor>
    let pipeline_id = pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
        label: Some(Cow::from("svo_traversal_pipeline")),
        layout: vec![layout_descriptor],
        shader,
        shader_defs: vec![],
        entry_point: Some("main".into()),
        push_constant_ranges: vec![],
        zero_initialize_workgroup_memory: false,
    });

    // 创建 GPU 缓冲区
    let node_buffer = render_device.create_buffer(&BufferDescriptor {
        label: Some("svo_node_buffer"),
        size: (1 << 18) * std::mem::size_of::<GpuNode>() as u64,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let camera_buffer = render_device.create_buffer(&BufferDescriptor {
        label: Some("svo_camera_uniform"),
        size: std::mem::size_of::<CameraUniformRaw>() as u64,
        usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let counter_buffer = render_device.create_buffer(&BufferDescriptor {
        label: Some("svo_counter"),
        size: 4,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });

    let visible_nodes_buffer = render_device.create_buffer(&BufferDescriptor {
        label: Some("svo_visible_nodes"),
        size: MAX_VISIBLE_NODES as u64 * 4,
        usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });

    // BindGroupEntries::sequential 接受实现了 IntoBinding 的类型
    // wgpu::BufferBinding 实现了 IntoBinding
    let bind_group = render_device.create_bind_group(
        Some("svo_traversal_bind_group"),
        &bind_group_layout,
        &BindGroupEntries::sequential((
            BufferBinding {
                buffer: &*node_buffer,
                offset: 0,
                size: None,
            },
            BufferBinding {
                buffer: &*camera_buffer,
                offset: 0,
                size: None,
            },
            BufferBinding {
                buffer: &*counter_buffer,
                offset: 0,
                size: None,
            },
            BufferBinding {
                buffer: &*visible_nodes_buffer,
                offset: 0,
                size: None,
            },
        )),
    );

    commands.insert_resource(SvoGpuBuffers {
        node_buffer,
        camera_buffer,
        counter_buffer,
        visible_nodes_buffer,
        bind_group,
    });

    commands.insert_resource(SvoRenderPipeline { pipeline_id });
}

// ── 准备数据系统 (每帧) ──────────────────────────────────────────

fn prepare_gpu_resources(
    gpu_buffers: Res<SvoGpuBuffers>,
    data: Res<SvoExtractData>,
    queue: Res<RenderQueue>,
    camera_query: Query<&Camera>,
) {
    let camera_uniform = if let Ok(camera) = camera_query.single() {
        let frustum_planes = extract_frustum_planes(camera);
        CameraUniformRaw {
            camera_world_x: data.camera_world_x,
            camera_world_y: data.camera_world_y,
            camera_world_z: data.camera_world_z,
            render_distance: data.render_distance,
            frustum_planes,
            node_count: data.node_count,
            _pad: 0,
            _pad2: 0,
            _pad3: 0,
        }
    } else {
        CameraUniformRaw {
            camera_world_x: 0.0, camera_world_y: 0.0, camera_world_z: 0.0,
            render_distance: 512.0,
            frustum_planes: [FrustumPlane { x: 0.0, y: 0.0, z: 0.0, w: 0.0 }; 6],
            node_count: 0, _pad: 0, _pad2: 0, _pad3: 0,
        }
    };

    // 上传节点数据（仅当有变化时）
    if data.nodes_changed && data.node_count > 0 {
        let bytes: &[u8] = bytemuck::cast_slice(&data.node_data);
        queue.write_buffer(&gpu_buffers.node_buffer, 0, bytes);
    }

    // 上传 camera uniform
    let binding = [camera_uniform];
    let camera_bytes: &[u8] = bytemuck::cast_slice(&binding);
    queue.write_buffer(&gpu_buffers.camera_buffer, 0, camera_bytes);

    // 重置计数器
    let zero: [u32; 1] = [0];
    queue.write_buffer(&gpu_buffers.counter_buffer, 0, bytemuck::cast_slice(&zero));
}

// ── Render Graph Node ──────────────────────────────────────────────

#[derive(Debug, Hash, PartialEq, Eq, Clone, RenderLabel)]
struct SvoTraversalLabel;

struct SvoTraversalNode;

impl render_graph::Node for SvoTraversalNode {
    fn run(
        &self,
        _graph: &mut render_graph::RenderGraphContext,
        render_context: &mut RenderContext,
        world: &World,
    ) -> Result<(), NodeRunError> {
        let gpu = world.get_resource::<SvoGpuBuffers>().unwrap();
        let pipeline_cache = world.get_resource::<PipelineCache>().unwrap();
        let svo_pipeline = world.get_resource::<SvoRenderPipeline>().unwrap();
        let data = world.get_resource::<SvoExtractData>().unwrap();

        if data.node_count == 0 {
            return Ok(());
        }

        let compute_pipeline = match pipeline_cache.get_compute_pipeline(svo_pipeline.pipeline_id) {
            Some(p) => p,
            None => return Ok(()),
        };

        let mut pass = render_context
            .command_encoder()
            .begin_compute_pass(&ComputePassDescriptor::default());

        pass.set_pipeline(compute_pipeline);
        pass.set_bind_group(0, &gpu.bind_group, &[]);
        let dispatch = (data.node_count + WORKGROUP_SIZE - 1) / WORKGROUP_SIZE;
        pass.dispatch_workgroups(dispatch, 1, 1);

        Ok(())
    }
}

// ── 插件 ──────────────────────────────────────────────────────────

pub struct SvoGpuTraversalPlugin;

impl Plugin for SvoGpuTraversalPlugin {
    fn build(&self, app: &mut App) {
        // 主世界: 注册提取资源
        app.init_resource::<SvoExtractData>();
        app.add_plugins(ExtractResourcePlugin::<SvoExtractData>::default());

        // 渲染世界: 提取系统 + 渲染节点
        let render_app = app.sub_app_mut(RenderApp);
        render_app
            .init_resource::<SvoExtractData>()
            .add_systems(ExtractSchedule, extract_svo_data)
            .add_systems(RenderStartup, init_render_pipeline)
            .add_systems(Render, prepare_gpu_resources.in_set(RenderSystems::PrepareBindGroups));

        // 注册渲染图节点
        let mut render_graph = render_app.world_mut().resource_mut::<RenderGraph>();
        render_graph.add_node(SvoTraversalLabel, SvoTraversalNode);
        render_graph.add_node_edge(SvoTraversalLabel, CameraDriverLabel);
    }
}
