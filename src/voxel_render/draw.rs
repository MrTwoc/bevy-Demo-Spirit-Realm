//! 绘制命令模块
//!
//! 实现 Indirect Draw 的渲染图节点。
//! 在渲染后处理阶段之前插入，体素区块通过一次 MultiDrawIndexedIndirect 调用完成渲染。

use bevy::{
    prelude::*,
    render::{
        render_graph::{self, RenderGraphContext, RenderLabel, NodeRunError},
        render_resource::*,
        renderer::RenderContext,
        view::{ViewDepthTexture, ViewTarget},
    },
};

use super::extract::ExtractedVoxelBuffers;
use super::pipeline::{VoxelBindGroup, VoxelRenderPipeline};
use super::queue::VoxelDrawCountBuffer;

/// VoxelRenderCommandPlugin（占位，实际渲染通过 VoxelIndirectNode 实现）
pub struct VoxelRenderCommandPlugin;

impl Plugin for VoxelRenderCommandPlugin {
    fn build(&self, _app: &mut App) {
        info!("VoxelRenderCommandPlugin loaded (indirect draw is active)");
    }
}

/// 渲染图节点标签
#[derive(RenderLabel, Debug, Clone, PartialEq, Eq, Hash)]
pub struct VoxelIndirectLabel;

/// 自定义渲染图节点 —— 通过 Indirect Draw 渲染体素区块
pub struct VoxelIndirectNode;

impl render_graph::Node for VoxelIndirectNode {
    fn run(
        &self,
        graph: &mut RenderGraphContext,
        render_context: &mut RenderContext,
        world: &World,
    ) -> Result<(), NodeRunError> {
        // ── 获取资源 ──────────────────────────────────────────
        let view_entity = match graph.get_view_entity() {
            Some(ve) => ve,
            None => return Ok(()), // view entity 尚未设置，跳过
        };
        let view_target = match world.entity(view_entity).get::<ViewTarget>() {
            Some(vt) => vt,
            None => return Ok(()),
        };

        let pipeline = match world.get_resource::<VoxelRenderPipeline>() {
            Some(p) => p,
            None => return Ok(()),
        };
        let pipeline_cache = match world.get_resource::<PipelineCache>() {
            Some(pc) => pc,
            None => return Ok(()),
        };
        let bind_group = match world.get_resource::<VoxelBindGroup>() {
            Some(bg) => bg,
            None => return Ok(()),
        };
        let extracted = match world.get_resource::<ExtractedVoxelBuffers>() {
            Some(e) => e,
            None => return Ok(()),
        };
        let count_buffer = match world.get_resource::<VoxelDrawCountBuffer>() {
            Some(cb) => cb,
            None => return Ok(()),
        };

        // ── 获取已编译管线 ──────────────────────────────────
        let render_pipeline = match pipeline_cache.get_render_pipeline(pipeline.pipeline_id) {
            Some(rp) => rp,
            None => return Ok(()),
        };

        // ── 检查数据有效性 ──────────────────────────────────
        let Some(indirect_buffer) = &extracted.indirect_buffer else {
            return Ok(());
        };
        let chunk_count = extracted.chunk_count;
        if chunk_count == 0 {
            return Ok(());
        }

        // ── 获取深度纹理视图 ──────────────────────────
        let depth_stencil_attachment = world
            .entity(view_entity)
            .get::<ViewDepthTexture>()
            .map(|depth| RenderPassDepthStencilAttachment {
                view: depth.view(),
                depth_ops: Some(Operations {
                    load: LoadOp::Load,   // 复用深度缓冲（深度测试通过即可）
                    store: StoreOp::Store,
                }),
                stencil_ops: None,
            });

        // ── 创建 RenderPass ─────────────────────────────────
        let mut render_pass = render_context.begin_tracked_render_pass(RenderPassDescriptor {
            label: Some("voxel_indirect_pass"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: view_target.main_texture_view(),
                resolve_target: None,
                depth_slice: None,
                ops: Operations {
                    load: LoadOp::Load,   // 复用已有渲染结果
                    store: StoreOp::Store, // 保留
                },
            })],
            depth_stencil_attachment,
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        // ── 设置管线 & 绑定组 ──────────────────────────────
        render_pass.set_render_pipeline(render_pipeline);
        // Group(0): storage buffers (vertex / index / offset / view uniform)
        render_pass.set_bind_group(0, &bind_group.bind_group, &[]);

        // ── 执行 Indirect Draw ─────────────────────────────
        render_pass.multi_draw_indexed_indirect_count(
            indirect_buffer,
            0,               // indirect buffer 的起始偏移（从第一个命令开始）
            &count_buffer.buffer,
            0,               // count buffer 的起始偏移
            chunk_count.max(extracted.max_chunks), // 最大命令数（安全性限制）
        );

        Ok(())
    }
}
