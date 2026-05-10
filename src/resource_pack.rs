//! Resource Pack System — 动态 Atlas 构建和材质包加载
//!
//! 材质包统一存放在 `assets/resourcepacks/` 目录下，
//! 每个子文件夹即为一个材质包。可通过 `ResourcePackManager::selected_pack`
//! 指定要使用的材质包名称（子文件夹名），默认使用第一个找到的材质包。
//!
//! # 方块→纹理映射机制（当前实现 vs Minecraft）
//!
//! ## 当前实现（硬编码映射）
//!
//! 当前使用 `block_texture_map: HashMap<(u8, String), String>` 进行映射：
//! - Key: `(block_id, face)` — 方块 ID + 面名称（"top"/"bottom"/"side"）
//! - Value: 纹理名称（对应材质包中的 PNG 文件名，不含扩展名）
//!
//! 映射链路：
//! ```text
//! block_id + face → block_texture_map → texture_name → Atlas UV
//! ```
//!
//! ## Minecraft 实现（JSON 驱动映射）
//!
//! Minecraft 使用四层 JSON 引用链：
//! ```text
//! block_id → blockstates/*.json → models/*.json → textures → Atlas UV
//! ```
//!
//! 示例（草方块）：
//! ```json
//! // blockstates/grass_block.json
//! { "variants": { "": { "model": "block/grass_block" } } }
//!
//! // models/block/grass_block.json
//! {
//!   "textures": {
//!     "top": "block/grass_top",
//!     "side": "block/grass_side",
//!     "bottom": "block/dirt"
//!   },
//!   "elements": [{
//!     "faces": {
//!       "up":    { "texture": "#top" },
//!       "north": { "texture": "#side" },
//!       "down":  { "texture": "#bottom" }
//!     }
//!   }]
//! }
//! ```
//!
//! ## 待完善事项（TODO）
//!
//! 1. **Phase 2**: 实现 `blockstates/*.json` 解析，替代硬编码映射
//! 2. **Phase 3**: 实现 `models/*.json` 解析，支持复杂模型和状态变体
//! 3. **纹理命名规范化**: 从简单文件名改为路径式命名（如 `block/grass_top`）
//! 4. **回退机制**: 纹理缺失时显示紫黑棋盘格（missing texture）
//!
//! 参见: `docs/动态Atlas材质包系统.md`

use bevy::prelude::*;
use bevy::reflect::TypePath;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// 需要生物群系着色（biome tint）的纹理列表
///
/// Minecraft 中某些纹理是灰度的，需要乘以生物群系颜色才能显示正确颜色。
/// 这里使用默认平原生物群系的草方块颜色 (0x8AB656)。
///
/// # 着色原理
///
/// 原始像素 RGB × 着色颜色 RGB = 最终颜色
/// 例如：灰度值 (0.8, 0.8, 0.8) × (0.54, 0.71, 0.34) = (0.43, 0.57, 0.27)
const BIOME_TINTED_TEXTURES: &[(&str, [f32; 3])] = &[
    // grass_block_top: 平原生物群系默认草色 #8AB656
    ("grass_block_top", [0.54, 0.71, 0.34]),
    // grass_block_side: 侧面也需要轻微着色
    ("grass_block_side", [0.75, 0.88, 0.60]),
];

/// 材质包根目录（所有材质包存放于此）
pub const RESOURCE_PACKS_DIR: &str = "assets/resourcepacks";

/// 纹理在 Atlas 中的位置信息
#[derive(Debug, Clone)]
pub struct TextureInfo {
    pub position: (u32, u32),
    pub size: (u32, u32),
    /// UV 坐标。Texture Array 模式下编码为 (layer, layer+1, 0.0, 1.0)
    pub uv: (f32, f32, f32, f32),
    /// Texture Array 层索引
    pub layer_index: u32,
}

/// 动态 Atlas 图集
#[derive(Debug)]
pub struct TextureAtlas {
    pub image: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub textures: HashMap<String, TextureInfo>,
    pub size: (u32, u32),
    /// Texture Array 像素数据（所有纹理按层排列）
    pub array_pixels: Vec<u8>,
    /// Texture Array 层数
    pub array_layers: u32,
    /// 纹理名称 → 层索引映射
    pub texture_index_map: HashMap<String, u32>,
    /// 单个纹理的统一尺寸（Texture Array 中每层的宽高）
    pub tex_size: u32,
}

