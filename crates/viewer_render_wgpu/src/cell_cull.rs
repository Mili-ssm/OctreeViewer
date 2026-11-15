//! Two-phase, per-cell Hi-Z occlusion for the GPU cell pool (R3 Task 7b2, `kpc` of Task 16).
//!
//! Per frame (Haar and Aaltonen, 2015):
//! 1. phase 1 compacts and draws what was visible last frame (page bit and slot bit of the `prev`
//!    half of [`visibility bits`](bits_bytes)) and lies in the frustum;
//! 2. the depth of phase 1 is reduced to a Hi-Z pyramid ([`HiZ`]);
//! 3. phase 2 tests every candidate against it (page pass: the tight box of the page; slot pass:
//!    each cell), writes the `next` bits and draws the cells that pass and that phase 1 did not
//!    draw, with the same predicate phase 1 used.
//!
//! Nothing here can make an image wrong through stale bits: phase 1 and phase 2 read the same
//! `prev` half, so a cell is skipped in phase 2 only if phase 1 drew it. Stale bits only cost
//! efficiency, which is why they are cleared whenever a page slot is uploaded, freed or deactivated.
//!
//! # Hi-Z
//! The pyramid is our own. Mip 0 has the size of the viewport and every texel of mip `L` holds the
//! MAX depth of all the pixels it covers: `[x << L, (x + 1) << L)` per axis, except the last texel
//! of an axis, which extends to the edge of the viewport. (wgpu sizes mip `L` as `size >> L`, which
//! floors, so on odd sizes that last texel is wider than `1 << L` and the reduction folds the odd
//! child row or column into it.) A lookup maps a pixel rectangle to the level where its texel
//! range, with the index clamped to the last texel, spans at most two texels per axis, and reads
//! those texels. No power-of-two padding is involved; the maximum over a superset of the rectangle
//! is what makes the test conservative. `hzb.rs` scales its pyramid differently and is not reused.
//!
//! # Depth margin
//! Unit: `u = 2^-24` (unit roundoff of f32 and spacing of depths just below 1.0); the margin is
//! absolute (NDC 0..1 units). A box is occluded only if the nearest depth of its eight corners
//! exceeds the maximum Hi-Z depth over its screen rectangle plus
//! `u * (MARGIN_RHO_U * rho + MARGIN_BASE_U)`, where `rho = max over corners of |corner| / w` is
//! computed in the shader (the test is skipped above [`RHO_MAX`]). It is sound if no fragment of
//! any cell inside the box is nearer than `nearest - margin`; [`depth_error_bound_u`] is the
//! derivation, [`MARGIN_RHO_U`] and [`MARGIN_BASE_U`] a linear upper bound of it.
//!
//! Derivation (`d = z_clip / w`, `p` the eye-relative corner, `rho = |p| / w >= 1`):
//! 1. `z_clip` and `w` are 4-term sums over the f32 matrix `M = proj * view` (eye at the origin,
//!    so the translation column is zero). Any evaluation order or contraction (`fma`) rounds each
//!    term at most four times: error <= `4u * (sum |m_i p_i| + |m_b|)`.
//! 2. The view rows are unit vectors, so `sum |m_wi p_i| <= |p|`; with `a = f / (f - n)` the
//!    depth row gives `T_z <= a (|p| + b')` and `b' = n f / (f - n) <= a w` (`w >= n` because the
//!    test requires `z >= 0`). `OrbitCamera::clip_distances` keeps `f >= 2n`, so `a <= 2`.
//!    The error of `d` is then `E_c <= 4u (a (rho + 1) + rho) * kappa`, `kappa >= 1` absorbing the
//!    perturbation of the denominator (< 2 % for `rho <= RHO_MAX`).
//! 3. The nearest computed corner depth of the box and the vertex-stage depth of a corner of a
//!    cell inside the box (the page pass tests the page box, whose corners are not the cell's)
//!    both sit within `E_c` of the exact depth, which is monotone along every box edge, so the
//!    exact minimum over the box is attained at a corner and bounds all of its points. `rho` is
//!    quasi-convex (`|p| <= c w` is a convex cone), so its maximum over the box is at a corner
//!    too. The two clip evaluations cost `2 E_c`.
//! 4. The shader's division adds at most 2.5 ulp (WGSL specification, "Floating Point
//!    Accuracy"; Vulkan "Precision and Operation of SPIR-V Instructions", `OpFDiv`), and the sum
//!    `h + margin` rounds by up to `u` when the result reaches 1.
//! 5. The fixed-function stages (the hardware `z / w`, snapping to the sub-pixel grid and the
//!    interpolation across the triangle) are not specified by WebGPU or Vulkan (Rasterization,
//!    "Basic Polygon Rasterization"); [`RASTER_U`] is an assumption, twice the 2.00 u measured on
//!    RADV and llvmpipe by `depth_error_stays_within_the_derived_bound_at_worst_case_poses`, which also
//!    checks the bound on the GPU at far, grazing and eye-near poses. Depth32Float stores the
//!    fragment depth unquantized, so the Hi-Z holds it exactly.
//!
//! At `rho = 1` the bound is about 48 u, growing about 25 u per unit of `rho`.
//!
//! The screen rectangle is the bounding box of the projected corners, widened by half a pixel
//! (pixel centres) and [`RECT_SLACK`] pixels (the rasterizer snaps vertices to a sub-pixel grid).
//! A corner with `w <= 1e-30` or `z < 0` (behind or in front of the near plane), or any
//! non-finite coordinate, disables the test (the comparisons are written so that NaN fails them).
//!
//! The Hi-Z lives with the viewport: it is rebuilt when the size changes and by
//! `CellRenderer::invalidate_viewport` (resize and `Outdated`). Like the visibility bits it is only
//! derived state: after a lost device the whole renderer is recreated and both start empty.

