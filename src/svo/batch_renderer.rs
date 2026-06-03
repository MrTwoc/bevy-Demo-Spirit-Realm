//! 批量渲染系统 (Batch Renderer)
//!
//! 将 SVO 遍历输出的可见节点列表转换为高效的批量渲染调用。
//! 借鉴 Voxy 的 Multi Draw Indirect 思路，减少 DrawCall 数量。
//!
//! # 架构设计
//!
//! ```text
//! ┌─────────────┐     ┌─────────────┐     ┌─────────────┐
//! │ SVO 遍历    │ ──→ │ 可见节点    │ ──→ │ 批量渲染    │
//! │ (GPU)       │     │ 列表        │     │ (GPU)       │
//! └─────────────┘     └─────────────┘     └─────────────┘
//! ```
//!
//! # 优化策略
//!
//! 1. **按材质分组**：相同材质的节点合并渲染
//! 2. **实例化渲染**：使用 Bevy 的实例化 API
//! 3. **LOD 切换**：根据距离动态调整渲染精度

use bevy::prelude::*;
use std::collections::HashMap;

use crate::svo::node_manager::NodeManager;
use crate::svo::SvoRenderQueue;

/// 批量渲染配置
#[derive(Resource, Clone, Debug)]
pub struct BatchRenderConfig {
    /// 是否启用批量渲染
    pub enabled: bool,
    /// 每批次最大节点数
    pub max_batch_size: u32,
    /// 是否启用实例化渲染
    pub use_instancing: bool,
    /// LOD 切换距离阈值
    pub lod_distances: [f32; 4],
}

impl Default for BatchRenderConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_batch_size: 1024,
            use_instancing: true,
            lod_distances: [100.0, 200.0, 400.0, 800.0],
        }
    }
}

/// 渲染批次
#[derive(Clone, Debug)]
pub struct RenderBatch {
    /// 材质 ID
    pub material_id: u32,
    /// 节点 ID 列表
    pub node_ids: Vec<u32>,
    /// LOD 级别
    pub lod_level: u32,
}

impl RenderBatch {
    pub fn new(material_id: u32, lod_level: u32) -> Self {
        Self {
            material_id,
            node_ids: Vec::new(),
            lod_level,
        }
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.node_ids.is_empty()
    }

    /// 添加节点
    pub fn add_node(&mut self, node_id: u32) {
        self.node_ids.push(node_id);
    }

    /// 获取节点数量
    pub fn len(&self) -> usize {
        self.node_ids.len()
    }
}

/// 批量渲染管理器
#[derive(Resource)]
pub struct BatchRenderer {
    /// 渲染批次列表
    pub batches: Vec<RenderBatch>,
    /// 材质到批次的映射
    material_to_batch: HashMap<u32, usize>,
    /// 配置
    pub config: BatchRenderConfig,
    /// 统计信息
    pub stats: BatchRenderStats,
}

/// 批量渲染统计信息
#[derive(Default, Clone, Debug)]
pub struct BatchRenderStats {
    /// 总节点数
    pub total_nodes: u32,
    /// 批次数
    pub batch_count: u32,
    /// 最大批次大小
    pub max_batch_size: u32,
    /// 最小批次大小
    pub min_batch_size: u32,
    /// 平均批次大小
    pub avg_batch_size: f32,
}

impl Default for BatchRenderer {
    fn default() -> Self {
        Self {
            batches: Vec::new(),
            material_to_batch: HashMap::new(),
            config: BatchRenderConfig::default(),
            stats: BatchRenderStats::default(),
        }
    }
}

impl BatchRenderer {
    /// 创建新的批量渲染管理器
    pub fn new(config: BatchRenderConfig) -> Self {
        Self {
            batches: Vec::new(),
            material_to_batch: HashMap::new(),
            config,
            stats: BatchRenderStats::default(),
        }
    }

    /// 清空所有批次
    pub fn clear(&mut self) {
        self.batches.clear();
        self.material_to_batch.clear();
        self.stats = BatchRenderStats::default();
    }