/// 自定义体素材质，使用 Texture Array 存储方块纹理。
///
/// UV 编码方式：UV.x = texture_layer_index + actual_u, UV.y = actual_v
/// 着色器解码：layer = floor(UV.x), sample_uv = fract(UV.x), UV.y
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct VoxelMaterial {
    #[texture(0, dimension = "2d_array")]
    #[sampler(1)]
    pub array_texture: Handle<Image>,
}

impl Material for VoxelMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/voxel.wgsl".into()
    }
}

/// 资源包管理器
#[derive(Resource)]
pub struct ResourcePackManager {
    /// 材质包根目录
    pub packs_dir: PathBuf,
    /// 当前选中的材质包名称（子文件夹名），None 表示自动选择第一个
    pub selected_pack: Option<String>,
    /// 当前实际加载的材质包路径
    pub current_pack: PathBuf,
    /// 所有可用材质包列表
    pub available_packs: Vec<String>,
    pub texture_cache: HashMap<String, (Vec<u8>, u32, u32)>, // (pixels, width, height)
    pub atlas: Option<TextureAtlas>,
    /// 方块→纹理映射表（硬编码，待改为 JSON 驱动）
    ///
    /// # 映射结构
    ///
    /// - **Key**: `(block_id: u8, face: String)`
    ///   - `block_id`: 方块类型 ID（1=草方块, 2=石头, 3=泥土, 4=沙子）
    ///   - `face`: 面名称，取值为 "top"、"bottom"、"side"
    ///
    /// - **Value**: `texture_name: String`
    ///   - 对应材质包中的 PNG 文件名（不含扩展名）
    ///   - 例如 "dirt" 对应 `assets/resourcepacks/1号材质包/dirt.png`
    ///
    /// # 与 Minecraft 的对比
    ///
    /// | 方面 | 当前实现 | Minecraft |
    /// |------|----------|-----------|
    /// | 映射方式 | 硬编码 HashMap | blockstates/models JSON |
    /// | 面定义 | 仅 top/bottom/side | up/down/north/south/east/west |
    /// | 状态变体 | 不支持 | 支持（如朝向、水位等） |
    /// | 模型复用 | 不支持 | 支持（JSON 引用） |
    ///
    /// # TODO: Phase 2-3 改造
    ///
    /// 将此硬编码映射替换为 JSON 驱动的映射系统：
    /// 1. 加载 `blockstates/*.json` 获取方块→模型映射
    /// 2. 加载 `models/*.json` 获取模型→纹理映射
    /// 3. 解析 `#variable` 引用（如 `#side` → `block/grass_side`）
    pub block_texture_map: HashMap<(u8, String), String>,
    /// 预构建的 UV 数组缓存，用于主线程网格生成的 O(1) 零分配查找。
    ///
    /// `[block_id][face_index]` -> UV 坐标
    /// face_index: 0=top, 1=bottom, 2=side
    /// 在 `build_atlas()` 时自动构建。
    block_uv_array: [[Option<(f32, f32, f32, f32)>; 3]; 256],
}

impl Default for ResourcePackManager {
    fn default() -> Self {
        Self {
            packs_dir: PathBuf::from(RESOURCE_PACKS_DIR),
            selected_pack: None,
            current_pack: PathBuf::new(),
            available_packs: Vec::new(),
            texture_cache: HashMap::new(),
            atlas: None,
            block_texture_map: Self::default_block_texture_map(),
            block_uv_array: [[None; 3]; 256],
        }
    }
}