/// Unit of the depth margin, `u = 2^-24`.
pub(crate) const DEPTH_UNIT: f64 = 1.0 / (1 << 24) as f64;
/// Roundings a term of a 4-term clip-space sum can suffer under any order or contraction.
#[cfg(test)]
pub(crate) const CLIP_ROUNDINGS: f64 = 4.0;
/// Upper bound of `a = f / (f - n)`: the camera keeps `f >= 2n`.
#[cfg(test)]
pub(crate) const PROJECTION_A_MAX: f64 = 2.0;
/// ULPs of one division of the occlusion shader.
#[cfg(test)]
pub(crate) const DIVISION_ULPS: f64 = 2.5;
/// Assumed error of the fixed-function stages, in `u`: twice the measured 2.00 u.
#[cfg(test)]
pub(crate) const RASTER_U: f64 = 4.0;
/// Rounding of `h + margin` when the sum reaches 1, in `u`.
#[cfg(test)]
pub(crate) const SUM_U: f64 = 1.0;
/// Largest `rho = |corner| / w` the test accepts; beyond it the box is never occluded.
pub(crate) const RHO_MAX: f64 = 65536.0;
/// Linear upper bound of [`depth_error_bound_u`] over `1 <= rho <= RHO_MAX`: slope and offset, in `u`.
pub(crate) const MARGIN_RHO_U: f64 = 25.0;
pub(crate) const MARGIN_BASE_U: f64 = 24.0;

/// Depth error that must stay below the margin, in `u`, for a box whose corners have
/// `|corner| / w <= rho`. See the module documentation for the derivation.
#[cfg(test)]
pub(crate) fn depth_error_bound_u(rho: f64) -> f64 {
    let perturbation = 1.0 + f64::powi(2.0, -22);
    let kappa = perturbation / (1.0 - CLIP_ROUNDINGS * rho * perturbation * DEPTH_UNIT);
    let clip = CLIP_ROUNDINGS * (PROJECTION_A_MAX * (rho + 1.0) + rho) * kappa;
    2.0 * clip + DIVISION_ULPS + RASTER_U + SUM_U
}

/// The margin the shader applies, in `u`.
#[cfg(test)]
pub(crate) fn depth_margin_u(rho: f64) -> f64 {
    MARGIN_RHO_U * rho + MARGIN_BASE_U
}

