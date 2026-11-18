
@group(0) @binding(0) var src: texture_depth_2d;
@group(0) @binding(1) var dst: texture_storage_2d<r32float, write>;
@compute @workgroup_size(8, 8)
fn copy_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(dst);
    if id.x >= size.x || id.y >= size.y { return; }
    textureStore(dst, vec2<i32>(id.xy), vec4<f32>(textureLoad(src, vec2<i32>(id.xy), 0), 0.0, 0.0, 0.0));
}
