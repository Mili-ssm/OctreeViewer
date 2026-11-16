//! Static GPU pool of cells and its compacting draw, with two-phase occlusion (R3 Tasks 7b1, 7b2).
//!
//! The pool holds the 16-byte instances of `cell_pages` in 8 KiB pages. Every frame four passes run
//! on the GPU: a page pass (frustum on the tight box of each active page), a finalize step (indirect
//! dispatch and draw arguments), a slot pass (one workgroup per surviving page: empty, octant, node
//! mask and frustum tests, a prefix sum and one `atomicAdd` per workgroup) and one
//! `drawIndexedIndirect` of the surviving cells.
//!
//! With occlusion on (the default) that sequence runs twice, as `kpc` of Task 16: phase 1 draws what
//! was visible last frame, a Hi-Z pyramid is built from its depth, and phase 2 tests everything
//! against it (page box and each cell) and draws what phase 1 did not. `cell_cull.rs` documents the
//! scheme, the Hi-Z and the depth margin. With occlusion off only the first sequence runs (`k`).
//!
//! Conventions shared with the CPU side:
//! - a mask bit set means "the cell is replaced by finer cells": it is not drawn;
//! - a page whose octant word is 0 is inert (that is how free and inactive pages look); a level that
//!   spans octants (instance flag bit 9) skips the octant and mask tests;
//! - pages are eye-relative and the frame matrix has the eye at the origin
//!   (`OrbitCamera::eye_frame`), so the caller rewrites the page entries when the camera moves.
//!
//! Every write goes through `queue.write_buffer`. Per frame the caller applies frees, uploads and
//! the page table, in that order, before submitting.

use std::{mem::size_of, num::NonZeroU64};

use bytemuck::{Pod, Zeroable};

use crate::cell_cull::{HiZ, HiZBuilder, bits_bytes, clear_page_bits, occlusion_wgsl};
use crate::cell_lod::LodView;
use crate::cell_pages::{
    CELL_PAGES_WGSL, PAGE_SLOTS, PackedLevel, PageEntry, PageFrame, srgb_to_linear_table,
};
use crate::gpu::CUBE_INDICES;
use crate::scene::SHADER_MODE_COLOR_WGSL;
use crate::{CellColorRanges, ColorMode};

/// Bytes of every buffer of a pool of `pages` pages (see `CellRenderer::new`).
pub(crate) const fn pool_resource_bytes(pages: u32) -> u64 {
    let pages = pages as u64;
    pages * PAGE_BYTES // instances
        + pages * size_of::<PageEntry>() as u64 // page table
        + pages * (NODE_WORDS as u64 * 4) // node records
        + 1024 // sRGB table
        + size_of::<FrameUniform>() as u64
        + (pages + 1) * 4 // page list
        + pages * PAGE_SLOTS as u64 * 4 // visible indices
        + bits_bytes(pages) // visibility bits
        + 2 * DRAW_ARGS_BYTES // draw arguments, one set per phase
        + COUNTER_BYTES // per-depth counters
        + 12 // dispatch arguments
        + 72 // cube indices
}

/// Bytes of one page of instances.
pub const PAGE_BYTES: u64 = (PAGE_SLOTS * 16) as u64;
/// Pages of the largest pool: 512 MiB (ADR 0040).
pub const POOL_PAGES: u32 = 65_536;
/// Bytes of the largest instance buffer; the device is asked for a storage binding this large.
pub const POOL_BYTES: u64 = POOL_PAGES as u64 * PAGE_BYTES;
/// Pages of the smallest pool the cells path runs with: 256 MiB. Below it the CPU path draws.
pub const MIN_POOL_PAGES: u32 = 32_768;
/// Storage buffers per shader stage that the passes need. The compute group binds seven
/// (instances, pages, masks, page list, visible, draw arguments, visibility bits) and the finalize
/// pipeline adds the dispatch arguments in its own group: 8, the WebGPU minimum, with no headroom.
/// The page and slot pipelines have one spare; a new buffer in the compute group would break
/// finalize. `slot_pass_stays_within_eight_storage_buffers` checks every pipeline.
pub const MIN_STORAGE_BUFFERS: u32 = 8;
/// `u32` words of the mask of one node (8 x 8 x 8 bits): words `0..MASK_WORDS` of its record.
pub const MASK_WORDS: usize = 16;
/// `u32` words of a node record, the per-slot layout of the `masks` buffer (E8): `0..16` the mask;
/// `16..19` the node corner (f32 bits, eye-relative, scaled units); `19` the node depth; `20..22`
/// the active depths (low, high); `22..24` the 4^3 mip (low, high); `24` the 2^3 mip in bits
/// `0..8` and the 1^3 mip at bit 8; `25..32` zero. `cell_lod::node_record` builds it.
pub const NODE_WORDS: usize = 32;

/// Bytes of the draw arguments of one phase; phase 2 follows phase 1 in the buffer.
const DRAW_ARGS_BYTES: u64 = 20;
/// Depths the per-cell counters tell apart.
pub const COUNTER_DEPTHS: usize = 64;
/// Bytes of the per-depth counters, appended to the `draw_args` buffer after both phases' arguments.
const COUNTER_BYTES: u64 = COUNTER_DEPTHS as u64 * 4;
/// Word of `draw_args` where the counters start.
const COUNTER_OFFSET: u64 = 2 * DRAW_ARGS_BYTES;

/// Cells drawn by absolute depth (`page.depth`) in the last frame rendered with counters on,
/// summed over the phases of the frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LodCounters {
    pub per_depth: [u32; COUNTER_DEPTHS],
}
const PAGE_PASS_GROUP: u32 = 64;
const SLOT_PASS_GROUP: u32 = 256;

/// How the device limits are chosen. `DownlevelOnly` forces the limits of the CPU path, which makes
/// `SurfaceRenderer::cells_supported` false; it exists for tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CellLimitMode {
    /// Raise the limits the pool needs, never above what the adapter gives.
    #[default]
    Auto,
    /// Request `downlevel_defaults` only.
    DownlevelOnly,
}

/// Limits of a device request. `cpu_limits` are what the CPU path has always used for its size and
/// HZB decisions; they do not change when `required_limits` are raised for the pool.
#[derive(Debug, Clone)]
pub(crate) struct DeviceLimits {
    pub required_limits: wgpu::Limits,
    pub cpu_limits: wgpu::Limits,
}

pub(crate) fn plan_device_limits(adapter: &wgpu::Adapter, mode: CellLimitMode) -> DeviceLimits {
    let adapter_limits = adapter.limits();
    let cpu_limits = wgpu::Limits::downlevel_defaults().using_resolution(adapter_limits.clone());
    let mut required_limits = cpu_limits.clone();
    if mode == CellLimitMode::Auto {
        required_limits.max_storage_buffers_per_shader_stage =
            cpu_limits.max_storage_buffers_per_shader_stage.max(
                adapter_limits
                    .max_storage_buffers_per_shader_stage
                    .min(MIN_STORAGE_BUFFERS),
            );
        required_limits.max_storage_buffer_binding_size =
            cpu_limits.max_storage_buffer_binding_size.max(
                adapter_limits
                    .max_storage_buffer_binding_size
                    .min(POOL_BYTES),
            );
        required_limits.max_buffer_size = cpu_limits
            .max_buffer_size
            .max(adapter_limits.max_buffer_size.min(POOL_BYTES));
    }
    DeviceLimits {
        required_limits,
        cpu_limits,
    }
}

