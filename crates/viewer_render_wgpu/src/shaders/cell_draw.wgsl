
@group(1) @binding(0) var<storage, read> inst: array<vec4<u32>>;
@group(1) @binding(1) var<storage, read> pages: array<CellPage>;
@group(1) @binding(2) var<storage, read> visible: array<u32>;

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) depth: f32,
};

@vertex
fn vs_main(
    @builtin(vertex_index) corner: u32,
    @builtin(instance_index) instance: u32,
) -> VertexOutput {
    let slot = visible[instance];
    let page = pages[slot / 512u];
    let cell = inst[slot];
    let position = cell_corner(cell_lo(page, cell), cell_hi(page, cell), corner);
    var out: VertexOutput;
    out.clip_position = cell_clip(position);
    out.depth = out.clip_position.z / out.clip_position.w;
    out.normal = vec3<f32>(cell_upper(corner)) * 2.0 - vec3<f32>(1.0);
    let word = cell_intensity(cell);
    let intensity = select(f32(word), -1.0, word == 0xffffu);
    let lo = cell_lo(page, cell);
    let hi = cell_hi(page, cell);
    out.color = shader_mode_color(
        frame.color_mode,
        vec4<f32>(cell_linear_rgb(cell), 1.0),
        intensity,
        vec2<f32>(frame.intensity_min, frame.intensity_max),
        f32(cell_class(cell)),
        (lo.z + hi.z) * 0.5 + frame.height_offset,
        frame.height_span,
    ).rgb;
    return out;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    if frame.shading != 0u {
        let light_direction = normalize(vec3<f32>(-0.35, -0.45, 0.82));
        let face_light = 0.68 + 0.32 * max(dot(normalize(input.normal), light_direction), 0.0);
        let depth_light = 1.0 - 0.12 * clamp(input.depth, 0.0, 1.0);
        return vec4<f32>(input.color * face_light * depth_light, 1.0);
    }
    return vec4<f32>(input.color, 1.0);
}
