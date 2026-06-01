//! 天空盒模块：加载 PNG 立方体贴图并绑定到相机 Skybox 组件。
//!
//! PNG 立方体贴图是一张垂直堆叠的 2D 纹理（6 个面），
//! 需要手动重解释为 Cube 纹理视图。
//!
//! 基于 Bevy 官方 skybox demo 简化而来。

use bevy::{
    core_pipeline::Skybox,
    prelude::*,
    render::render_resource::{TextureViewDescriptor, TextureViewDimension},
};

/// 天空盒纹理路径（换纹理只改这里）
pub const SKYBOX_PATH: &str = "skybox/sky01-1.png";

/// 天空盒加载状态
#[derive(Resource)]
pub struct Cubemap {
    is_loaded: bool,
    image_handle: Handle<Image>,
}

/// 启动系统：加载天空盒纹理，保存句柄到资源
pub fn setup_skybox(mut commands: Commands, asset_server: Res<AssetServer>) {
    let skybox_handle = asset_server.load(SKYBOX_PATH);
    commands.insert_resource(Cubemap {
        is_loaded: false,
        image_handle: skybox_handle,
    });
}

/// 等待纹理加载完成后，将 PNG 重解释为 CubeMap 并更新 Skybox 组件
pub fn asset_loaded(
    asset_server: Res<AssetServer>,
    mut images: ResMut<Assets<Image>>,
    mut cubemap: ResMut<Cubemap>,
    mut skyboxes: Query<&mut Skybox>,
) {
    if cubemap.is_loaded {
        return;
    }
    if !asset_server.load_state(&cubemap.image_handle).is_loaded() {
        return;
    }

    let image = images.get_mut(&cubemap.image_handle).unwrap();
    // PNG 不包含立方体贴图元数据，需手动重解释为数组纹理
    if image.texture_descriptor.array_layer_count() == 1 {
        image
            .reinterpret_stacked_2d_as_array(image.height() / image.width())
            .expect("天空盒 PNG 应为正方形，高度应能被 6 整除");
        image.texture_view_descriptor = Some(TextureViewDescriptor {
            dimension: Some(TextureViewDimension::Cube),
            ..default()
        });
    }

    for mut skybox in &mut skyboxes {
        skybox.image = cubemap.image_handle.clone();
    }

    cubemap.is_loaded = true;
}