/// Pages of the pool the limits admit (ADR 0040): [`POOL_PAGES`] or what a storage binding, a
/// buffer and a dispatch of one workgroup per page allow, whichever is smaller. `None` below
/// [`MIN_POOL_PAGES`].
pub(crate) fn pool_pages_for_limits(limits: &wgpu::Limits) -> Option<u32> {
    let bytes = limits
        .max_storage_buffer_binding_size
        .min(limits.max_buffer_size)
        .min(POOL_BYTES);
    let pages = (bytes / PAGE_BYTES)
        .min(u64::from(limits.max_compute_workgroups_per_dimension))
        .min(u64::from(POOL_PAGES)) as u32;
    (pages >= MIN_POOL_PAGES).then_some(pages)
}

/// Default budget of the pool's own resources (instances, page table, masks, page list, visible
/// indices, visibility bits): 660 MiB, above the 655 MiB the largest pool needs (ADR 0040). It is
/// separate from `GpuBudget`, which drives the CPU path.
pub const DEFAULT_CELL_POOL_BUDGET_BYTES: u64 = 660 << 20;

/// Pages of the pool for `limits` and a `budget_bytes` for its resources: the largest the limits
/// admit ([`pool_pages_for_limits`]) whose [`pool_resource_bytes`] fit the budget; `None` when
/// that is below [`MIN_POOL_PAGES`].
pub(crate) fn pool_pages_within_budget(limits: &wgpu::Limits, budget_bytes: u64) -> Option<u32> {
    let (mut low, mut high) = (0, pool_pages_for_limits(limits)?);
    // `pool_resource_bytes` grows with the pages: the largest that fits.
    while low < high {
        let mid = low + (high - low).div_ceil(2);
        if pool_resource_bytes(mid) <= budget_bytes {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    (low >= MIN_POOL_PAGES).then_some(low)
}

/// The pool the device runs, in pages: its limits and `budget_bytes` (see
/// [`pool_pages_within_budget`]) plus compute, indirect execution, storage buffers in the vertex
/// stage, `r32float` storage textures and a sampleable depth texture (the Hi-Z of Task 7b2).
/// `None`: the CPU path.
pub(crate) fn cell_pool_pages(
    adapter: &wgpu::Adapter,
    device_limits: &wgpu::Limits,
    budget_bytes: u64,
) -> Option<u32> {
    let pages = pool_pages_within_budget(device_limits, budget_bytes)?;
    cells_supported_for(adapter, device_limits, u64::from(pages) * PAGE_BYTES).then_some(pages)
}

/// Whether the device can run the cells path with the default pool budget.
pub(crate) fn cells_supported(adapter: &wgpu::Adapter, device_limits: &wgpu::Limits) -> bool {
    cell_pool_pages(adapter, device_limits, DEFAULT_CELL_POOL_BUDGET_BYTES).is_some()
}

/// [`cells_supported`] for a pool of `pool_bytes`; tests use small pools on adapters whose own
/// limits are below the production size.
pub(crate) fn cells_supported_for(
    adapter: &wgpu::Adapter,
    device_limits: &wgpu::Limits,
    pool_bytes: u64,
) -> bool {
    let flags = adapter.get_downlevel_capabilities().flags;
    let depth = adapter.get_texture_format_features(wgpu::TextureFormat::Depth32Float);
    let needed = wgpu::DownlevelFlags::COMPUTE_SHADERS
        | wgpu::DownlevelFlags::INDIRECT_EXECUTION
        | wgpu::DownlevelFlags::VERTEX_STORAGE;
    let r32float = adapter.get_texture_format_features(wgpu::TextureFormat::R32Float);
    flags.contains(needed)
        && depth
            .allowed_usages
            .contains(wgpu::TextureUsages::TEXTURE_BINDING)
        && r32float
            .allowed_usages
            .contains(wgpu::TextureUsages::STORAGE_BINDING)
        && r32float
            .flags
            .contains(wgpu::TextureFormatFeatureFlags::STORAGE_WRITE_ONLY)
        && device_limits.max_storage_buffers_per_shader_stage >= MIN_STORAGE_BUFFERS
        && device_limits.max_storage_buffer_binding_size >= pool_bytes
        && device_limits.max_buffer_size >= pool_bytes
        && device_limits.max_compute_invocations_per_workgroup >= SLOT_PASS_GROUP
        && device_limits.max_compute_workgroup_size_x >= SLOT_PASS_GROUP
        && u64::from(device_limits.max_compute_workgroups_per_dimension) >= pool_bytes / PAGE_BYTES
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CellPoolError {
    /// The device does not meet the limits and features of the pool (`SurfaceRenderer::cells_supported`).
    Unsupported,
    PageCountMismatch {
        pages: usize,
        slots: usize,
    },
    PageSlotOutOfRange {
        slot: u32,
        capacity: u32,
    },
    DuplicatePageSlot(u32),
    MaskSlotOutOfRange {
        slot: u32,
        capacity: u32,
    },
    /// The mask slot is not allocated (never taken, or already freed).
    MaskSlotNotAllocated(u32),
    /// A `PackedLevel` that breaks its own invariants.
    InvalidLevel(&'static str),
    /// A free or mask call before any level created the pool.
    NoPool,
    /// Creating the pool would pass the GPU budget.
    BudgetExceeded {
        required_bytes: u64,
        max_bytes: u64,
    },
}

impl std::fmt::Display for CellPoolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported => f.write_str("this GPU cannot run the cell pool"),
            Self::PageCountMismatch { pages, slots } => {
                write!(f, "level has {pages} pages but {slots} slots were given")
            }
            Self::PageSlotOutOfRange { slot, capacity } => {
                write!(
                    f,
                    "page slot {slot} is outside the pool of {capacity} pages"
                )
            }
            Self::DuplicatePageSlot(slot) => write!(f, "page slot {slot} was given twice"),
            Self::MaskSlotOutOfRange { slot, capacity } => {
                write!(f, "mask slot {slot} is outside the {capacity} mask slots")
            }
            Self::MaskSlotNotAllocated(slot) => write!(f, "mask slot {slot} is not allocated"),
            Self::InvalidLevel(reason) => write!(f, "invalid packed level: {reason}"),
            Self::NoPool => f.write_str("the cell pool does not exist yet"),
            Self::BudgetExceeded {
                required_bytes,
                max_bytes,
            } => write!(
                f,
                "the cell pool needs a peak of {required_bytes} bytes, the GPU budget is {max_bytes}"
            ),
        }
    }
}

impl std::error::Error for CellPoolError {}

/// Per-frame inputs of [`CellRenderer::encode_cells`].
#[derive(Debug, Clone, Copy)]
pub(crate) struct CellFrame {
    /// Eye-at-origin matrix of `OrbitCamera::eye_frame`.
    pub view_projection: [[f32; 4]; 4],
    pub clear_color: wgpu::Color,
    /// The face and depth shading of the CPU path; off gives the plain cell colour.
    pub depth_shading: bool,
    /// Size in pixels of the colour and depth targets; the Hi-Z follows it.
    pub viewport: [u32; 2],
}

/// What the cell shader needs for the shader colour modes (R3-Q32): the mode and the ranges, as
/// `SurfaceRenderer` derives them for the frame. Changing them only rewrites the frame uniform.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct CellColor {
    pub mode: ColorMode,
    /// Min and max cell intensity, as `u16` values.
    pub intensity_range: [f32; 2],
    /// Added to the eye-relative height of a cell centre to give `z - zmin` in render units:
    /// `eye_z - zmin`, computed in `f64` so absolute coordinates cost no precision.
    pub height_offset: f32,
    /// `zmax - zmin` in render units.
    pub height_span: f32,
}

