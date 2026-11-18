
struct CameraUniform {
    view_proj: mat4x4<f32>,
    render_origin: vec4<f32>,
    style: vec4<f32>,
    intensity_range: vec4<f32>,
};
struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) depth: f32,
};
@group(0) @binding(0) var<uniform> camera: CameraUniform;
@group(1) @binding(0) var<storage, read> source_words: array<u32>;
@group(1) @binding(1) var<storage, read> visible_indices: array<u32>;

fn word_float(index: u32) -> f32 { return bitcast<f32>(source_words[index]); }

@vertex
fn vs_indexed(
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @builtin(instance_index) instance_index: u32,
) -> VertexOutput {
    let base = visible_indices[instance_index] * 10u;
    let center = vec3<f32>(word_float(base), word_float(base + 1u), word_float(base + 2u));
    let size = word_float(base + 3u);
    let color = vec4<f32>(word_float(base + 4u), word_float(base + 5u), word_float(base + 6u), word_float(base + 7u));
    let intensity = word_float(base + 8u);
    let classification = word_float(base + 9u);
    let local_position = center * camera.render_origin.w - camera.render_origin.xyz
        + position * size * camera.style.y * camera.render_origin.w;
    var out: VertexOutput;
    out.clip_position = camera.view_proj * vec4<f32>(local_position, 1.0);
    out.depth = out.clip_position.z / out.clip_position.w;
    out.normal = normal;
    let rgb_intensity = dot(color.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
    var final_color = color;
    if camera.style.x == 1.0 {
        let intensity_span = camera.intensity_range.y - camera.intensity_range.x;
        let scaled_intensity = clamp((intensity - camera.intensity_range.x) / max(intensity_span, 0.000001), 0.0, 1.0);
        let normalized = select(rgb_intensity, select(0.5, scaled_intensity, intensity_span > 0.000001), intensity >= 0.0);
        final_color = vec4<f32>(vec3<f32>(normalized), 1.0);
    } else if camera.style.x == 2.0 {
        let height = clamp((center.z - camera.style.z) / max(camera.style.w - camera.style.z, 0.000001), 0.0, 1.0);
        final_color = vec4<f32>(mix(vec3<f32>(0.08, 0.25, 0.9), vec3<f32>(0.95, 0.22, 0.08), height), 1.0);
    } else if camera.style.x == 3.0 && classification >= 0.0 {
        let hue = fract(classification * 0.61803398875);
        let rgb = clamp(abs(fract(hue + vec3<f32>(0.0, 0.6666667, 0.3333333)) * 6.0 - 3.0) - 1.0, vec3<f32>(0.0), vec3<f32>(1.0));
        final_color = vec4<f32>(rgb, 1.0);
    }
    out.color = final_color;
    return out;
}

@fragment
fn fs_indexed(input: VertexOutput) -> @location(0) vec4<f32> {
    if camera.intensity_range.z > 0.5 {
        let light_direction = normalize(vec3<f32>(-0.35, -0.45, 0.82));
        let face_light = 0.68 + 0.32 * max(dot(normalize(input.normal), light_direction), 0.0);
        let depth_light = 1.0 - 0.12 * clamp(input.depth, 0.0, 1.0);
        return vec4<f32>(input.color.rgb * face_light * depth_light, input.color.a);
    }
    return input.color;
}