impl ResourcePackManager {
    /// 默认的方块纹理映射表（硬编码，待改为 JSON 驱动）
    ///
    /// # 当前映射关系
    ///
    /// | block_id | 方块名称 | top | bottom | side | 备注 |
    /// |----------|----------|-----|--------|------|------|
    /// | 1 | 草方块 | grass_block_top | dirt | grass_block_side | 顶部/侧面/底部分别使用不同材质 |
    /// | 2 | 石头 | stone | stone | stone | 使用 stone.png |
    /// | 3 | 泥土 | dirt | dirt | dirt | - |
    /// | 4 | 沙子 | sand | sand | sand | 使用 sand.png |
    ///
    /// # Minecraft 对应实现
    ///
    /// Minecraft 中草方块的映射（通过 JSON 链）：
    /// ```text
    /// block_id=grass_block
    ///   → blockstates/grass_block.json → model="block/grass_block"
    ///   → models/block/grass_block.json
    ///     → top=#top → "block/grass_top"
    ///     → side=#side → "block/grass_side"
    ///     → bottom=#bottom → "block/dirt"
    /// ```
    ///
    /// # TODO: Phase 2 改造
    ///
    /// 将此函数替换为 JSON 加载逻辑：
    /// ```rust
    /// fn load_block_texture_map_from_json(dir: &Path) -> HashMap<(u8, String), String> {
    ///     // 1. 加载 blockstates/*.json
    ///     // 2. 加载 models/*.json
    ///     // 3. 解析 #variable 引用
    ///     // 4. 构建映射表
    /// }
    /// ```
    fn default_block_texture_map() -> HashMap<(u8, String), String> {
        let mut map = HashMap::new();

        // ─────────────────────────────────────────────────────────────
        // block_id = 1: 草方块 (Grass Block)
        // ─────────────────────────────────────────────────────────────
        // TODO: 替换为 JSON 映射
        // Minecraft 对应: grass_top.png / grass_side.png / dirt.png
        // top 使用 grass_block_side（草方块顶部材质）
        // side 使用 grass_block_side（草方块侧面材质）
        // bottom 使用 dirt（草方块底部为泥土）
        map.insert((1, "top".to_string()), "grass_block_top".to_string());
        map.insert((1, "bottom".to_string()), "dirt".to_string());
        map.insert((1, "side".to_string()), "grass_block_side".to_string());

        // ─────────────────────────────────────────────────────────────
        // block_id = 2: 石头 (Stone)
        // ─────────────────────────────────────────────────────────────
        // TODO: 替换为 JSON 映射
        // Minecraft 对应: stone.png
        // 使用 stone.png 材质
        map.insert((2, "top".to_string()), "stone".to_string());
        map.insert((2, "bottom".to_string()), "stone".to_string());
        map.insert((2, "side".to_string()), "stone".to_string());

        // ─────────────────────────────────────────────────────────────
        // block_id = 3: 泥土 (Dirt)
        // ─────────────────────────────────────────────────────────────
        // TODO: 替换为 JSON 映射
        // Minecraft 对应: dirt.png
        map.insert((3, "top".to_string()), "dirt".to_string());
        map.insert((3, "bottom".to_string()), "dirt".to_string());
        map.insert((3, "side".to_string()), "dirt".to_string());

        // ─────────────────────────────────────────────────────────────
        // block_id = 4: 沙子 (Sand)
        // ─────────────────────────────────────────────────────────────
        // TODO: 替换为 JSON 映射
        // Minecraft 对应: sand.png
        map.insert((4, "top".to_string()), "sand".to_string());
        map.insert((4, "bottom".to_string()), "sand".to_string());
        map.insert((4, "side".to_string()), "sand".to_string());

        map
    }