/// Pixels the rectangle of a box grows on every side beyond the half pixel of the pixel centres.
pub(crate) const RECT_SLACK: f32 = 0.0625;

/// Bytes of the visibility bits of `pages` pages: per half (two halves) one `u32` per page and
/// sixteen `u32` (512 bits) per page for its slots.
pub(crate) const fn bits_bytes(pages: u64) -> u64 {
    pages * 2 * (4 + 16 * 4)
}

/// Zeroes both halves of the page bit and the slot bits of `slots`, writing each run of
/// consecutive slots once.
pub(crate) fn clear_page_bits(
    queue: &wgpu::Queue,
    bits: &wgpu::Buffer,
    capacity: u32,
    slots: &[u32],
) {
    let mut sorted = slots.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let zeros = vec![0_u32; sorted.len().max(1) * 16];
    let capacity = u64::from(capacity);
    let mut start = 0;
    while start < sorted.len() {
        let mut end = start + 1;
        while end < sorted.len() && sorted[end] == sorted[end - 1] + 1 {
            end += 1;
        }
        let first = u64::from(sorted[start]);
        let count = end - start;
        for half in 0..2_u64 {
            queue.write_buffer(
                bits,
                (half * capacity + first) * 4,
                bytemuck::cast_slice(&zeros[..count]),
            );
            queue.write_buffer(
                bits,
                (2 * capacity + half * capacity * 16 + first * 16) * 4,
                bytemuck::cast_slice(&zeros[..count * 16]),
            );
        }
        start = end;
    }
}

/// WGSL of the occlusion test; it needs `CELL_PAGES_WGSL` and the common frame code before it.
pub(crate) fn occlusion_wgsl() -> String {
    let unit = DEPTH_UNIT as f32;
    format!(
        r"
@group(3) @binding(0) var hiz: texture_2d<f32>;

const DEPTH_UNIT: f32 = {unit:e};
const MARGIN_RHO_U: f32 = {MARGIN_RHO_U:e};
const MARGIN_BASE_U: f32 = {MARGIN_BASE_U:e};
const RHO_MAX: f32 = {RHO_MAX:e};
const RECT_SLACK: f32 = {RECT_SLACK:e};

// True only if every pixel the box [lo, hi] can touch already holds something nearer than its
// nearest corner (see the module documentation of `cell_cull.rs`).
fn cell_occluded(lo: vec3<f32>, hi: vec3<f32>) -> bool {{
    var nearest = 3.0e38;
    var rho = 0.0;
    var pmin = vec2<f32>(3.0e38);
    var pmax = vec2<f32>(-3.0e38);
    for (var i = 0u; i < 8u; i++) {{
        let corner = cell_corner(lo, hi, i);
        let q = cell_clip(corner);
        // Written so that a NaN fails the test and takes the not-occluded exit.
        if !(q.w > 1.0e-30) || !(q.z >= 0.0) {{ return false; }}
        let ndc = q.xyz / q.w;
        if !(abs(ndc.x) <= 3.0e38 && abs(ndc.y) <= 3.0e38 && ndc.z <= 3.0e38) {{ return false; }}
        nearest = min(nearest, ndc.z);
        rho = max(rho, length(corner) / q.w);
        let px = vec2<f32>(
            (ndc.x * 0.5 + 0.5) * frame.viewport.x, (0.5 - ndc.y * 0.5) * frame.viewport.y);
        pmin = min(pmin, px);
        pmax = max(pmax, px);
    }}
    if !(rho <= RHO_MAX) {{ return false; }}
    let limit = frame.viewport + vec2<f32>(2.0);
    let low = vec2<i32>(floor(clamp(pmin - vec2<f32>(0.5 + RECT_SLACK), vec2<f32>(-2.0), limit)));
    let high = vec2<i32>(floor(clamp(pmax - vec2<f32>(0.5 - RECT_SLACK), vec2<f32>(-2.0), limit)));
    let max_px = vec2<i32>(frame.viewport) - vec2<i32>(1);
    if high.x < 0 || high.y < 0 || low.x > max_px.x || low.y > max_px.y {{ return false; }}
    let a = clamp(low, vec2<i32>(0), max_px);
    let b = clamp(high, vec2<i32>(0), max_px);
    var level = 0u;
    var last = max_px;
    loop {{
        let shift = vec2<u32>(level);
        last = vec2<i32>(textureDimensions(hiz, i32(level))) - vec2<i32>(1);
        let d = min(b >> shift, last) - min(a >> shift, last);
        if (d.x <= 1 && d.y <= 1) || level + 1u >= frame.mip_count {{ break; }}
        level++;
    }}
    let shift = vec2<u32>(level);
    let a2 = min(a >> shift, last);
    let b2 = min(b >> shift, last);
    let l = i32(level);
    var h = textureLoad(hiz, vec2<i32>(a2.x, a2.y), l).r;
    h = max(h, textureLoad(hiz, vec2<i32>(b2.x, a2.y), l).r);
    h = max(h, textureLoad(hiz, vec2<i32>(a2.x, b2.y), l).r);
    h = max(h, textureLoad(hiz, vec2<i32>(b2.x, b2.y), l).r);
    return nearest > h + DEPTH_UNIT * (MARGIN_RHO_U * rho + MARGIN_BASE_U);
}}
"
    )
}

