// assets/shaders/voxel_indirect.wgsl
//
// MultiDrawIndirect 渲染着色器
//
// 从全局Storage Buffer读取顶点数据，支持一次Draw Call渲染多个区块。
// 使用 DrawIndexedIndirect 命令，每个区块作为独立的"实例"渲染。

// ============================================================================
// 数据结构定义
// ============================================================================

// 区块元数据
struct ChunkMetadata {
    position: vec3<f32>,  // 区块世界坐标
    lod_level: f32,       // LOD级别（用于调试着色）
}

// 顶点数据（打包格式）
// 位置（3 floats）+ 法线编码（1 float）+ UV（2 floats）+ 额外数据（1 float）
struct PackedVertex {
    position: vec3<f32>,
    normal_encoded: f32,
    uv: vec2<f32>,
    extra: vec2<f32>,
}

// ============================================================================
// 绑定组定义
// ============================================================================

@group(0) @binding(0)
var<storage, read> vertex_buffer: array<PackedVertex>;

@group(0) @binding(1)
var<storage, read> index_buffer: array<u32>;

@group(0) @binding(2)
var<storage, read> chunk_offsets: array<vec4<f32>>;

@group(0) @binding(3)
var<storage, read> chunk_metadata: array<ChunkMetadata>;

// Bevy的标准绑定（View矩阵等）
#import bevy_render::view::View
@group(0) @binding(100)
var<uniform> view: View;

// ============================================================================
// 辅助函数
// ============================================================================

// 解码法线方向
// 使用简单的6方向编码（与CPU端对齐）
fn decode_normal(encoded: f32) -> vec3<f32> {
    let face_type = u32(encoded);
    switch(face_type) {
        case 0u: { return vec3<f32>(1.0, 0.0, 0.0); }  // +X (Right)
        case 1u: { return vec3<f32>(-1.0, 0.0, 0.0); } // -X (Left)
        case 2u: { return vec3<f32>(0.0, 1.0, 0.0); }  // +Y (Top)
        case 3u: { return vec3<f32>(0.0, -1.0, 0.0); } // -Y (Bottom)
        case 4u: { return vec3<f32>(0.0, 0.0, 1.0); }  // +Z (Front)
        case 5u: { return vec3<f32>(0.0, 0.0, -1.0); } // -Z (Back)
        default: { return vec3<f32>(0.0, 1.0, 0.0); }  // 默认向上
    }
}

// 简单的Lambertian光照计算
fn calculate_lighting(normal: vec3<f32>) -> f32 {
    // 主光源方向（太阳光）
    let sun_direction = normalize(vec3<f32>(0.5, 0.8, 0.3));
    let sun_intensity = max(dot(normal, sun_direction), 0.0);
    
    // 环境光
    let ambient = 0.3;
    
    return ambient + sun_intensity * 0.7;
}

// LOD调试着色（可选）
fn lod_debug_color(lod_level: f32) -> vec3<f32> {
    switch(u32(lod_level)) {
        case 0u: { return vec3<f32>(1.0, 1.0, 1.0); } // LOD0: 白色
        case 1u: { return vec3<f32>(0.0, 1.0, 0.0); } // LOD1: 绿色
        case 2u: { return vec3<f32>(1.0, 1.0, 0.0); } // LOD2: 黄色
        case 3u: { return vec3<f32>(1.0, 0.0, 0.0); } // LOD3: 红色
        default: { return vec3<f32>(1.0, 1.0, 1.0); }
    }
}

// ============================================================================
// 顶点着色器
// ============================================================================

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec3<f32>,
};

@vertex
fn vertex(
    @builtin(instance_index) instance_id: u32,
    @builtin(vertex_index) vertex_id: u32,
) -> VertexOutput {
    // 获取区块偏移
    let chunk_offset = chunk_offsets[instance_id].xyz;
    
    // 获取区块元数据
    let metadata = chunk_metadata[instance_id];
    
    // 从全局Buffer读取顶点数据
    let vertex_data = vertex_buffer[vertex_id];
    
    // 解码顶点属性
    let local_position = vertex_data.position;
    let normal = decode_normal(vertex_data.normal_encoded);
    let uv = vertex_data.uv;
    
    // 变换到世界空间
    let world_position = local_position + chunk_offset;
    
    // 变换到裁剪空间
    let clip_position = view.view_proj * vec4<f32>(world_position, 1.0);
    
    // 计算光照
    let lighting = calculate_lighting(normal);
    
    // 输出
    var output: VertexOutput;
    output.clip_position = clip_position;
    output.world_position = world_position;
    output.world_normal = normal;
    output.uv = uv;
    
    // 基础颜色（白色 × 光照）
    // 可选：使用LOD调试颜色
    // output.color = lod_debug_color(metadata.lod_level) * lighting;
    output.color = vec3<f32>(lighting, lighting, lighting);
    
    return output;
}

// ============================================================================
// 片段着色器
// ============================================================================

// Texture Array 绑定（用于实际纹理渲染）
@group(1) @binding(0)
var voxel_texture: texture_2d_array<f32>;
@group(1) @binding(1)
var voxel_sampler: sampler;

@fragment
fn fragment(
    @builtin(front_facing) is_front: bool,
    input: VertexOutput,
) -> @location(0) vec4<f32> {
    // 从UV.x解码纹理层索引（整数部分）和实际UV（小数部分）
    let layer = u32(floor(input.uv.x));
    let sample_uv = vec2<f32>(fract(input.uv.x), input.uv.y);
    
    // 从Texture Array采样
    let texture_color = textureSample(voxel_texture, voxel_sampler, sample_uv, layer);
    
    // 应用光照
    let final_color = texture_color.rgb * input.color;
    
    return vec4<f32>(final_color, texture_color.a);
}

// 简化版片段着色器（无纹理，用于调试）
@fragment
fn fragment_debug(
    @builtin(front_facing) is_front: bool,
    input: VertexOutput,
) -> @location(0) vec4<f32> {
    // 直接使用光照颜色
    return vec4<f32>(input.color, 1.0);
}

// 线框渲染模式（用于调试）
@fragment
fn fragment_wireframe(
    @builtin(front_facing) is_front: bool,
    input: VertexOutput,
) -> @location(0) vec4<f32> {
    // 返回固定的线框颜色
    return vec4<f32>(0.0, 1.0, 0.0, 1.0);
}