    /// 扫描 `assets/resourcepacks/` 下所有可用材质包
    pub fn scan_available_packs(&mut self) {
        self.available_packs.clear();
        if let Ok(entries) = std::fs::read_dir(&self.packs_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        self.available_packs.push(name.to_string());
                    }
                }
            }
        }
        self.available_packs.sort();
        info!("Available resource packs: {:?}", self.available_packs);
    }

    /// 从 `assets/resourcepacks/` 加载材质包
    ///
    /// 优先使用 `selected_pack` 指定的材质包；若未指定或不存在，
    /// 则自动选择第一个可用材质包；若目录为空则生成默认材质包。
    pub fn load_resource_pack(&mut self) -> Result<(), String> {
        // 确保材质包根目录存在
        std::fs::create_dir_all(&self.packs_dir).map_err(|e| e.to_string())?;

        // 扫描可用材质包
        self.scan_available_packs();

        // 确定要加载的材质包路径
        let pack_path = self.resolve_pack_path()?;

        info!("Loading resource pack from: {:?}", pack_path);
        self.current_pack = pack_path.clone();

        match self.scan_textures(&pack_path) {
            Ok(count) => {
                info!("Loaded {} textures from {:?}", count, pack_path);
            }
            Err(e) => {
                warn!(
                    "Failed to load from {:?}: {}, generating defaults...",
                    pack_path, e
                );
                self.generate_default_textures(&pack_path)?;
                self.scan_textures(&pack_path)?;
            }
        }

        self.build_atlas()?;

        info!(
            "Resource pack loaded: {} textures, atlas size {:?}",
            self.texture_cache.len(),
            self.atlas.as_ref().map(|a| a.size)
        );
        Ok(())
    }

    /// 解析要使用的材质包路径
    fn resolve_pack_path(&mut self) -> Result<PathBuf, String> {
        // 1. 优先使用 selected_pack 指定的材质包
        if let Some(ref name) = self.selected_pack {
            let path = self.packs_dir.join(name);
            if path.exists() {
                return Ok(path);
            }
            warn!(
                "Selected pack '{}' not found at {:?}, falling back to auto-detect",
                name, path
            );
        }

        // 2. 自动选择第一个可用材质包
        if let Some(first) = self.available_packs.first() {
            let path = self.packs_dir.join(first);
            info!("Auto-selected resource pack: '{}'", first);
            return Ok(path);
        }

        // 3. 没有任何材质包，创建默认的
        warn!("No resource packs found, creating default pack...");
        let default_name = "default";
        let default_path = self.packs_dir.join(default_name);
        self.generate_default_textures(&default_path)?;
        self.available_packs.push(default_name.to_string());
        Ok(default_path)
    }

    /// 切换到指定名称的材质包（运行时调用）
    pub fn switch_pack(&mut self, pack_name: &str) -> Result<(), String> {
        let path = self.packs_dir.join(pack_name);
        if !path.exists() {
            return Err(format!(
                "Resource pack '{}' not found at {:?}",
                pack_name, path
            ));
        }

        self.selected_pack = Some(pack_name.to_string());
        self.current_pack = path.clone();
        self.texture_cache.clear();
        self.atlas = None;

        self.scan_textures(&path)?;
        self.build_atlas()?;

        info!("Switched to resource pack: '{}'", pack_name);
        Ok(())
    }

    /// 扫描目录中的所有 PNG 文件
    fn scan_textures(&mut self, dir: &Path) -> Result<usize, String> {
        if !dir.exists() {
            return Err(format!("Directory not found: {:?}", dir));
        }
        let mut count = 0;
        self.scan_dir_recursive(dir, &mut count)?;
        if count == 0 {
            return Err("No textures found".to_string());
        }
        Ok(count)
    }

    /// 递归扫描目录
    fn scan_dir_recursive(&mut self, dir: &Path, count: &mut usize) -> Result<(), String> {
        for entry in std::fs::read_dir(dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();

            if path.is_dir() {
                self.scan_dir_recursive(&path, count)?;
                continue;
            }

            if path.extension().map_or(false, |ext| ext == "png") {
                let filename = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string();

                match load_png_as_rgba(&path) {
                    Ok((mut pixels, width, height)) => {
                        // 对需要生物群系着色的纹理应用颜色变换
                        if let Some(tint) = BIOME_TINTED_TEXTURES
                            .iter()
                            .find(|(name, _)| *name == filename)
                        {
                            apply_biome_tint(&mut pixels, tint.1);
                            info!(
                                "  Loaded texture: {} ({}x{}) [biome tinted]",
                                filename, width, height
                            );
                        } else {
                            info!("  Loaded texture: {} ({}x{})", filename, width, height);
                        }
                        self.texture_cache.insert(filename, (pixels, width, height));
                        *count += 1;
                    }
                    Err(e) => {
                        warn!("  Failed to load {:?}: {}", path, e);
                    }
                }
            }
        }
        Ok(())
    }

    /// 构建动态 Atlas 图集（同时构建 Texture Array 数据）
    fn build_atlas(&mut self) -> Result<(), String> {
        if self.texture_cache.is_empty() {
            return Err("No textures loaded".to_string());
        }

        let (atlas_width, atlas_height, placements) = self.calculate_atlas_layout()?;

        // 创建 Atlas 图像（RGBA 像素数据）
        let mut atlas_pixels = vec![0u8; (atlas_width * atlas_height * 4) as usize];
        let mut texture_infos = HashMap::new();

        // 构建 Texture Array 数据
        let mut texture_index_map = HashMap::new();
        let mut sorted_names: Vec<&String> = self.texture_cache.keys().collect();
        sorted_names.sort();
        let array_layers = sorted_names.len() as u32;

        // 确定统一的纹理尺寸（使用最大尺寸）
        let tex_size = self
            .texture_cache
            .values()
            .map(|(_, w, h)| (*w).max(*h))
            .max()
            .unwrap_or(16);

        let mut array_pixels = vec![0u8; (tex_size * tex_size * 4 * array_layers) as usize];

        for (layer_idx, name) in sorted_names.iter().enumerate() {
            texture_index_map.insert(name.to_string(), layer_idx as u32);

            if let Some((src_pixels, src_w, src_h)) = self.texture_cache.get(*name) {
                // 复制到 Texture Array 层
                for py in 0..*src_h {
                    for px in 0..*src_w {
                        let src_idx = ((py * src_w + px) * 4) as usize;
                        let dst_idx =
                            ((layer_idx as u32 * tex_size * tex_size + py * tex_size + px) * 4)
                                as usize;
                        if src_idx + 3 < src_pixels.len() && dst_idx + 3 < array_pixels.len() {
                            array_pixels[dst_idx..dst_idx + 4]
                                .copy_from_slice(&src_pixels[src_idx..src_idx + 4]);
                        }
                    }
                }
            }
        }

        for (texture_name, (x, y, _width, _height)) in &placements {
            if let Some((src_pixels, src_w, src_h)) = self.texture_cache.get(texture_name) {
                // 复制像素数据到 Atlas
                for py in 0..*src_h {
                    for px in 0..*src_w {
                        let src_idx = ((py * src_w + px) * 4) as usize;
                        let dst_x = x + px;
                        let dst_y = y + py;
                        let dst_idx = ((dst_y * atlas_width + dst_x) * 4) as usize;
                        if src_idx + 3 < src_pixels.len() && dst_idx + 3 < atlas_pixels.len() {
                            atlas_pixels[dst_idx..dst_idx + 4]
                                .copy_from_slice(&src_pixels[src_idx..src_idx + 4]);
                        }
                    }
                }

                let layer = *texture_index_map.get(texture_name).unwrap_or(&0) as f32;
                // Texture Array UV: x = layer_index + u, y = v
                let u_min = layer;
                let u_max = layer + 1.0;
                let v_min = 0.0;
                let v_max = 1.0;

                texture_infos.insert(
                    texture_name.clone(),
                    TextureInfo {
                        position: (*x, *y),
                        size: (*src_w, *src_h),
                        uv: (u_min, u_max, v_min, v_max),
                        layer_index: *texture_index_map.get(texture_name).unwrap_or(&0),
                    },
                );
            }
        }

        self.atlas = Some(TextureAtlas {
            image: atlas_pixels,
            width: atlas_width,
            height: atlas_height,
            textures: texture_infos,
            size: (atlas_width, atlas_height),
            array_pixels,
            array_layers,
            texture_index_map,
            tex_size,
        });

        // 预构建 UV 数组缓存，用于主线程网格生成的 O(1) 零分配查找
        self.build_uv_array();

        Ok(())
    }

    /// 计算 Atlas 布局（简单的 bin-packing 算法）
    fn calculate_atlas_layout(
        &self,
    ) -> Result<(u32, u32, HashMap<String, (u32, u32, u32, u32)>), String> {
        let mut placements = HashMap::new();
        let mut current_x = 0u32;
        let mut current_y = 0u32;
        let mut row_height = 0u32;
        let atlas_width = 256u32;

        let mut textures: Vec<(&String, &(Vec<u8>, u32, u32))> =
            self.texture_cache.iter().collect();
        textures.sort_by_key(|(name, _)| name.to_string());

        for (name, (_, width, height)) in textures {
            if current_x + width > atlas_width {
                current_x = 0;
                current_y += row_height;
                row_height = 0;
            }

            placements.insert(name.clone(), (current_x, current_y, *width, *height));
            current_x += width;
            row_height = row_height.max(*height);
        }

        let atlas_height = (current_y + row_height).next_power_of_two();
        let atlas_width = atlas_width.next_power_of_two();

        Ok((atlas_width, atlas_height, placements))
    }

    /// 获取方块指定面的纹理 UV 坐标（通过面名称字符串，保留兼容性）。
    pub fn get_block_uv(&self, block_id: u8, face: &str) -> Option<(f32, f32, f32, f32)> {
        let texture_name = self.block_texture_map.get(&(block_id, face.to_string()))?;
        let atlas = self.atlas.as_ref()?;
        let texture_info = atlas.textures.get(texture_name)?;
        Some(texture_info.uv)
    }

    /// 获取方块指定面的纹理 UV 坐标（通过 face index，O(1) 零分配查找）。
    ///
    /// face_index: 0=top, 1=bottom, 2=side
    /// 用于网格生成热路径，避免 HashMap 查找和 String 分配。
    #[inline]
    pub fn get_block_uv_by_index(&self, block_id: u8, face_index: usize) -> (f32, f32, f32, f32) {
        self.block_uv_array[block_id as usize][face_index].unwrap_or((0.0, 1.0, 0.0, 1.0))
    }

    /// 构建 UV 数组缓存。
    ///
    /// 遍历 `block_texture_map`，将 HashMap 查找结果预填充到 `block_uv_array` 二维数组中。
    /// 在 `build_atlas()` 完成后调用。
    fn build_uv_array(&mut self) {
        self.block_uv_array = [[None; 3]; 256];
        if let Some(atlas) = &self.atlas {
            for ((block_id, face), texture_name) in &self.block_texture_map {
                if let Some(tex_info) = atlas.textures.get(texture_name) {
                    let fi = crate::async_mesh::face_name_to_index(face);
                    self.block_uv_array[*block_id as usize][fi] = Some(tex_info.uv);
                }
            }
        }
    }

    /// 生成默认纹理（当材质包目录为空时）
    fn generate_default_textures(&self, dir: &Path) -> Result<(), String> {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;

        let default_textures: Vec<(&str, [u8; 4])> = vec![
            ("dirt", [135, 100, 60, 255]),
            ("obsidian", [20, 18, 30, 255]),
            ("glass", [200, 220, 240, 255]),
            ("bedrock", [85, 85, 85, 255]),
        ];

        for (name, color) in default_textures {
            let pixels = create_default_texture_pixels(color);
            let path = dir.join(format!("{}.png", name));
            save_rgba_as_png(&path, &pixels, 16, 16)?;
            info!("Generated default texture: {}", name);
        }

        Ok(())
    }
}