const HIZ_COPY_WGSL: &str = r"
@group(0) @binding(0) var src: texture_depth_2d;
@group(0) @binding(1) var dst: texture_storage_2d<r32float, write>;
@compute @workgroup_size(8, 8)
fn copy_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(dst);
    if id.x >= size.x || id.y >= size.y { return; }
    textureStore(dst, vec2<i32>(id.xy), vec4<f32>(textureLoad(src, vec2<i32>(id.xy), 0), 0.0, 0.0, 0.0));
}
";

// Texel (x, y) of the destination is the max of the source texels 2x..2x+1, 2y..2y+1. The last
// texel of an axis also takes the source's odd last texel, which `size >> 1` leaves out.
const HIZ_REDUCE_WGSL: &str = r"
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
";

/// WGSL sources of the Hi-Z build, for validation in tests.
#[cfg(test)]
pub(crate) fn hiz_shader_sources() -> [&'static str; 2] {
    [HIZ_COPY_WGSL, HIZ_REDUCE_WGSL]
}

/// Levels of a pyramid whose last level is one texel (`size >> level`, never below 1).
pub(crate) fn mip_count(size: [u32; 2]) -> u32 {
    32 - size[0].max(size[1]).max(1).leading_zeros()
}

/// Texels of one axis at `level`, as wgpu sizes a mip chain.
pub(crate) fn level_size(size: u32, level: u32) -> u32 {
    (size >> level).max(1)
}

/// Pipelines and layouts that build and sample the pyramid; independent of the viewport.
pub(crate) struct HiZBuilder {
    copy_pipeline: wgpu::ComputePipeline,
    reduce_pipeline: wgpu::ComputePipeline,
    copy_layout: wgpu::BindGroupLayout,
    reduce_layout: wgpu::BindGroupLayout,
    /// Group 3 of the page and slot passes: the whole pyramid, sampled.
    pub sample_layout: wgpu::BindGroupLayout,
}

impl HiZBuilder {
    pub fn new(device: &wgpu::Device) -> Self {
        let texture = |binding, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty,
            count: None,
        };
        let sampled = wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        };
        let depth = wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Depth,
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        };
        let storage = wgpu::BindingType::StorageTexture {
            access: wgpu::StorageTextureAccess::WriteOnly,
            format: wgpu::TextureFormat::R32Float,
            view_dimension: wgpu::TextureViewDimension::D2,
        };
        let layout = |label, entries: &[wgpu::BindGroupLayoutEntry]| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some(label),
                entries,
            })
        };
        let copy_layout = layout(
            "cell-hiz-copy-layout",
            &[texture(0, depth), texture(1, storage)],
        );
        let reduce_layout = layout(
            "cell-hiz-reduce-layout",
            &[texture(0, sampled), texture(1, storage)],
        );
        let sample_layout = layout("cell-hiz-sample-layout", &[texture(0, sampled)]);
        let pipeline = |label, source: &str, entry_point, layout: &wgpu::BindGroupLayout| {
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            });
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &[Some(layout)],
                immediate_size: 0,
            });
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                module: &module,
                entry_point: Some(entry_point),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        Self {
            copy_pipeline: pipeline("cell-hiz-copy", HIZ_COPY_WGSL, "copy_main", &copy_layout),
            reduce_pipeline: pipeline(
                "cell-hiz-reduce",
                HIZ_REDUCE_WGSL,
                "reduce_main",
                &reduce_layout,
            ),
            copy_layout,
            reduce_layout,
            sample_layout,
        }
    }
}

