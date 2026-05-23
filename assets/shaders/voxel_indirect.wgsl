// assets/shaders/voxel_indirect.wgsl
//
// MultiDrawIndirect 渲染着色器
//
// 从全局Storage Buffer读取顶点数据，支持一次Draw Call渲染多个区块。
// 使用 DrawIndexedIndirect 命令，每个区块作为独立的"实例"渲染。
//
// BindGroup(0):
//   b0: vertex_buffer   (storage/read)  - PackedVertex 数组
//   b1: index_buffer    (storage/read)  - u32 索引数组
//   b2: chunk_offsets   (storage/read)  - vec4<f32> 数组（xyz=世界坐标偏移）
//   b3: view_uniform    (uniform)       - ViewUniform（view_proj 矩阵）

// ============================================================================
// 数据结构定义
// ============================================================================

// 顶点数据（打包格式）
// 位置（3 floats）+ 法线编码（1 float）+ UV（2 floats）+ 额外数据（2 floats）
struct PackedVertex {
    position: vec3<f32>,
    normal_encoded: f32,
    uv: vec2<f32>,
    extra: vec2<f32>,
}

// 视图 Uniform（与 Rust 端 ViewUniformRaw 对齐）
struct ViewUniform {
    view_proj: mat4x4<f32>,
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
var<uniform> view: ViewUniform;

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
    output.color = vec3<f32>(lighting, lighting, lighting);
    
    return output;
}

// ============================================================================
// 片段着色器
// ============================================================================

@fragment
fn fragment_debug(
    input: VertexOutput,
) -> @location(0) vec4<f32> {
    // 直接使用光照颜色（纯色渲染，无纹理）
    return vec4<f32>(input.color, 1.0);
}
