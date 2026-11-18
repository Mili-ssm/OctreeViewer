
struct CellFrame {
    view_proj: mat4x4<f32>,
    num_pages: u32,
    shading: u32,
    prev: u32,
    capacity: u32,
    viewport: vec2<f32>,
    mip_count: u32,
    pad0: u32,
    color_mode: f32,
    intensity_min: f32,
    intensity_max: f32,
    height_offset: f32,
    height_span: f32,
    lod_mode: u32,
    counters: u32,
    target_px: f32,
    forward: vec3<f32>,
    focal: f32,
    right: vec3<f32>,
    min_side: f32,
    up: vec3<f32>,
    pad1: u32,
};

@group(0) @binding(1)
var<uniform> frame: CellFrame;

// The one matrix product used by the frustum tests and by the vertex stage.
fn cell_clip(position: vec3<f32>) -> vec4<f32> {
    return frame.view_proj * vec4<f32>(position, 1.0);
}

// Corner `index` (0..8) in the order of the CPU path's cube vertices.
fn cell_upper(index: u32) -> vec3<u32> {
    return vec3<u32>(((index + 1u) >> 1u) & 1u, (index >> 1u) & 1u, index >> 2u);
}

fn cell_corner(lo: vec3<f32>, hi: vec3<f32>, index: u32) -> vec3<f32> {
    return select(lo, hi, cell_upper(index) == vec3<u32>(1u));
}

// Conservative: outside only if all eight corners lie beyond the same clip plane, with a small
// relative margin.
fn cell_outside_frustum(lo: vec3<f32>, hi: vec3<f32>) -> bool {
    var all_out = 63u;
    for (var i = 0u; i < 8u; i++) {
        let q = cell_clip(cell_corner(lo, hi, i));
        let m = 1.0e-4 * abs(q.w) + 1.0e-30;
        var bits = 0u;
        if q.x < -q.w - m { bits |= 1u; }
        if q.x > q.w + m { bits |= 2u; }
        if q.y < -q.w - m { bits |= 4u; }
        if q.y > q.w + m { bits |= 8u; }
        if q.z < -m { bits |= 16u; }
        if q.z > q.w + m { bits |= 32u; }
        all_out &= bits;
    }
    return all_out != 0u;
}