/// The pyramid of one viewport size, with the bind groups that fill and sample it.
pub(crate) struct HiZ {
    size: [u32; 2],
    mip_count: u32,
    bytes: u64,
    /// The views keep the texture alive; tests read it back.
    #[cfg(test)]
    texture: wgpu::Texture,
    pub sample_group: wgpu::BindGroup,
    /// Level 0 as the storage target of the depth copy.
    level0_view: wgpu::TextureView,
    /// `reduce_groups[l - 1]` reads level `l - 1` and writes level `l`.
    reduce_groups: Vec<wgpu::BindGroup>,
}

impl HiZ {
    pub fn new(device: &wgpu::Device, builder: &HiZBuilder, size: [u32; 2]) -> Self {
        let size = size.map(|v| v.max(1));
        let mip_count = mip_count(size);
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("cell-hiz"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: mip_count,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::STORAGE_BINDING
                | if cfg!(test) {
                    wgpu::TextureUsages::COPY_SRC
                } else {
                    wgpu::TextureUsages::empty()
                },
            view_formats: &[],
        });
        let level_view = |level| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                base_mip_level: level,
                mip_level_count: Some(1),
                ..Default::default()
            })
        };
        let bytes = (0..mip_count)
            .map(|level| {
                u64::from(level_size(size[0], level)) * u64::from(level_size(size[1], level)) * 4
            })
            .sum();
        let reduce_groups = (1..mip_count)
            .map(|level| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("cell-hiz-reduce"),
                    layout: &builder.reduce_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&level_view(level - 1)),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(&level_view(level)),
                        },
                    ],
                })
            })
            .collect();
        let sample_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cell-hiz-sample"),
            layout: &builder.sample_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(
                    &texture.create_view(&Default::default()),
                ),
            }],
        });
        let level0_view = level_view(0);
        Self {
            size,
            mip_count,
            bytes,
            #[cfg(test)]
            texture,
            sample_group,
            level0_view,
            reduce_groups,
        }
    }

    pub fn size(&self) -> [u32; 2] {
        self.size
    }

    pub fn mip_count(&self) -> u32 {
        self.mip_count
    }

    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    #[cfg(test)]
    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }

    /// Records the copy of `depth_view` (the whole texture, of this pyramid's size) into level 0
    /// and the reduction of the remaining levels, one pass each.
    pub fn encode_build(
        &self,
        device: &wgpu::Device,
        builder: &HiZBuilder,
        encoder: &mut wgpu::CommandEncoder,
        depth_view: &wgpu::TextureView,
    ) {
        let copy_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cell-hiz-copy"),
            layout: &builder.copy_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(depth_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&self.level0_view),
                },
            ],
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("cell-hiz-copy-pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&builder.copy_pipeline);
            pass.set_bind_group(0, &copy_group, &[]);
            pass.dispatch_workgroups(self.size[0].div_ceil(8), self.size[1].div_ceil(8), 1);
        }
        for (index, group) in self.reduce_groups.iter().enumerate() {
            let level = index as u32 + 1;
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("cell-hiz-reduce-pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&builder.reduce_pipeline);
            pass.set_bind_group(0, group, &[]);
            pass.dispatch_workgroups(
                level_size(self.size[0], level).div_ceil(8),
                level_size(self.size[1], level).div_ceil(8),
                1,
            );
        }
    }
}