impl CellColor {
    /// The uniform of a frame. `eye_z` is the eye height of the frame the page entries were built
    /// with (scaled render units), `origin_z` the renderer's coordinate origin. The offset is
    /// `eye_z - (zmin - origin_z) * scale` in `f64` and rounds once to `f32`; the shader adds it to
    /// the eye-relative centre height, so no absolute coordinate is ever held in `f32`.
    pub(crate) fn for_frame(
        mode: ColorMode,
        ranges: CellColorRanges,
        eye_z: f64,
        visual_scale: f64,
        origin_z: f64,
    ) -> Self {
        let default = Self::default();
        let (height_offset, height_span) = ranges.z.map_or(
            (default.height_offset, default.height_span),
            |[zmin, zmax]| {
                (
                    (eye_z - (zmin - origin_z) * visual_scale) as f32,
                    ((zmax - zmin) * visual_scale) as f32,
                )
            },
        );
        Self {
            mode,
            intensity_range: ranges.intensity.unwrap_or(default.intensity_range),
            height_offset,
            height_span,
        }
    }
}

impl Default for CellColor {
    /// The values the CPU path uses without ranges.
    fn default() -> Self {
        Self {
            mode: ColorMode::Rgb,
            intensity_range: [0.0, 1.0],
            height_offset: 0.0,
            height_span: 1.0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct FrameUniform {
    pub(crate) view_projection: [[f32; 4]; 4],
    pub(crate) num_pages: u32,
    pub(crate) shading: u32,
    /// Half of the visibility bits that phase 1 and phase 2 read (0 or 1).
    pub(crate) prev: u32,
    pub(crate) capacity: u32,
    pub(crate) viewport: [f32; 2],
    pub(crate) mip_count: u32,
    pub(crate) _padding: u32,
    pub(crate) color_mode: f32,
    pub(crate) intensity_min: f32,
    pub(crate) intensity_max: f32,
    pub(crate) height_offset: f32,
    pub(crate) height_span: f32,
    /// 0 octant mode, 1 cell mode.
    pub(crate) lod_mode: u32,
    /// 0 off.
    pub(crate) counters: u32,
    pub(crate) target_px: f32,
    pub(crate) forward: [f32; 3],
    pub(crate) focal: f32,
    pub(crate) right: [f32; 3],
    pub(crate) min_side: f32,
    pub(crate) up: [f32; 3],
    pub(crate) _padding_end: u32,
}

pub(crate) const COMMON_WGSL: &str = r"
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
";

// `PHASE` is prepended per module: 0 = plain compaction (`k`), 1 = phase 1, 2 = phase 2.
const COMPUTE_WGSL: &str = r"
@group(1) @binding(0) var<storage, read> inst: array<vec4<u32>>;
@group(1) @binding(1) var<storage, read> pages: array<CellPage>;
@group(1) @binding(2) var<storage, read> masks: array<u32>;
@group(1) @binding(3) var<storage, read_write> page_list: array<atomic<u32>>;
@group(1) @binding(4) var<storage, read_write> visible: array<u32>;
@group(1) @binding(5) var<storage, read_write> draw_args: array<atomic<u32>>;
@group(1) @binding(6) var<storage, read_write> bits: array<u32>;
@group(2) @binding(0) var<storage, read_write> dispatch_args: array<u32>;

// Visibility bits: one word per page in each half, then sixteen words (512 slot bits) per page.
fn page_bit_index(half: u32, page: u32) -> u32 {
    return half * frame.capacity + page;
}

fn slot_word_index(half: u32, page: u32, word: u32) -> u32 {
    return 2u * frame.capacity + half * frame.capacity * 16u + page * 16u + word;
}

const COUNTER_BASE: u32 = 10u;

// Phase 2 writes its draw arguments after those of phase 1.
fn args_base() -> u32 {
    return select(0u, 5u, PHASE == 2u);
}

fn page_box_hi(page: CellPage) -> vec3<f32> {
    let box_max = vec3<f32>(
        f32(page.box_xy & 0xffffu), f32(page.box_xy >> 16u), f32(page.box_z_flags & 0xffffu));
    return fma(box_max + vec3<f32>(1.0), vec3<f32>(page.side), page.origin);
}

@compute @workgroup_size(64)
fn page_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let p = id.x;
    if p >= frame.num_pages { return; }
    let page = pages[p];
    // The page box starts at the page origin: its minimum cell has relative coordinate 0.
    let hi = page_box_hi(page);
    // An inert page (octant word 0) is never kept, but phase 2 still writes its bit.
    var keep = page.octants != 0u && !cell_outside_frustum(page.origin, hi);
    if PHASE == 1u && keep {
        keep = bits[page_bit_index(frame.prev, p)] != 0u;
    }
    if PHASE == 2u {
        if keep { keep = !cell_occluded(page.origin, hi); }
        bits[page_bit_index(1u - frame.prev, p)] = select(0u, 1u, keep);
    }
    if keep {
        let i = atomicAdd(&page_list[0], 1u);
        atomicStore(&page_list[1u + i], p);
    }
}

@compute @workgroup_size(1)
fn finalize_main() {
    dispatch_args[0] = atomicLoad(&page_list[0]);
    dispatch_args[1] = 1u;
    dispatch_args[2] = 1u;
    let base = args_base();
    atomicStore(&draw_args[base], 36u);
    atomicStore(&draw_args[base + 1u], 0u);
    atomicStore(&draw_args[base + 2u], 0u);
    atomicStore(&draw_args[base + 3u], 0u);
    atomicStore(&draw_args[base + 4u], 0u);
}

// Cell mode (`lod_mode == 1`): the WGSL twin of `cell_lod`. Sums and products are written in the
// same order as the Rust, with no `fma`. A corner at or behind the eye plane gives a finite value
// far above any target (3.0e38, just under the f32 maximum of about 3.4e38; WGSL does not guarantee
// `inf`), which refines like the Rust `+inf`.
const CELL_NEAR_EDGE: f32 = 3.0e38;

fn cell_dot(a: vec3<f32>, b: vec3<f32>) -> f32 {
    return a.x * b.x + a.y * b.y + a.z * b.z;
}

// The smallest projected edge among the 8 children of the cell: see `min_child_edge_px` in
// cell_lod.rs, whose operation order this copies.
fn cell_min_child_edge_px(lo: vec3<f32>, side: f32) -> f32 {
    let half = side * 0.5;
    var projected: array<vec2<f32>, 27>;
    var behind = 0u;
    for (var index = 0u; index < 27u; index++) {
        let steps = vec3<u32>(index % 3u, (index / 3u) % 3u, index / 9u);
        let point = vec3<f32>(
            select(select(lo.x, lo.x + half, steps.x == 1u), (lo.x + half) + half, steps.x == 2u),
            select(select(lo.y, lo.y + half, steps.y == 1u), (lo.y + half) + half, steps.y == 2u),
            select(select(lo.z, lo.z + half, steps.z == 1u), (lo.z + half) + half, steps.z == 2u));
        let depth = cell_dot(point, frame.forward);
        if depth <= 0.0 {
            behind |= 1u << index;
            continue;
        }
        projected[index] = vec2<f32>(
            frame.focal * (cell_dot(point, frame.right) / depth),
            frame.focal * (cell_dot(point, frame.up) / depth));
    }
    var smallest = CELL_NEAR_EDGE;
    for (var child = 0u; child < 8u; child++) {
        let base = (child & 1u) + ((child >> 1u) & 1u) * 3u + ((child >> 2u) & 1u) * 9u;
        var blocked = false;
        for (var corner = 0u; corner < 8u; corner++) {
            let point = base + (corner & 1u) + ((corner >> 1u) & 1u) * 3u + (corner >> 2u) * 9u;
            if ((behind >> point) & 1u) != 0u { blocked = true; }
        }
        if blocked { continue; }
        var maximum = 0.0;
        for (var corner = 0u; corner < 8u; corner++) {
            for (var axis = 0u; axis < 3u; axis++) {
                if ((corner >> axis) & 1u) != 0u { continue; }
                let first = base + (corner & 1u) + ((corner >> 1u) & 1u) * 3u + (corner >> 2u) * 9u;
                let other = corner | (1u << axis);
                let second = base + (other & 1u) + ((other >> 1u) & 1u) * 3u + (other >> 2u) * 9u;
                let dx = projected[first].x - projected[second].x;
                let dy = projected[first].y - projected[second].y;
                maximum = max(maximum, sqrt(dx * dx + dy * dy));
            }
        }
        smallest = min(smallest, maximum);
    }
    return smallest;
}

fn cell_refines(lo: vec3<f32>, side: f32) -> bool {
    return side * 0.5 >= frame.min_side && cell_min_child_edge_px(lo, side) >= frame.target_px;
}

// Is any set mask bit inside the cell `rel` of the node record at word `record`, `level` below the
// node depth? From level 3 the 8^3 bit; above it the 4^3, 2^3 and 1^3 mips (words 22..25).
fn cell_masked_any(record: u32, level: u32, rel: vec3<u32>) -> bool {
    if level >= 3u {
        let m = rel >> vec3<u32>(level - 3u);
        let bit = m.x + 8u * m.y + 64u * m.z;
        return ((masks[record + (bit >> 5u)] >> (bit & 31u)) & 1u) != 0u;
    }
    if level == 0u { return ((masks[record + 24u] >> 8u) & 1u) != 0u; }
    if level == 1u {
        return ((masks[record + 24u] >> (rel.x + 2u * rel.y + 4u * rel.z)) & 1u) != 0u;
    }
    let bit = rel.x + 4u * rel.y + 16u * rel.z;
    return ((masks[record + 22u + (bit >> 5u)] >> (bit & 31u)) & 1u) != 0u;
}

// E4/E5: does this cell draw its points? Mirrors `cell_lod::draws`, ancestors from the coarsest.
fn cell_draws(page: CellPage, cell: vec4<u32>) -> bool {
    if all(page.rel == vec3<u32>(0xffffffffu)) { return false; }
    let record = page.node * 32u;
    let node_depth = masks[record + 19u];
    if page.depth < node_depth || page.depth - node_depth > 31u { return false; }
    let level = page.depth - node_depth;
    let active_depths = masks[record + 20u];
    if ((active_depths >> level) & 1u) == 0u { return false; }
    let rel = page.rel + cell_rel(cell);
    if cell_masked_any(record, level, rel) { return false; }
    let anchor = vec3<f32>(
        bitcast<f32>(masks[record + 16u]),
        bitcast<f32>(masks[record + 17u]),
        bitcast<f32>(masks[record + 18u]));
    let finer = ((active_depths >> level) >> 1u) != 0u || masks[record + 21u] != 0u;
    if finer && cell_refines(anchor + vec3<f32>(rel) * page.side, page.side) { return false; }
    var coarser = active_depths & ((1u << level) - 1u);
    while coarser != 0u {
        let ancestor_level = countTrailingZeros(coarser);
        coarser &= coarser - 1u;
        let up = level - ancestor_level;
        let ancestor_rel = rel >> vec3<u32>(up);
        let ancestor_side = page.side * bitcast<f32>((127u + up) << 23u);
        let ancestor_lo = anchor + vec3<f32>(ancestor_rel) * ancestor_side;
        if !(cell_refines(ancestor_lo, ancestor_side)
            || cell_masked_any(record, ancestor_level, ancestor_rel)) {
            return false;
        }
    }
    return true;
}

var<workgroup> scan: array<u32, 256>;
var<workgroup> next_words: array<atomic<u32>, 16>;
var<workgroup> wg_base: u32;

// Bit 0: draw the slot in this phase. Bit 1: the slot is visible for the next frame (phase 2).
// Phase 2 skips what phase 1 drew with the predicate phase 1 used: the page was kept on its
// previous bit (the page pass of phase 1) and the slot passed on its previous bit.
fn slot_state(page: CellPage, page_index: u32, local: u32, page_was_drawn: bool) -> u32 {
    let cell = inst[page_index * 512u + local];
    if cell_is_empty(cell) { return 0u; }
    if frame.lod_mode == 1u {
        if !cell_draws(page, cell) { return 0u; }
    } else if !cell_spans_octants(cell) {
        if ((page.octants >> cell_octant(cell)) & 1u) == 0u { return 0u; }
        let bit = cell_mask_index(cell);
        if ((masks[page.node * 32u + (bit >> 5u)] >> (bit & 31u)) & 1u) == 1u { return 0u; }
    }
    let lo = cell_lo(page, cell);
    let hi = cell_hi(page, cell);
    if cell_outside_frustum(lo, hi) { return 0u; }
    if PHASE == 0u { return 1u; }
    let was_visible =
        ((bits[slot_word_index(frame.prev, page_index, local >> 5u)] >> (local & 31u)) & 1u) == 1u;
    if PHASE == 1u { return select(0u, 1u, was_visible); }
    if cell_occluded(lo, hi) { return 0u; }
    return select(3u, 2u, page_was_drawn && was_visible);
}

@compute @workgroup_size(256)
fn slot_main(@builtin(workgroup_id) wid: vec3<u32>, @builtin(local_invocation_index) lid: u32) {
    let page_index = atomicLoad(&page_list[1u + wid.x]);
    let page = pages[page_index];
    var page_was_drawn = false;
    if PHASE == 2u {
        page_was_drawn = bits[page_bit_index(frame.prev, page_index)] != 0u;
        if lid < 16u { atomicStore(&next_words[lid], 0u); }
        workgroupBarrier();
    }
    let l0 = lid * 2u;
    let l1 = l0 + 1u;
    let s0 = slot_state(page, page_index, l0, page_was_drawn);
    let s1 = slot_state(page, page_index, l1, page_was_drawn);
    if PHASE == 2u {
        if (s0 & 2u) != 0u { atomicOr(&next_words[l0 >> 5u], 1u << (l0 & 31u)); }
        if (s1 & 2u) != 0u { atomicOr(&next_words[l1 >> 5u], 1u << (l1 & 31u)); }
    }
    let c0 = s0 & 1u;
    let c1 = s1 & 1u;
    scan[lid] = c0 + c1;
    workgroupBarrier();
    for (var off = 1u; off < 256u; off = off << 1u) {
        var v = scan[lid];
        if lid >= off { v += scan[lid - off]; }
        workgroupBarrier();
        scan[lid] = v;
        workgroupBarrier();
    }
    let total = scan[255];
    if frame.counters != 0u {
        let counter = COUNTER_BASE + min(page.depth, 63u);
        if c0 == 1u { atomicAdd(&draw_args[counter], 1u); }
        if c1 == 1u { atomicAdd(&draw_args[counter], 1u); }
    }
    let exclusive = scan[lid] - (c0 + c1);
    if lid == 0u && total > 0u { wg_base = atomicAdd(&draw_args[args_base() + 1u], total); }
    workgroupBarrier();
    if total > 0u {
        let out = wg_base + exclusive;
        if c0 == 1u { visible[out] = page_index * 512u + l0; }
        if c1 == 1u { visible[out + c0] = page_index * 512u + l1; }
    }
    if PHASE == 2u && lid < 16u {
        bits[slot_word_index(1u - frame.prev, page_index, lid)] = atomicLoad(&next_words[lid]);
    }
}
";

const DRAW_WGSL: &str = r"
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
";

/// WGSL sources of the compute passes (plain, phase 1, phase 2) and of the draw, for validation in
/// tests.
pub(crate) fn shader_sources() -> [String; 4] {
    let compute = |phase: u32| {
        format!(
            "const PHASE: u32 = {phase}u;\n{CELL_PAGES_WGSL}{COMMON_WGSL}{}{COMPUTE_WGSL}",
            occlusion_wgsl()
        )
    };
    [
        compute(0),
        compute(1),
        compute(2),
        format!("{CELL_PAGES_WGSL}{COMMON_WGSL}{SHADER_MODE_COLOR_WGSL}{DRAW_WGSL}"),
    ]
}

/// The frame the page entries were built with (`PageFrame` without the grid). Picking runs in
/// this frame, not in the renderer's current one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PageFrameStamp {
    pub eye_scaled: [f64; 3],
    pub visual_scale: f64,
    pub coordinate_origin: [f64; 3],
}

impl PageFrameStamp {
    pub(crate) fn of(frame: &PageFrame) -> Self {
        Self {
            eye_scaled: frame.eye_scaled,
            visual_scale: frame.visual_scale,
            coordinate_origin: frame.coordinate_origin,
        }
    }

