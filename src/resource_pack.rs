//! Resource Pack System — 动态 Atlas 构建和材质包加载
//!
//! 材质包统一存放在 `assets/resourcepacks/` 目录下，
//! 每个子文件夹即为一个材质包。可通过 `ResourcePackManager::selected_pack`
//! 指定要使用的材质包名称（子文件夹名），默认使用第一个找到的材质包。
//!
//! # 方块→纹理映射机制（JSON 驱动）
//!
//! 使用 `assets/blockstates/*.json` 定义方块属性和纹理映射：
//! ```text
//! block_id → blockstates/*.json → textures → Atlas UV
//! ```
//!
//! 每个 JSON 文件定义一种方块：
//! ```json
//! {
//!   "id": 1,
//!   "name": "grass_block",
//!   "solid": true,
//!   "transparent": false,
//!   "textures": { "top": "grass_block_top", "bottom": "dirt", "side": "grass_block_side" }
//! }
//! ```
//!
//! 加新方块只需：
//! 1. 在 `assets/blockstates/` 下创建 JSON 文件
//! 2. 在材质包目录下放入对应的 PNG 纹理
//! 3. 无需修改代码，重启即可生效

use bevy::prelude::*;
use bevy::reflect::TypePath;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::block_definition::{self, BlockDefinition};

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
    // grass_block_top: 鲜绿色（降低 G 通道）
    ("grass_block_top", [0.5, 0.7, 0.22]),
    // grass_block_side: 侧面草调整为适中的绿色
    ("grass_block_side", [0.55, 0.85, 0.6]),
    // oak_leaves: 树叶调整为适中的绿色
    ("oak_leaves", [0.45, 0.8, 0.45]),
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
    /// 半纹素偏移量（用于 UV 内缩，防止纹理出血）
    pub half_texel: f32,
}

/// Texture Array 图集（纹理按层排列）。
///
/// 渲染管线始终使用 Texture Array (2D Array) 格式，已移除传统 2D Atlas
/// 相关字段（`image`/`width`/`height`/`size`），消除 ~260KB 沉余像素副本。
#[derive(Debug)]
pub struct TextureAtlas {
    pub textures: HashMap<String, TextureInfo>,
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
    /// 半透明混合模式，用于水方块等透明物体
    pub alpha_mode: AlphaMode,
}

impl Default for VoxelMaterial {
    fn default() -> Self {
        Self {
            array_texture: Handle::default(),
            alpha_mode: AlphaMode::Opaque,
        }
    }
}

impl Material for VoxelMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/voxel.wgsl".into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        self.alpha_mode
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
    /// 方块→纹理映射表（从 `blockstates/*.json` 自动生成）
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
    /// 在 `load_resource_pack()` 中从 `block_definitions` 自动生成。
    pub block_texture_map: HashMap<(u8, String), String>,
    /// 预构建的 UV 数组缓存，用于主线程网格生成的 O(1) 零分配查找。
    ///
    /// `[block_id][face_index]` -> UV 坐标
    /// face_index: 0=top, 1=bottom, 2=side
    /// 在 `build_atlas()` 时自动构建。
    block_uv_array: [[Option<(f32, f32, f32, f32)>; 3]; 256],
    /// 从 JSON 加载的方块定义（blockstates/*.json）
    pub block_definitions: HashMap<u8, BlockDefinition>,
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
            block_texture_map: HashMap::new(),
            block_uv_array: [[None; 3]; 256],
            block_definitions: HashMap::new(),
        }
    }
}

impl ResourcePackManager {
    /// 从 JSON 方块定义生成纹理映射表
    ///
    /// 遍历 `block_definitions`，将每个方块的 textures 字段
    /// 转换为 `block_texture_map` 格式：`(block_id, face) → texture_name`。
    fn build_block_texture_map_from_definitions(&mut self) {
        self.block_texture_map.clear();
        for def in self.block_definitions.values() {
            self.block_texture_map
                .insert((def.id, "top".to_string()), def.textures.top.clone());
            self.block_texture_map
                .insert((def.id, "bottom".to_string()), def.textures.bottom.clone());
            self.block_texture_map
                .insert((def.id, "side".to_string()), def.textures.side.clone());
        }
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

        // 加载方块定义（blockstates/*.json）
        self.block_definitions = block_definition::load_block_definitions();
        self.build_block_texture_map_from_definitions();

        // 初始化全局方块属性表（用于 is_block_solid 热路径）
        let props_table =
            block_definition::BlockPropertiesTable::from_definitions(&self.block_definitions);
        block_definition::init_block_properties(props_table);

        // 扫描可用材质包
        self.scan_available_packs();

        // 确定要加载的材质包路径
        let pack_path = self.resolve_pack_path()?;

        // info!("Loading resource pack from: {:?}", pack_path);
        self.current_pack = pack_path.clone();

        match self.scan_textures(&pack_path) {
            Ok(count) => {
                // info!("Loaded {} textures from {:?}", count, pack_path);
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
            "Resource pack loaded: {} textures, tex_size={}",
            self.texture_cache.len(),
            self.atlas.as_ref().map_or(0, |a| a.tex_size)
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
                            // info!(
                            //     "  Loaded texture: {} ({}x{}) [biome tinted]",
                            //     filename, width, height
                            // );
                        } else {
                            // info!("  Loaded texture: {} ({}x{})", filename, width, height);
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

    /// 构建 Texture Array 图集（单遍：像素拷贝 + UV 编码）。
    fn build_atlas(&mut self) -> Result<(), String> {
        if self.texture_cache.is_empty() {
            return Err("No textures loaded".to_string());
        }

        let mut texture_index_map = HashMap::new();
        let mut texture_infos = HashMap::new();
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
            let layer = layer_idx as u32;
            texture_index_map.insert(name.to_string(), layer);

            if let Some((src_pixels, src_w, src_h)) = self.texture_cache.get(*name) {
                // 复制到 Texture Array 层
                for py in 0..*src_h {
                    for px in 0..*src_w {
                        let src_idx = ((py * src_w + px) * 4) as usize;
                        let dst_idx =
                            ((layer * tex_size * tex_size + py * tex_size + px) * 4) as usize;
                        if src_idx + 3 < src_pixels.len() && dst_idx + 3 < array_pixels.len() {
                            array_pixels[dst_idx..dst_idx + 4]
                                .copy_from_slice(&src_pixels[src_idx..src_idx + 4]);
                        }
                    }
                }

                // 构建 TextureInfo（UV 编码到 Texture Array 层）
                let half_texel = 0.5 / tex_size as f32;
                let layer_f = layer as f32;
                let u_min = layer_f + half_texel;
                let u_max = layer_f + 1.0 - half_texel;
                let v_min = half_texel;
                let v_max = 1.0 - half_texel;

                texture_infos.insert(
                    name.to_string(),
                    TextureInfo {
                        position: (0, 0),
                        size: (*src_w, *src_h),
                        uv: (u_min, u_max, v_min, v_max),
                        layer_index: layer,
                        half_texel,
                    },
                );
            }
        }

        self.atlas = Some(TextureAtlas {
            textures: texture_infos,
            array_pixels,
            array_layers,
            texture_index_map,
            tex_size,
        });

        // 预构建 UV 数组缓存，用于主线程网格生成的 O(1) 零分配查找
        self.build_uv_array();

        Ok(())
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
            ("water", [30, 100, 200, 180]), // 半透明蓝色水纹理
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
