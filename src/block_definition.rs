//! 方块定义系统 — JSON 驱动的方块属性和纹理映射
//!
//! 从 `assets/blockstates/*.json` 加载方块定义，替代硬编码映射。
//! 每个 JSON 文件定义一种方块的 ID、名称、属性和纹理映射。
//!
//! # JSON 格式
//!
//! ```json
//! {
//!   "id": 1,
//!   "name": "grass_block",
//!   "solid": true,
//!   "transparent": false,
//!   "textures": {
//!     "top": "grass_block_top",
//!     "bottom": "dirt",
//!     "side": "grass_block_side"
//!   }
//! }
//! ```
//!
//! # 面名称
//!
//! - `top` — 顶面（+Y）
//! - `bottom` — 底面（-Y）
//! - `side` — 四个侧面（±X, ±Z）

use bevy::prelude::*;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

/// 方块纹理映射
#[derive(Debug, Clone, Deserialize)]
pub struct BlockTextures {
    pub top: String,
    pub bottom: String,
    pub side: String,
}

/// 单个方块的完整定义
#[derive(Debug, Clone, Deserialize)]
pub struct BlockDefinition {
    pub id: u8,
    pub name: String,
    /// 是否为实心方块（影响面剔除）
    #[serde(default = "default_true")]
    pub solid: bool,
    /// 是否为透明方块（影响材质选择和面剔除）
    #[serde(default)]
    pub transparent: bool,
    pub textures: BlockTextures,
}

fn default_true() -> bool {
    true
}

/// 方块属性查找表（可跨线程发送，用于面剔除热路径）
///
/// 使用 `[bool; 256]` 固定数组实现 O(1) 查找，
/// 与 `UvLookupTable` 模式一致。
#[derive(Debug, Clone)]
pub struct BlockPropertiesTable {
    solid: [bool; 256],
    transparent: [bool; 256],
}

impl Default for BlockPropertiesTable {
    fn default() -> Self {
        // 默认：所有方块都是实心非透明（向后兼容）
        Self {
            solid: [true; 256],
            transparent: [false; 256],
        }
    }
}

impl BlockPropertiesTable {
    pub fn from_definitions(definitions: &HashMap<u8, BlockDefinition>) -> Self {
        let mut table = Self::default();
        // 空气默认非实心
        table.solid[0] = false;
        for def in definitions.values() {
            table.solid[def.id as usize] = def.solid;
            table.transparent[def.id as usize] = def.transparent;
        }
        table
    }

    #[inline]
    pub fn is_solid(&self, block_id: u8) -> bool {
        self.solid[block_id as usize]
    }

    #[inline]
    pub fn is_transparent(&self, block_id: u8) -> bool {
        self.transparent[block_id as usize]
    }
}

// ── 全局方块属性表（用于 is_block_solid 热路径） ──

/// 全局方块属性表，在资源包加载时初始化。
/// 使用 `OnceLock` 保证线程安全的一次性初始化。
static BLOCK_PROPERTIES: OnceLock<BlockPropertiesTable> = OnceLock::new();

/// 初始化全局方块属性表（在 `load_resource_pack` 中调用）
pub fn init_block_properties(table: BlockPropertiesTable) {
    let _ = BLOCK_PROPERTIES.set(table);
}

/// 判断方块是否为实心（从全局属性表读取）
///
/// 如果属性表尚未初始化（理论上不应发生），回退到默认行为。
#[inline]
pub fn is_block_solid_from_table(block_id: u8) -> bool {
    match BLOCK_PROPERTIES.get() {
        Some(table) => table.is_solid(block_id),
        None => {
            // 回退：空气和水非实心，其余实心
            !matches!(block_id, 0 | 5)
        }
    }
}

/// 从 `assets/blockstates/` 目录加载所有方块定义
///
/// 扫描目录下的所有 `.json` 文件，解析为 `BlockDefinition`，
/// 返回 `HashMap<block_id, BlockDefinition>`。
pub fn load_block_definitions() -> HashMap<u8, BlockDefinition> {
    let blockstates_dir = Path::new("assets/blockstates");
    let mut definitions = HashMap::new();

    if !blockstates_dir.exists() {
        warn!(
            "Blockstates directory not found: {:?}, using empty definitions",
            blockstates_dir
        );
        return definitions;
    }

    let entries = match std::fs::read_dir(blockstates_dir) {
        Ok(e) => e,
        Err(e) => {
            warn!("Failed to read blockstates directory: {}", e);
            return definitions;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().map_or(true, |ext| ext != "json") {
            continue;
        }

        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                warn!("Failed to read {:?}: {}", path, e);
                continue;
            }
        };

        match serde_json::from_str::<BlockDefinition>(&content) {
            Ok(def) => {
                let filename = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("?");
                info!(
                    "  Loaded block definition: {} (id={}, solid={}, transparent={})",
                    filename, def.id, def.solid, def.transparent
                );
                definitions.insert(def.id, def);
            }
            Err(e) => {
                warn!("Failed to parse {:?}: {}", path, e);
            }
        }
    }

    info!(
        "Loaded {} block definitions from blockstates/",
        definitions.len()
    );
    definitions
}