    /// Whether the stamp uses the given scale and origin (compared exactly: they are copied, not
    /// computed).
    pub(crate) fn same_units(&self, visual_scale: f64, coordinate_origin: [f64; 3]) -> bool {
        self.visual_scale == visual_scale && self.coordinate_origin == coordinate_origin
    }
}

/// The pool, its page and mask tables and the pipelines that draw it. Built from a device only, so
/// a headless harness and `SurfaceRenderer` share the code.
pub(crate) struct CellRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    capacity: u32,
    instances: wgpu::Buffer,
    page_table: wgpu::Buffer,
    masks: wgpu::Buffer,
    frame_uniform: wgpu::Buffer,
    page_list: wgpu::Buffer,
    draw_args: wgpu::Buffer,
    dispatch_args: wgpu::Buffer,
    cube_indices: wgpu::Buffer,
    /// Visibility bits of the two-phase occlusion, double-buffered (see `cell_cull.rs`).
    bits: wgpu::Buffer,
    common_group: wgpu::BindGroup,
    compute_group: wgpu::BindGroup,
    finalize_group: wgpu::BindGroup,
    draw_group: wgpu::BindGroup,
    /// Indexed by phase: 0 plain, 1 phase 1, 2 phase 2.
    page_pipelines: [wgpu::ComputePipeline; 3],
    finalize_pipelines: [wgpu::ComputePipeline; 3],
    slot_pipelines: [wgpu::ComputePipeline; 3],
    draw_pipeline: wgpu::RenderPipeline,
    hiz_builder: HiZBuilder,
    hiz: HiZ,
    /// Two-phase occlusion (`kpc`) when true, plain compaction (`k`) when false.
    occlusion: bool,
    /// Per-depth counters on (`set_counters`).
    counters: bool,
    /// Counters were turned off after counting: the next frame zeroes them once.
    counters_stale: bool,
    /// Half of `bits` that phase 1 and 2 read this frame; phase 2 writes the other.
    prev_half: u32,
    /// The bits describe the last frame drawn with occlusion; false forces a full clear.
    bits_valid: bool,
    resource_bytes: u64,
    /// CPU copy of the page table, to rewrite only what changed.
    shadow: Vec<PageEntry>,
    active: Vec<u32>,
    /// One past the highest slot ever active: the page pass covers `0..high_water`.
    high_water: u32,
    free_masks: Vec<u32>,
    next_mask: u32,
    mask_allocated: Vec<bool>,
    /// CPU copy of every node record, kept exactly like the GPU one: zeroed only when the slot is
    /// taken, set by `set_node_record`, untouched when the slot is freed. Picking reads its mask.
    mask_mirror: Vec<[u32; NODE_WORDS]>,
    /// The frame the entries of `shadow` were built with; `None` until the first
    /// `set_page_frame`.
    page_frame: Option<PageFrameStamp>,
    color: CellColor,
    /// `None` keeps octant mode.
    lod_view: Option<LodView>,
    /// For the tests that read the compacted slot ids back.
    #[cfg(test)]
    visible: wgpu::Buffer,
}

