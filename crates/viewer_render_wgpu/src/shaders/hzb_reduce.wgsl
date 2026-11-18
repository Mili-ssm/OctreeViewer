
@group(0) @binding(0) var source_depth: texture_depth_2d;
@group(0) @binding(1) var depth_output: texture_storage_2d<r32float, write>;

@compute @workgroup_size(8, 8, 1)
fn copy_depth(@builtin(global_invocation_id) id: vec3<u32>) {
    let output_size = textureDimensions(depth_output);
    if id.x >= output_size.x || id.y >= output_size.y { return; }
    let source_size = textureDimensions(source_depth);
    var depth = 1.0;
    if id.x < source_size.x && id.y < source_size.y {
        depth = textureLoad(source_depth, vec2<i32>(id.xy), 0);
    }
    textureStore(depth_output, vec2<i32>(id.xy), vec4<f32>(depth, 0.0, 0.0, 0.0));
}

@group(0) @binding(2) var source_level: texture_2d<f32>;
@group(0) @binding(3) var reduced_output: texture_storage_2d<r32float, write>;

@compute @workgroup_size(8, 8, 1)
fn reduce_depth(@builtin(global_invocation_id) id: vec3<u32>) {
    let output_size = textureDimensions(reduced_output);
    if id.x >= output_size.x || id.y >= output_size.y { return; }
    let source_size = textureDimensions(source_level);
    let base = id.xy * 2u;
    let d0 = textureLoad(source_level, vec2<i32>(min(base, source_size - 1u)), 0).r;
    let d1 = textureLoad(source_level, vec2<i32>(min(base + vec2<u32>(1, 0), source_size - 1u)), 0).r;
    let d2 = textureLoad(source_level, vec2<i32>(min(base + vec2<u32>(0, 1), source_size - 1u)), 0).r;
    let d3 = textureLoad(source_level, vec2<i32>(min(base + vec2<u32>(1, 1), source_size - 1u)), 0).r;
    textureStore(reduced_output, vec2<i32>(id.xy), vec4<f32>(max(max(d0, d1), max(d2, d3)), 0.0, 0.0, 0.0));
}