/// 加载 PNG 文件为 RGBA 像素数据
fn load_png_as_rgba(path: &Path) -> Result<(Vec<u8>, u32, u32), String> {
    let img = image::open(path).map_err(|e| e.to_string())?;
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    Ok((rgba.to_vec(), width, height))
}

/// 保存 RGBA 像素数据为 PNG 文件
fn save_rgba_as_png(path: &Path, pixels: &[u8], width: u32, height: u32) -> Result<(), String> {
    use image::ImageEncoder;
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let encoder = image::codecs::png::PngEncoder::new(file);
    encoder
        .write_image(pixels, width, height, image::ExtendedColorType::Rgba8)
        .map_err(|e| e.to_string())
}

/// 对 RGBA 像素数据应用生物群系着色
///
/// 将每个像素的 RGB 通道乘以着色颜色，模拟 Minecraft 的 biome tint 效果。
/// Alpha 通道保持不变。
fn apply_biome_tint(pixels: &mut [u8], tint: [f32; 3]) {
    for chunk in pixels.chunks_exact_mut(4) {
        chunk[0] = (chunk[0] as f32 * tint[0]).clamp(0.0, 255.0) as u8;
        chunk[1] = (chunk[1] as f32 * tint[1]).clamp(0.0, 255.0) as u8;
        chunk[2] = (chunk[2] as f32 * tint[2]).clamp(0.0, 255.0) as u8;
        // chunk[3] (alpha) 保持不变
    }
}

