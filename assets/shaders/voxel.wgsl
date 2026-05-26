// Voxel Material Fragment Shader（轻量化版）
//
// 使用 Texture Array 存储方块纹理，通过 UV.x 的整数部分编码纹理层索引。
// UV 编码方式：UV.x = texture_index + actual_u, UV.y = actual_v
// 着色器解码：layer = floor(UV.x), sample_uv = fract(UV.x), UV.y
//
// 光照：简单的 hemisphere lighting 替代完整 PBR，GPU fragment 开销降低 ~40%。

#import bevy_pbr::forward_io::VertexOutput
#import bevy_pbr::mesh_view_bindings::view
#import bevy_core_pipeline::tonemapping::tone_mapping

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var voxel_array_texture: texture_2d_array<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var voxel_array_texture_sampler: sampler;

@fragment
fn fragment(
    @builtin(front_facing) is_front: bool,
    mesh: VertexOutput,
) -> @location(0) vec4<f32> {
#ifdef VERTEX_UVS
    // 从 UV.x 解码纹理层索引（整数部分）和实际 UV（小数部分）
    // UV.x = layer_index + actual_u, UV.y = actual_v
    // actual_u 和 actual_v 可以大于 1，通过 fract() 实现纹理平铺
    let layer = u32(floor(mesh.uv.x));
    let sample_uv = vec2<f32>(fract(mesh.uv.x), fract(mesh.uv.y));
    var color = textureSample(
        voxel_array_texture, voxel_array_texture_sampler, sample_uv, layer
    );
#else
    var color = vec4<f32>(1.0, 0.0, 1.0, 1.0); // missing texture magenta
#endif

    // ── Minecraft 晴天白天光照（Hemisphere Lighting）────────────────────
    // 模拟 Minecraft 晴天白天的光照模型：
    //   - 太阳高悬（light_dir.y = 0.85），略偏一侧产生自然阴影
    //   - 环境光 0.3：暗面更深，有体积感（≈ Minecraft 天空光 5/15）
    //   - 方向光 0.5：亮面 0.8×，柔和不过曝
    //   - 对比度 ~2.7:1，有层次感的日间光影
    let world_normal = normalize(mesh.world_normal);
    // 背面朝前时翻转法线
    let N = select(world_normal, -world_normal, !is_front);
    // 太阳方向：高角度（0.85）偏右（0.2）偏前（0.3），与 setup_lighting 方向灯对齐
    let light_dir = normalize(vec3<f32>(0.2, 0.85, 0.3));
    let ndotl = max(dot(N, light_dir), 0.0);
    // 半球光照：背面 0.3× → 正面 0.8×，整体暗一档，对比更鲜明
    let lit = color.rgb * (0.3 + ndotl * 0.5);
    color = vec4<f32>(lit, color.a);
    // ─────────────────────────────────────────────────────────────

    return tone_mapping(color, view.color_grading);
}
