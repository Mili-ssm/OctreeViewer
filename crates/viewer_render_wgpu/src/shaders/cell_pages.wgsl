
struct CellPage {
    origin: vec3<f32>,
    side: f32,
    node: u32,
    octants: u32,
    box_xy: u32,
    box_z_flags: u32,
    rel: vec3<u32>,
    depth: u32,
};

@group(0) @binding(0)
var<uniform> cell_srgb_to_linear: array<vec4<f32>, 64>;

fn cell_is_empty(cell: vec4<u32>) -> bool {
    return cell.x == 0xffffffffu && cell.y == 0xffffffffu;
}

fn cell_rel(cell: vec4<u32>) -> vec3<u32> {
    return vec3<u32>(cell.x & 0xffffu, cell.x >> 16u, cell.y & 0xffffu);
}

fn cell_flags(cell: vec4<u32>) -> u32 {
    return cell.y >> 16u;
}

fn cell_mask_index(cell: vec4<u32>) -> u32 {
    return cell_flags(cell) & 0x1ffu;
}

fn cell_spans_octants(cell: vec4<u32>) -> bool {
    return (cell_flags(cell) & 0x200u) != 0u;
}

fn cell_octant(cell: vec4<u32>) -> u32 {
    let index = cell_mask_index(cell);
    return 4u * ((index & 7u) >> 2u) + 2u * (((index >> 3u) & 7u) >> 2u) + (index >> 8u);
}

fn cell_intensity(cell: vec4<u32>) -> u32 {
    return cell.w & 0xffffu;
}

fn cell_class(cell: vec4<u32>) -> i32 {
    return bitcast<i32>(cell.w) >> 16u;
}

fn cell_lo(page: CellPage, cell: vec4<u32>) -> vec3<f32> {
    return fma(vec3<f32>(cell_rel(cell)), vec3<f32>(page.side), page.origin);
}

fn cell_hi(page: CellPage, cell: vec4<u32>) -> vec3<f32> {
    return fma(vec3<f32>(cell_rel(cell)) + vec3<f32>(1.0), vec3<f32>(page.side), page.origin);
}

fn cell_srgb_channel(byte: u32) -> f32 {
    return cell_srgb_to_linear[byte >> 2u][byte & 3u];
}

fn cell_linear_rgb(cell: vec4<u32>) -> vec3<f32> {
    return vec3<f32>(
        cell_srgb_channel(cell.z & 0xffu),
        cell_srgb_channel((cell.z >> 8u) & 0xffu),
        cell_srgb_channel((cell.z >> 16u) & 0xffu),
    );
}