/// 创建默认纹理像素数据
fn create_default_texture_pixels(base_color: [u8; 4]) -> Vec<u8> {
    let mut pixels = vec![0u8; 16 * 16 * 4];
    for y in 0u32..16 {
        for x in 0u32..16 {
            let noise: i32 = ((x * 7 + y * 13) % 20) as i32 - 10;
            let r = (base_color[0] as i32 + noise).clamp(0, 255) as u8;
            let g = (base_color[1] as i32 + noise).clamp(0, 255) as u8;
            let b = (base_color[2] as i32 + noise).clamp(0, 255) as u8;
            let idx = ((y * 16 + x) * 4) as usize;
            pixels[idx] = r;
            pixels[idx + 1] = g;
            pixels[idx + 2] = b;
            pixels[idx + 3] = 255;
        }
    }
    pixels
}

/// Bevy 插件：资源包系统
pub struct ResourcePackPlugin;

impl Plugin for ResourcePackPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ResourcePackManager>();
        // 注意：load_resource_pack_system 在 main.rs 中通过 .chain() 手动注册，
        // 确保在 setup_world 之前执行
    }
}

/// 启动时加载资源包的系统（必须在 setup_world 之前运行）
pub fn load_resource_pack_system(mut manager: ResMut<ResourcePackManager>) {
    match manager.load_resource_pack() {
        Ok(()) => {
            info!("Resource pack loaded successfully");
            if let Some(atlas) = &manager.atlas {
                for (name, info) in &atlas.textures {
                    info!(
                        "  Texture '{}': UV ({:.3}, {:.3}, {:.3}, {:.3})",
                        name, info.uv.0, info.uv.1, info.uv.2, info.uv.3
                    );
                }
            }
        }
        Err(e) => {
            error!("Failed to load resource pack: {}", e);
        }
    }
}