    /// 添加节点到批次
    pub fn add_node(&mut self, node_id: u32, material_id: u32, lod_level: u32) {
        // 查找或创建批次
        let batch_key = (material_id << 4) | lod_level;
        let batch_idx = if let Some(&idx) = self.material_to_batch.get(&batch_key) {
            idx
        } else {
            let idx = self.batches.len();
            self.batches.push(RenderBatch::new(material_id, lod_level));
            self.material_to_batch.insert(batch_key, idx);
            idx
        };

        // 添加节点到批次
        self.batches[batch_idx].add_node(node_id);
    }

    /// 从 SVO 渲染队列构建批次
    pub fn build_from_render_queue(
        &mut self,
        render_queue: &SvoRenderQueue,
        node_manager: &NodeManager,
    ) {
        self.clear();

        if !self.config.enabled {
            return;
        }

        // 遍历可见节点列表
        for &node_id in render_queue.node_ids() {
            // 获取节点数据 (使用新的 NodeStore API)
            let geometry_handle = node_manager.store.get_node_geometry(node_id);
            let position = node_manager.store.node_position(node_id);
            let lod_level = crate::svo::decode_level(position);

            // 跳过空几何体
            // get_node_geometry 返回 i32:
            //   -1: 无几何体 (null)
            //   -2: 空几何体 (empty)
            //   >= 0: 有效的几何体 ID
            if geometry_handle < 0 {
                continue;
            }

            // 添加到批次
            self.add_node(node_id, geometry_handle as u32, lod_level);
        }

        // 更新统计信息
        self.update_stats();
    }

    /// 更新统计信息
    fn update_stats(&mut self) {
        let total_nodes: u32 = self.batches.iter().map(|b| b.len() as u32).sum();
        let batch_count = self.batches.len() as u32;

        let (max_size, min_size) = if batch_count > 0 {
            let max = self.batches.iter().map(|b| b.len()).max().unwrap_or(0) as u32;
            let min = self.batches.iter().map(|b| b.len()).min().unwrap_or(0) as u32;
            (max, min)
        } else {
            (0, 0)
        };

        let avg_size = if batch_count > 0 {
            total_nodes as f32 / batch_count as f32
        } else {
            0.0
        };

        self.stats = BatchRenderStats {
            total_nodes,
            batch_count,
            max_batch_size: max_size,
            min_batch_size: min_size,
            avg_batch_size: avg_size,
        };
    }

    /// 获取批次列表
    pub fn batches(&self) -> &[RenderBatch] {
        &self.batches
    }

    /// 获取统计信息
    pub fn stats(&self) -> &BatchRenderStats {
        &self.stats
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.batches.is_empty()
    }

    /// 获取总节点数
    pub fn total_nodes(&self) -> u32 {
        self.stats.total_nodes
    }
}

/// 批量渲染插件
pub struct BatchRendererPlugin;

impl Plugin for BatchRendererPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BatchRenderer>()
           .init_resource::<BatchRenderConfig>()
           .add_systems(Update, update_batch_renderer);
    }
}

/// 更新批量渲染器的系统
fn update_batch_renderer(
    mut batch_renderer: ResMut<BatchRenderer>,
    render_queue: Res<SvoRenderQueue>,
    node_manager: Res<NodeManager>,
    config: Res<BatchRenderConfig>,
) {
    // 每帧重建批次
    if render_queue.has_new_data {
        batch_renderer.config = config.clone();
        batch_renderer.build_from_render_queue(&render_queue, &node_manager);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_batch() {
        let mut batch = RenderBatch::new(1, 0);
        assert!(batch.is_empty());

        batch.add_node(100);
        batch.add_node(200);
        assert_eq!(batch.len(), 2);
        assert!(!batch.is_empty());
    }

    #[test]
    fn test_batch_renderer() {
        let mut renderer = BatchRenderer::default();
        assert!(renderer.is_empty());

        renderer.add_node(100, 1, 0);
        renderer.add_node(200, 1, 0);
        renderer.add_node(300, 2, 1);

        assert_eq!(renderer.total_nodes(), 3);
        assert_eq!(renderer.batches().len(), 2);
    }
}
