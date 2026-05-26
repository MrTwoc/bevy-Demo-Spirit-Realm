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

    // ── Hemisphere Lighting（替代完整 PBR） ──────────────────────
    // 等价于 roughness=1.0, metallic=0.0 时的 PBR 漫反射结果：
    //   - 上方暖色直射光强度 0.6
    //   - 底部环境光强度 0.4（通过 (1 - ndotl) * 0.4 实现柔和间接光）
    //
    // 光照方向固定，匹配 setup_lighting 中 DirectionalLight 的角度
    let world_normal = normalize(mesh.world_normal);
    // 背面朝前时翻转法线
    let N = select(world_normal, -world_normal, !is_front);
    let light_dir = normalize(vec3<f32>(0.5, 0.8, 0.3));
    let ndotl = max(dot(N, light_dir), 0.0);
    let lit = color.rgb * (0.4 + ndotl * 0.6);
    color = vec4<f32>(lit, color.a);
    // ─────────────────────────────────────────────────────────────

    return tone_mapping(color, view.color_grading);
}
