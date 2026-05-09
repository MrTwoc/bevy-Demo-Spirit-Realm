// Voxel Material Fragment Shader
//
// 使用 Texture Array 存储方块纹理，通过 UV.x 的整数部分编码纹理层索引。
// UV 编码方式：UV.x = texture_index + actual_u, UV.y = actual_v
// 着色器解码：layer = floor(UV.x), sample_uv = fract(UV.x), UV.y

#import bevy_pbr::{
    forward_io::VertexOutput,
    mesh_view_bindings::view,
    pbr_types::{STANDARD_MATERIAL_FLAGS_DOUBLE_SIDED_BIT, PbrInput, pbr_input_new},
    pbr_functions as fns,
}
#import bevy_core_pipeline::tonemapping::tone_mapping

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var voxel_array_texture: texture_2d_array<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var voxel_array_texture_sampler: sampler;

@fragment
fn fragment(
    @builtin(front_facing) is_front: bool,
    mesh: VertexOutput,
) -> @location(0) vec4<f32> {
    // 从 UV.x 解码纹理层索引（整数部分）和实际 UV（小数部分）
    let layer = u32(floor(mesh.uv.x));
    let sample_uv = vec2<f32>(fract(mesh.uv.x), mesh.uv.y);

    var pbr_input: PbrInput = pbr_input_new();
    pbr_input.material.base_color = textureSample(
        voxel_array_texture, voxel_array_texture_sampler, sample_uv, layer
    );

    let double_sided = (pbr_input.material.flags & STANDARD_MATERIAL_FLAGS_DOUBLE_SIDED_BIT) != 0u;
    pbr_input.frag_coord = mesh.position;
    pbr_input.world_position = mesh.world_position;
    pbr_input.world_normal = fns::prepare_world_normal(
        mesh.world_normal, double_sided, is_front,
    );
    pbr_input.is_orthographic = view.clip_from_view[3].w == 1.0;
    pbr_input.N = normalize(pbr_input.world_normal);
    pbr_input.V = fns::calculate_view(mesh.world_position, pbr_input.is_orthographic);

    return tone_mapping(fns::apply_pbr_lighting(pbr_input), view.color_grading);
}