impl CellRenderer {
    /// `capacity` pages of the pool ([`POOL_PAGES`] in production, fewer in tests). The caller has
    /// checked `SurfaceRenderer::cells_supported` and created the device with the limits it implies.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        color_format: wgpu::TextureFormat,
        capacity: u32,
    ) -> Self {
        assert!(
            (1..=POOL_PAGES).contains(&capacity),
            "pool capacity must be 1..={POOL_PAGES} pages"
        );
        let pages = u64::from(capacity);
        let buffer = |label, size, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        use wgpu::BufferUsages as U;
        let instances = buffer(
            "cell-instances",
            pages * PAGE_BYTES,
            U::STORAGE | U::COPY_DST | U::COPY_SRC,
        );
        let page_table = buffer(
            "cell-page-table",
            pages * size_of::<PageEntry>() as u64,
            U::STORAGE | U::COPY_DST,
        );
        let masks = buffer(
            "cell-masks",
            pages * (NODE_WORDS * 4) as u64,
            U::STORAGE | U::COPY_DST,
        );
        let srgb = buffer(
            "cell-srgb",
            size_of::<[[f32; 4]; 64]>() as u64,
            U::UNIFORM | U::COPY_DST,
        );
        let frame_uniform = buffer(
            "cell-frame",
            size_of::<FrameUniform>() as u64,
            U::UNIFORM | U::COPY_DST,
        );
        let page_list = buffer("cell-page-list", (pages + 1) * 4, U::STORAGE | U::COPY_DST);
        let visible = buffer(
            "cell-visible",
            pages * PAGE_SLOTS as u64 * 4,
            U::STORAGE | U::COPY_SRC,
        );
        let bits = buffer(
            "cell-visibility-bits",
            bits_bytes(pages),
            U::STORAGE | U::COPY_DST,
        );
        let draw_args = buffer(
            "cell-draw-args",
            2 * DRAW_ARGS_BYTES + COUNTER_BYTES,
            U::STORAGE | U::INDIRECT | U::COPY_SRC | U::COPY_DST,
        );
        let dispatch_args = buffer("cell-dispatch-args", 12, U::STORAGE | U::INDIRECT);
        let cube_indices = buffer("cell-cube-indices", 72, U::INDEX | U::COPY_DST);
        queue.write_buffer(&srgb, 0, bytemuck::bytes_of(&srgb_to_linear_table()));
        queue.write_buffer(&cube_indices, 0, bytemuck::cast_slice(&CUBE_INDICES));
        let resource_bytes = [
            &instances,
            &page_table,
            &masks,
            &srgb,
            &frame_uniform,
            &page_list,
            &visible,
            &bits,
            &draw_args,
            &dispatch_args,
            &cube_indices,
        ]
        .iter()
        .map(|buffer| buffer.size())
        .sum();
        debug_assert_eq!(resource_bytes, pool_resource_bytes(capacity));

        let entry = |binding, visibility, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty,
            count: None,
        };
        let uniform = |size: usize| wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: NonZeroU64::new(size as u64),
        };
        let storage = |read_only| wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        };
        use wgpu::ShaderStages as S;
        let common_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cell-common-layout"),
            entries: &[
                entry(
                    0,
                    S::COMPUTE | S::VERTEX | S::FRAGMENT,
                    uniform(size_of::<[[f32; 4]; 64]>()),
                ),
                entry(
                    1,
                    S::COMPUTE | S::VERTEX | S::FRAGMENT,
                    uniform(size_of::<FrameUniform>()),
                ),
            ],
        });
        let compute_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cell-compute-layout"),
            entries: &[
                entry(0, S::COMPUTE, storage(true)),
                entry(1, S::COMPUTE, storage(true)),
                entry(2, S::COMPUTE, storage(true)),
                entry(3, S::COMPUTE, storage(false)),
                entry(4, S::COMPUTE, storage(false)),
                entry(5, S::COMPUTE, storage(false)),
                entry(6, S::COMPUTE, storage(false)),
            ],
        });
        // The dispatch arguments are indirect input of the slot pass, so no pass that uses them
        // as indirect may also bind them as storage: they live in a group of their own.
        let finalize_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cell-finalize-layout"),
            entries: &[entry(0, S::COMPUTE, storage(false))],
        });
        let draw_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cell-draw-layout"),
            entries: &[
                entry(0, S::VERTEX, storage(true)),
                entry(1, S::VERTEX, storage(true)),
                entry(2, S::VERTEX, storage(true)),
            ],
        });
        let bind = |label, layout: &wgpu::BindGroupLayout, buffers: &[&wgpu::Buffer]| {
            let entries: Vec<_> = buffers
                .iter()
                .enumerate()
                .map(|(binding, buffer)| wgpu::BindGroupEntry {
                    binding: binding as u32,
                    resource: buffer.as_entire_binding(),
                })
                .collect();
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout,
                entries: &entries,
            })
        };
        let common_group = bind("cell-common", &common_layout, &[&srgb, &frame_uniform]);
        let compute_group = bind(
            "cell-compute",
            &compute_layout,
            &[
                &instances,
                &page_table,
                &masks,
                &page_list,
                &visible,
                &draw_args,
                &bits,
            ],
        );
        let finalize_group = bind("cell-finalize", &finalize_layout, &[&dispatch_args]);
        let draw_group = bind(
            "cell-draw",
            &draw_layout,
            &[&instances, &page_table, &visible],
        );

        let hiz_builder = HiZBuilder::new(device);
        let hiz = HiZ::new(device, &hiz_builder, [1, 1]);
        let [plain_source, phase1_source, phase2_source, draw_source] = shader_sources();
        let module = |label, source: String| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            })
        };
        let compute_modules = [
            module("cell-compute-plain", plain_source),
            module("cell-compute-phase1", phase1_source),
            module("cell-compute-phase2", phase2_source),
        ];
        let draw_module = module("cell-draw", draw_source);
        let layout = |label, groups: &[Option<&wgpu::BindGroupLayout>]| {
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: groups,
                immediate_size: 0,
            })
        };
        // Group 2 is the finalize group in the finalize pipelines and group 3 the Hi-Z in the
        // page and slot passes; the page and slot modules reference no group 2 binding.
        let page_layout = layout(
            "cell-page-layout",
            &[
                Some(&common_layout),
                Some(&compute_layout),
                None,
                Some(&hiz_builder.sample_layout),
            ],
        );
        let finalize_pipeline_layout = layout(
            "cell-finalize-pipeline-layout",
            &[
                Some(&common_layout),
                Some(&compute_layout),
                Some(&finalize_layout),
            ],
        );
        let draw_pipeline_layout = layout(
            "cell-draw-pipeline-layout",
            &[Some(&common_layout), Some(&draw_layout)],
        );
        let compute =
            |label, module: &wgpu::ShaderModule, layout: &wgpu::PipelineLayout, entry_point| {
                device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some(label),
                    layout: Some(layout),
                    module,
                    entry_point: Some(entry_point),
                    compilation_options: Default::default(),
                    cache: None,
                })
            };
        let per_phase = |label, layout: &wgpu::PipelineLayout, entry_point| {
            compute_modules
                .each_ref()
                .map(|module| compute(label, module, layout, entry_point))
        };
        let page_pipelines = per_phase("cell-page-pass", &page_layout, "page_main");
        let finalize_pipelines = per_phase(
            "cell-finalize-pass",
            &finalize_pipeline_layout,
            "finalize_main",
        );
        let slot_pipelines = per_phase("cell-slot-pass", &page_layout, "slot_main");
        let draw_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("cell-draw-pipeline"),
            layout: Some(&draw_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &draw_module,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &draw_module,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: color_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        Self {
            device: device.clone(),
            queue: queue.clone(),
            capacity,
            instances,
            page_table,
            masks,
            frame_uniform,
            page_list,
            draw_args,
            dispatch_args,
            cube_indices,
            bits,
            common_group,
            compute_group,
            finalize_group,
            draw_group,
            page_pipelines,
            finalize_pipelines,
            slot_pipelines,
            draw_pipeline,
            hiz_builder,
            hiz,
            occlusion: true,
            counters: false,
            counters_stale: false,
            prev_half: 0,
            bits_valid: false,
            resource_bytes,
            shadow: vec![PageEntry::zeroed(); capacity as usize],
            active: Vec::new(),
            high_water: 0,
            free_masks: Vec::new(),
            next_mask: 0,
            mask_allocated: vec![false; capacity as usize],
            mask_mirror: vec![[0; NODE_WORDS]; capacity as usize],
            page_frame: None,
            color: CellColor::default(),
            lod_view: None,
            #[cfg(test)]
            visible: visible.clone(),
        }
    }

    /// Bytes of every buffer of the pool, for `allocated_gpu_bytes` and `GpuResourceStats`.
    pub fn resource_bytes(&self) -> u64 {
        self.resource_bytes + self.hiz.bytes()
    }

    /// Turns the two-phase occlusion on (`kpc`, the default) or off (`k`: compaction only). Turning
    /// it on again starts from empty visibility bits.
    pub fn set_occlusion(&mut self, enabled: bool) {
        self.occlusion = enabled;
    }

    /// Counts the cells drawn per depth on the GPU in the frames that follow (default off). Off
    /// costs the slot pass one branch on the frame uniform; the first frame after turning
    /// them off zeroes them.
    pub fn set_counters(&mut self, enabled: bool) {
        self.counters_stale |= self.counters && !enabled;
        self.counters = enabled;
    }

    /// The counters of the last frame rendered with [`Self::set_counters`] on. Blocks until the
    /// copy is mapped: for tests, benches and measurements, not for the frame path.
    pub fn read_lod_counters(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> LodCounters {
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cell-counters-staging"),
            size: COUNTER_BYTES,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(&self.draw_args, COUNTER_OFFSET, &staging, 0, COUNTER_BYTES);
        queue.submit([encoder.finish()]);
        let (sender, receiver) = std::sync::mpsc::channel();
        staging.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = sender.send(r);
        });
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll");
        receiver.recv().unwrap().expect("map");
        let view = staging.slice(..).get_mapped_range().expect("mapped range");
        let words: Vec<u32> = bytemuck::pod_collect_to_vec(&view);
        let mut per_depth = [0; COUNTER_DEPTHS];
        per_depth.copy_from_slice(&words);
        LodCounters { per_depth }
    }

    /// `Some` switches the slot pass to the per-cell decision of [`crate::cell_lod`] (`lod_mode`
    /// 1); `None` keeps the octant-bit and mask test. The driver passes `Some` in cell mode and
    /// `None` in octant mode.
    pub(crate) fn set_lod_view(&mut self, view: Option<LodView>) {
        self.lod_view = view;
    }

    /// The view the slot pass decides with (`None`: octant mode).
    pub(crate) fn lod_view(&self) -> Option<LodView> {
        self.lod_view
    }

    /// The record the GPU holds for `slot` (see `mask_mirror`).
    pub(crate) fn node_record(&self, slot: u32) -> &[u32; NODE_WORDS] {
        &self.mask_mirror[slot as usize]
    }

    /// The compacted slot ids (`page * PAGE_SLOTS + slot`) of the last compaction that drew;
    /// with occlusion each phase overwrites the previous one. The first `draw_args` word 1 (plain
    /// or phase 1) or word 6 (phase 2) says how many are valid.
    #[cfg(test)]
    pub(crate) fn visible_buffer(&self) -> &wgpu::Buffer {
        &self.visible
    }

    /// Forgets last frame's visibility: the next occlusion frame draws everything in phase 2.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "used by the camera-jump tests; a camera cut may call it"
        )
    )]
    pub fn reset_visibility(&mut self) {
        self.bits_valid = false;
    }

    /// The viewport changed or the surface was reconfigured (`resize`, `Outdated`): rebuild the
    /// Hi-Z at the next frame and start the visibility bits again.
    pub fn invalidate_viewport(&mut self) {
        self.hiz = HiZ::new(&self.device, &self.hiz_builder, [1, 1]);
        self.bits_valid = false;
    }

    /// The Hi-Z of the current viewport, read back by the tests.
    #[cfg(test)]
    pub(crate) fn hiz(&self) -> &HiZ {
        &self.hiz
    }

    /// Indirect draw arguments; word 1 is the number of cells drawn. Read back by the tests and
    /// the T7c bench.
    pub(crate) fn draw_args_buffer(&self) -> &wgpu::Buffer {
        &self.draw_args
    }

    /// Writes a packed level into the given page slots (one per page of the level, distinct).
    /// Contiguous slots are written with one call.
    pub fn upload_level(
        &mut self,
        level: &PackedLevel,
        page_slots: &[u32],
    ) -> Result<(), CellPoolError> {
        if level.pages.len() != page_slots.len() {
            return Err(CellPoolError::PageCountMismatch {
                pages: level.pages.len(),
                slots: page_slots.len(),
            });
        }
        if level.instances.len() != level.pages.len() * PAGE_SLOTS {
            return Err(CellPoolError::InvalidLevel(
                "instances.len() is not pages * 512",
            ));
        }
        let filled: usize = level.pages.iter().map(|page| page.cells as usize).sum();
        if level
            .pages
            .iter()
            .any(|page| page.cells as usize > PAGE_SLOTS)
            || level.instances.iter().filter(|i| !i.is_empty()).count() != filled
        {
            return Err(CellPoolError::InvalidLevel(
                "page cell counts do not match the non-empty instances",
            ));
        }
        let mut sorted = page_slots.to_vec();
        sorted.sort_unstable();
        if let Some(&slot) = sorted.last().filter(|slot| **slot >= self.capacity) {
            return Err(CellPoolError::PageSlotOutOfRange {
                slot,
                capacity: self.capacity,
            });
        }
        if let Some(pair) = sorted.windows(2).find(|pair| pair[0] == pair[1]) {
            return Err(CellPoolError::DuplicatePageSlot(pair[0]));
        }
        let mut start = 0;
        while start < page_slots.len() {
            let mut end = start + 1;
            while end < page_slots.len() && page_slots[end] == page_slots[end - 1] + 1 {
                end += 1;
            }
            self.queue.write_buffer(
                &self.instances,
                u64::from(page_slots[start]) * PAGE_BYTES,
                bytemuck::cast_slice(&level.instances[start * PAGE_SLOTS..end * PAGE_SLOTS]),
            );
            start = end;
        }
        // The slots hold other cells now: whatever the bits said about the old ones is stale.
        clear_page_bits(&self.queue, &self.bits, self.capacity, page_slots);
        Ok(())
    }

    /// Makes the pages inert (octant word 0) and removes them from the active set, so a stale list
    /// cannot draw them. Their instances stay until the slots are uploaded again.
    pub fn free_pages(&mut self, page_slots: &[u32]) -> Result<(), CellPoolError> {
        for &slot in page_slots {
            if slot >= self.capacity {
                return Err(CellPoolError::PageSlotOutOfRange {
                    slot,
                    capacity: self.capacity,
                });
            }
        }
        for &slot in page_slots {
            self.shadow[slot as usize] = PageEntry::zeroed();
            self.queue.write_buffer(
                &self.page_table,
                u64::from(slot) * size_of::<PageEntry>() as u64,
                bytemuck::bytes_of(&PageEntry::zeroed()),
            );
        }
        clear_page_bits(&self.queue, &self.bits, self.capacity, page_slots);
        let freed: std::collections::HashSet<u32> = page_slots.iter().copied().collect();
        self.active.retain(|slot| !freed.contains(slot));
        self.shrink_high_water();
        Ok(())
    }

    /// Lowers `high_water` past inert entries at the top so the page pass stays as short as the
    /// highest live slot.
    fn shrink_high_water(&mut self) {
        while self.high_water > 0
            && self.shadow[self.high_water as usize - 1] == PageEntry::zeroed()
        {
            self.high_water -= 1;
        }
    }

    /// Takes a free mask slot (own free list; one slot per resident (node, level)). The slot is
    /// zeroed on the GPU, so a reused slot never carries stale bits.
    pub fn alloc_mask_slot(&mut self) -> Option<u32> {
        let slot = self.free_masks.pop().or_else(|| {
            (self.next_mask < self.capacity).then(|| {
                self.next_mask += 1;
                self.next_mask - 1
            })
        })?;
        self.mask_allocated[slot as usize] = true;
        self.mask_mirror[slot as usize] = [0; NODE_WORDS];
        self.queue.write_buffer(
            &self.masks,
            u64::from(slot) * (NODE_WORDS * 4) as u64,
            bytemuck::cast_slice(&[0_u32; NODE_WORDS]),
        );
        Some(slot)
    }

    /// Returns an allocated mask slot to the free list; anything else is an error.
    pub fn free_mask_slot(&mut self, slot: u32) -> Result<(), CellPoolError> {
        if slot >= self.capacity {
            return Err(CellPoolError::MaskSlotOutOfRange {
                slot,
                capacity: self.capacity,
            });
        }
        if !std::mem::replace(&mut self.mask_allocated[slot as usize], false) {
            return Err(CellPoolError::MaskSlotNotAllocated(slot));
        }
        self.free_masks.push(slot);
        Ok(())
    }

    /// Sets the record of an allocated node slot. In its mask (words `0..MASK_WORDS`) bit
    /// `x + 8y + 64z` set means that cell is replaced by finer ones.
    pub fn set_node_record(
        &mut self,
        slot: u32,
        record: &[u32; NODE_WORDS],
    ) -> Result<(), CellPoolError> {
        self.check_mask_slot(slot)?;
        self.mask_mirror[slot as usize] = *record;
        self.queue.write_buffer(
            &self.masks,
            u64::from(slot) * (NODE_WORDS * 4) as u64,
            bytemuck::cast_slice(record),
        );
        Ok(())
    }

    /// The mask the GPU holds for `slot` (see `mask_mirror`).
    pub(crate) fn node_mask(&self, slot: u32) -> &[u32; MASK_WORDS] {
        self.mask_mirror[slot as usize][..MASK_WORDS]
            .try_into()
            .expect("the mask is the first MASK_WORDS words of a record")
    }

    /// Records the frame that the page entries are relative to; call it after `set_active_pages`,
    /// which does not know the frame and clears the stored one so a stale eye is never used.
    pub(crate) fn set_page_frame(&mut self, frame: PageFrameStamp) {
        self.page_frame = Some(frame);
    }

    /// The colour mode and ranges the next `encode_cells` draws with. Nothing is rebuilt or
    /// re-uploaded: only the frame uniform changes.
    pub(crate) fn set_color(&mut self, color: CellColor) {
        self.color = color;
    }

    pub(crate) fn page_frame(&self) -> Option<PageFrameStamp> {
        self.page_frame
    }

    pub(crate) fn instances_buffer(&self) -> &wgpu::Buffer {
        &self.instances
    }

    /// Active slots with their page-table entries, as last written.
    pub(crate) fn active_entries(&self) -> impl Iterator<Item = (u32, PageEntry)> + '_ {
        self.active
            .iter()
            .map(|&slot| (slot, self.shadow[slot as usize]))
    }

    fn check_mask_slot(&self, slot: u32) -> Result<(), CellPoolError> {
        if slot >= self.capacity {
            return Err(CellPoolError::MaskSlotOutOfRange {
                slot,
                capacity: self.capacity,
            });
        }
        if !self.mask_allocated[slot as usize] {
            return Err(CellPoolError::MaskSlotNotAllocated(slot));
        }
        Ok(())
    }

    /// Sets this frame's active pages: `(slot, entry)` with the mask slot (word 4, allocated) and
    /// the octant bits (word 5) in the entry. Slots must be distinct; order does not matter. Pages
    /// active before and absent now become inert. One contiguous write covers what changed. The
    /// whole call is validated before anything changes.
    pub fn set_active_pages(&mut self, pages: &[(u32, PageEntry)]) -> Result<(), CellPoolError> {
        let mut keep = std::collections::HashSet::with_capacity(pages.len());
        for &(slot, entry) in pages {
            if slot >= self.capacity {
                return Err(CellPoolError::PageSlotOutOfRange {
                    slot,
                    capacity: self.capacity,
                });
            }
            if !keep.insert(slot) {
                return Err(CellPoolError::DuplicatePageSlot(slot));
            }
            self.check_mask_slot(entry.0[4])?;
        }
        let mut dirty = (u32::MAX, 0_u32);
        let mut touch = |slot: u32| dirty = (dirty.0.min(slot), dirty.1.max(slot + 1));
        let mut deactivated = Vec::new();
        for &slot in &self.active {
            if !keep.contains(&slot) && self.shadow[slot as usize] != PageEntry::zeroed() {
                self.shadow[slot as usize] = PageEntry::zeroed();
                touch(slot);
                deactivated.push(slot);
            }
        }
        // The page pass stops writing bits for slots above the high-water mark, so a page that
        // leaves the active set must not keep a bit that a later activation would read.
        clear_page_bits(&self.queue, &self.bits, self.capacity, &deactivated);
        for &(slot, entry) in pages {
            if self.shadow[slot as usize] != entry {
                self.shadow[slot as usize] = entry;
                touch(slot);
            }
            self.high_water = self.high_water.max(slot + 1);
        }
        self.active = pages.iter().map(|(slot, _)| *slot).collect();
        self.page_frame = None;
        self.shrink_high_water();
        if dirty.0 < dirty.1 {
            self.queue.write_buffer(
                &self.page_table,
                u64::from(dirty.0) * size_of::<PageEntry>() as u64,
                bytemuck::cast_slice(&self.shadow[dirty.0 as usize..dirty.1 as usize]),
            );
        }
        Ok(())
    }

    /// Records the frame into `encoder`: with occlusion on, phase 1, the Hi-Z build and phase 2;
    /// with it off, one compaction and one draw. The frame uniform is written with the queue at this
    /// call, so a submission must carry at most one `encode_cells` (a second call would change the
    /// matrix of the first). `depth_view` must be a view of the whole depth texture, which needs
    /// `TEXTURE_BINDING` and the size `frame.viewport`.
    pub fn encode_cells(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        color_view: &wgpu::TextureView,
        depth_view: &wgpu::TextureView,
        frame: &CellFrame,
    ) {
        if self.counters || self.counters_stale {
            self.counters_stale = false;
            // Before every compute pass of the frame: the clear is ordered ahead of phase 1.
            encoder.clear_buffer(&self.draw_args, COUNTER_OFFSET, Some(COUNTER_BYTES));
        }
        if self.occlusion {
            if self.hiz.size() != frame.viewport.map(|v| v.max(1)) {
                self.hiz = HiZ::new(&self.device, &self.hiz_builder, frame.viewport);
                self.bits_valid = false;
            }
            if !self.bits_valid {
                encoder.clear_buffer(&self.bits, 0, None);
                self.prev_half = 0;
                self.bits_valid = true;
            }
        } else {
            // Bits are only meaningful across consecutive occlusion frames.
            self.bits_valid = false;
        }
        let lod = self.lod_view.unwrap_or(LodView {
            forward: [0.0; 3],
            right: [0.0; 3],
            up: [0.0; 3],
            focal: 0.0,
            target_px: 0.0,
            min_side: 0.0,
        });
        self.queue.write_buffer(
            &self.frame_uniform,
            0,
            bytemuck::bytes_of(&FrameUniform {
                view_projection: frame.view_projection,
                num_pages: self.high_water,
                shading: u32::from(frame.depth_shading),
                prev: self.prev_half,
                capacity: self.capacity,
                viewport: frame.viewport.map(|v| v as f32),
                mip_count: self.hiz.mip_count(),
                _padding: 0,
                color_mode: self.color.mode.shader_value(),
                intensity_min: self.color.intensity_range[0],
                intensity_max: self.color.intensity_range[1],
                height_offset: self.color.height_offset,
                height_span: self.color.height_span,
                lod_mode: u32::from(self.lod_view.is_some()),
                counters: u32::from(self.counters),
                target_px: lod.target_px,
                forward: lod.forward,
                focal: lod.focal,
                right: lod.right,
                min_side: lod.min_side,
                up: lod.up,
                _padding_end: 0,
            }),
        );
        if self.occlusion {
            self.encode_compaction(encoder, 1);
            self.encode_draw(encoder, color_view, depth_view, Some(frame.clear_color), 0);
            self.hiz
                .encode_build(&self.device, &self.hiz_builder, encoder, depth_view);
            self.encode_compaction(encoder, 2);
            self.encode_draw(encoder, color_view, depth_view, None, DRAW_ARGS_BYTES);
            // Only occlusion frames flip the halves: the next frame reads what this one wrote.
            self.prev_half = 1 - self.prev_half;
        } else {
            self.encode_compaction(encoder, 0);
            self.encode_draw(encoder, color_view, depth_view, Some(frame.clear_color), 0);
        }
    }

    /// Page pass, finalize and slot pass of `phase` (0 plain, 1, 2).
    pub(crate) fn encode_compaction(&self, encoder: &mut wgpu::CommandEncoder, phase: usize) {
        encoder.clear_buffer(&self.page_list, 0, Some(4));
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("cell-page-pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.page_pipelines[phase]);
            pass.set_bind_group(0, &self.common_group, &[]);
            pass.set_bind_group(1, &self.compute_group, &[]);
            pass.set_bind_group(3, &self.hiz.sample_group, &[]);
            pass.dispatch_workgroups(self.high_water.div_ceil(PAGE_PASS_GROUP), 1, 1);
        }
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("cell-finalize-pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.finalize_pipelines[phase]);
            pass.set_bind_group(0, &self.common_group, &[]);
            pass.set_bind_group(1, &self.compute_group, &[]);
            pass.set_bind_group(2, &self.finalize_group, &[]);
            pass.dispatch_workgroups(1, 1, 1);
        }
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("cell-slot-pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.slot_pipelines[phase]);
        pass.set_bind_group(0, &self.common_group, &[]);
        pass.set_bind_group(1, &self.compute_group, &[]);
        pass.set_bind_group(3, &self.hiz.sample_group, &[]);
        pass.dispatch_workgroups_indirect(&self.dispatch_args, 0);
    }

    /// One `drawIndexedIndirect`. `clear` is the clear colour of the first draw of the frame (which
    /// also clears depth); the second draw loads both attachments.
    fn encode_draw(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        color_view: &wgpu::TextureView,
        depth_view: &wgpu::TextureView,
        clear: Option<wgpu::Color>,
        args_offset: u64,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("cell-draw-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color_view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: clear.map_or(wgpu::LoadOp::Load, wgpu::LoadOp::Clear),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: clear.map_or(wgpu::LoadOp::Load, |_| wgpu::LoadOp::Clear(1.0)),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            occlusion_query_set: None,
            timestamp_writes: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.draw_pipeline);
        pass.set_bind_group(0, &self.common_group, &[]);
        pass.set_bind_group(1, &self.draw_group, &[]);
        pass.set_index_buffer(self.cube_indices.slice(..), wgpu::IndexFormat::Uint16);
        pass.draw_indexed_indirect(&self.draw_args, args_offset);
    }
}
