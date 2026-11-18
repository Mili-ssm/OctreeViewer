
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var dst: texture_storage_2d<r32float, write>;
@compute @workgroup_size(8, 8)
fn reduce_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(dst);
    if id.x >= size.x || id.y >= size.y { return; }
    let src_last = vec2<i32>(textureDimensions(src)) - vec2<i32>(1);
    let dst_last = vec2<i32>(size) - vec2<i32>(1);
    let p = vec2<i32>(id.xy);
    let first = p * 2;
    let end = select(first + vec2<i32>(1), src_last, p == dst_last);
    var m = 0.0;
    for (var y = first.y; y <= end.y; y++) {
        for (var x = first.x; x <= end.x; x++) {
            m = max(m, textureLoad(src, vec2<i32>(x, y), 0).r);
        }
    }
    textureStore(dst, p, vec4<f32>(m, 0.0, 0.0, 0.0));
}
