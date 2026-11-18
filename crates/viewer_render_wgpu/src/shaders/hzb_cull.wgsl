
struct CameraUniform {
    view_proj: mat4x4<f32>,
    render_origin: vec4<f32>,
    style: vec4<f32>,
    intensity_range: vec4<f32>,
};
struct DrawArgs {
    index_count: u32,
    instance_count: atomic<u32>,
    first_index: u32,
    base_vertex: i32,
    first_instance: u32,
};
struct CullParams {
    viewport: vec2<u32>,
    pyramid_size: vec2<u32>,
    instance_count: u32,
    mip_count: u32,
    depth_epsilon: f32,
    padding: u32,
};
@group(0) @binding(0) var<uniform> camera: CameraUniform;
@group(1) @binding(0) var<storage, read> source_words: array<u32>;
@group(1) @binding(1) var hzb: texture_2d<f32>;
@group(1) @binding(2) var<storage, read_write> visible_indices: array<u32>;
@group(1) @binding(3) var<storage, read_write> draw_args: DrawArgs;
@group(1) @binding(4) var<uniform> params: CullParams;

fn word_float(index: u32) -> f32 { return bitcast<f32>(source_words[index]); }

fn is_occluded(instance_id: u32) -> bool {
    let base = instance_id * 10u;
    let center = vec3<f32>(word_float(base), word_float(base + 1u), word_float(base + 2u));
    let size = word_float(base + 3u) * camera.style.y * camera.render_origin.w;
    let scale = camera.render_origin.w;
    var rect_min = vec2<f32>(1.0e30);
    var rect_max = vec2<f32>(-1.0e30);
    var nearest_depth = 1.0;
    for (var corner_index = 0u; corner_index < 8u; corner_index += 1u) {
        let signs = vec3<f32>(
            select(-1.0, 1.0, (corner_index & 1u) != 0u),
            select(-1.0, 1.0, (corner_index & 2u) != 0u),
            select(-1.0, 1.0, (corner_index & 4u) != 0u),
        );
        let local = center * scale - camera.render_origin.xyz + signs * (size * 0.5);
        let clip = camera.view_proj * vec4<f32>(local, 1.0);
        if clip.w <= 0.000001 || clip.z < 0.0 || clip.z > clip.w { return false; }
        let ndc = clip.xy / clip.w;
        if any(ndc != ndc) || any(abs(ndc) > vec2<f32>(1.0e20)) { return false; }
        let pixel = vec2<f32>(
            (ndc.x * 0.5 + 0.5) * f32(params.viewport.x),
            (0.5 - ndc.y * 0.5) * f32(params.viewport.y),
        );
        rect_min = min(rect_min, pixel);
        rect_max = max(rect_max, pixel);
        nearest_depth = min(nearest_depth, clip.z / clip.w);
    }
    if rect_max.x < 0.0 || rect_max.y < 0.0
        || rect_min.x >= f32(params.viewport.x) || rect_min.y >= f32(params.viewport.y) {
        return true;
    }
    let pixel_min = clamp(rect_min - vec2<f32>(1.0), vec2<f32>(0.0), vec2<f32>(params.viewport) - 1.0);
    let pixel_max = clamp(rect_max + vec2<f32>(1.0), vec2<f32>(0.0), vec2<f32>(params.viewport) - 1.0);
    let span = max(pixel_max - pixel_min + vec2<f32>(1.0), vec2<f32>(1.0));
    let mip = min(u32(ceil(log2(max(span.x, span.y)))), params.mip_count - 1u);
    let mip_scale = f32(1u << mip);
    // The pyramid holds depth 1:1 with pixels (padding is far depth), so no rescaling.
    let sample_min = vec2<i32>(floor(pixel_min / mip_scale));
    let sample_max = vec2<i32>(floor(pixel_max / mip_scale));
    if any(sample_max - sample_min > vec2<i32>(3)) { return false; }
    let mip_size = vec2<i32>(textureDimensions(hzb, i32(mip)));
    var farthest_occluder = 0.0;
    for (var y = sample_min.y; y <= sample_max.y; y += 1) {
        for (var x = sample_min.x; x <= sample_max.x; x += 1) {
            let coord = clamp(vec2<i32>(x, y), vec2<i32>(0), mip_size - 1);
            farthest_occluder = max(farthest_occluder, textureLoad(hzb, coord, i32(mip)).r);
        }
    }
    return nearest_depth > farthest_occluder + params.depth_epsilon;
}

@compute @workgroup_size(64, 1, 1)
fn cull_instances(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= params.instance_count { return; }
    if is_occluded(id.x) { return; }
    let destination = atomicAdd(&draw_args.instance_count, 1u);
    if destination < params.instance_count { visible_indices[destination] = id.x; }
}
