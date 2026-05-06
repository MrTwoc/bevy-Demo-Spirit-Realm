//! Resource Pack System — 动态 Atlas 构建和材质包加载

use bevy::prelude::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// 纹理在 Atlas 中的位置信息
#[derive(Debug, Clone)]
pub struct TextureInfo {
    pub position: (u32, u32),
    pub size: (u32, u32),
    pub uv: (f32, f32, f32, f32),
}

/// 动态 Atlas 图集
#[derive(Debug)]
pub struct TextureAtlas {
    pub image: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub textures: HashMap<String, TextureInfo>,
    pub size: (u32, u32),
}

/// 资源包管理器
#[derive(Resource)]
pub struct ResourcePackManager {
    pub current_pack: PathBuf,
    pub texture_cache: HashMap<String, (Vec<u8>, u32, u32)>, // (pixels, width, height)
    pub atlas: Option<TextureAtlas>,
    pub block_texture_map: HashMap<(u8, String), String>,
}

impl Default for ResourcePackManager {
    fn default() -> Self {
        Self {
            current_pack: PathBuf::from("assets/1号材质包"),
            texture_cache: HashMap::new(),
            atlas: None,
            block_texture_map: Self::default_block_texture_map(),
        }
    }
}

impl ResourcePackManager {
    /// 默认的方块纹理映射表
    fn default_block_texture_map() -> HashMap<(u8, String), String> {
        let mut map = HashMap::new();

        // 草方块 (block_id = 1) — 用 dirt 替代
        map.insert((1, "top".to_string()), "dirt".to_string());
        map.insert((1, "bottom".to_string()), "dirt".to_string());
        map.insert((1, "side".to_string()), "dirt".to_string());

        // 石头 (block_id = 2) — 用 obsidian 替代
        map.insert((2, "top".to_string()), "obsidian".to_string());
        map.insert((2, "bottom".to_string()), "obsidian".to_string());
        map.insert((2, "side".to_string()), "obsidian".to_string());

        // 泥土 (block_id = 3)
        map.insert((3, "top".to_string()), "dirt".to_string());
        map.insert((3, "bottom".to_string()), "dirt".to_string());
        map.insert((3, "side".to_string()), "dirt".to_string());

        // 沙子 (block_id = 4) — 用 glass 替代
        map.insert((4, "top".to_string()), "glass".to_string());
        map.insert((4, "bottom".to_string()), "glass".to_string());
        map.insert((4, "side".to_string()), "glass".to_string());

        map
    }

    /// 扫描材质包目录，加载所有 PNG 纹理
    pub fn load_resource_pack(&mut self) -> Result<(), String> {
        let search_paths = vec![
            PathBuf::from("assets/1号材质包"),
            PathBuf::from("assets/resourcepacks/default/assets/minecraft/textures/block"),
            PathBuf::from("assets/textures"),
        ];

        let mut loaded = false;
        for path in &search_paths {
            if path.exists() {
                info!("Found resource pack at: {:?}", path);
                self.current_pack = path.clone();
                match self.scan_textures(path) {
                    Ok(count) => {
                        info!("Loaded {} textures from {:?}", count, path);
                        loaded = true;
                        break;
                    }
                    Err(e) => {
                        warn!("Failed to load from {:?}: {}", path, e);
                    }
                }
            }
        }

        if !loaded {
            warn!("No resource pack found, generating default textures...");
            let default_dir = PathBuf::from("assets/1号材质包");
            self.generate_default_textures(&default_dir)?;
            self.scan_textures(&default_dir)?;
        }

        self.build_atlas()?;

        info!(
            "Resource pack loaded: {} textures, atlas size {:?}",
            self.texture_cache.len(),
            self.atlas.as_ref().map(|a| a.size)
        );
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
                    Ok((pixels, width, height)) => {
                        info!("  Loaded texture: {} ({}x{})", filename, width, height);
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

    /// 构建动态 Atlas 图集
    fn build_atlas(&mut self) -> Result<(), String> {
        if self.texture_cache.is_empty() {
            return Err("No textures loaded".to_string());
        }

        let (atlas_width, atlas_height, placements) = self.calculate_atlas_layout()?;

        // 创建 Atlas 图像（RGBA 像素数据）
        let mut atlas_pixels = vec![0u8; (atlas_width * atlas_height * 4) as usize];
        let mut texture_infos = HashMap::new();

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

                let u_min = *x as f32 / atlas_width as f32;
                let u_max = (*x + *src_w) as f32 / atlas_width as f32;
                let v_min = *y as f32 / atlas_height as f32;
                let v_max = (*y + *src_h) as f32 / atlas_height as f32;

                texture_infos.insert(
                    texture_name.clone(),
                    TextureInfo {
                        position: (*x, *y),
                        size: (*src_w, *src_h),
                        uv: (u_min, u_max, v_min, v_max),
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
        });

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

    /// 获取方块指定面的纹理 UV 坐标
    pub fn get_block_uv(&self, block_id: u8, face: &str) -> Option<(f32, f32, f32, f32)> {
        let texture_name = self.block_texture_map.get(&(block_id, face.to_string()))?;
        let atlas = self.atlas.as_ref()?;
        let texture_info = atlas.textures.get(texture_name)?;
        Some(texture_info.uv)
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
