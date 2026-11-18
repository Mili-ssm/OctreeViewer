//! 16-byte cell instances and the page table of the R3 cube pool (Task 6).
//!
//! A (node, level) of `node_cells` is packed into pages of 512 slots; a cell stores its
//! coordinate relative to the page origin as `u16` x 3, so one page spans at most 2^16 cells per
//! axis. The GPU decodes a cell as `lo = fma(rel, side, origin)` and `hi = fma(rel + 1, side,
//! origin)`; [`decode_cell`] and [`CELL_PAGES_WGSL`] do the same operations in the same order, so
//! the results are bit-identical. Uploading the pool and the passes belong to Task 7.

use bytemuck::{Pod, Zeroable};
use viewer_core::{ColorRgb, NodeId, SpatialBounds};

use crate::node_cells::{CellGrid, NodeCell, NodeCellLevel, node_range, representative_depth};

/// Slots per page.
pub const PAGE_SLOTS: usize = 512;
/// Largest cell extent of a page on one axis (`u16` relative coordinates).
pub const MAX_PAGE_EXTENT: u64 = 1 << 16;
/// Instance flag: the level spans more than one octant (depth <= node depth).
pub const FLAG_SPANS_OCTANTS: u32 = 1 << 9;
/// Page flag: at least one cell of the level has an intensity.
pub const PAGE_HAS_INTENSITY: u32 = 1;
/// Page flag: at least one cell of the level has a classification.
pub const PAGE_HAS_CLASS: u32 = 2;
/// Intensity word value for "missing".
pub const MISSING_INTENSITY: u16 = u16::MAX;
/// Largest stored intensity; `MISSING_INTENSITY` is reserved.
pub const MAX_INTENSITY: u16 = u16::MAX - 1;

/// One cell: `[x | y << 16, z | flags << 16, rgba8 sRGB, intensity | class << 16]`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Pod, Zeroable)]
pub struct CellInstance(pub [u32; 4]);

impl CellInstance {
    /// Padding slot: words 0 and 1 are `u32::MAX`, which no cell can produce (flag bit 15 is 0).
    pub const EMPTY: Self = Self([u32::MAX, u32::MAX, 0, 0]);

    /// Checks words 0 and 1 only, the sentinel pair.
    pub fn is_empty(&self) -> bool {
        self.0[0] == u32::MAX && self.0[1] == u32::MAX
    }

    /// Coordinate relative to the page origin.
    pub fn rel(&self) -> [u16; 3] {
        [self.0[0] as u16, (self.0[0] >> 16) as u16, self.0[1] as u16]
    }

    pub fn flags(&self) -> u16 {
        (self.0[1] >> 16) as u16
    }

    /// Cell of the node's 8x8x8 mask grid, `x + 8y + 64z`.
    pub fn mask_index(&self) -> u32 {
        u32::from(self.flags()) & 0x1ff
    }

    pub fn spans_octants(&self) -> bool {
        u32::from(self.flags()) & FLAG_SPANS_OCTANTS != 0
    }

    /// Octant `4x + 2y + z` of the mask cell.
    pub fn octant(&self) -> u32 {
        let index = self.mask_index();
        4 * ((index & 7) >> 2) + 2 * (((index >> 3) & 7) >> 2) + (index >> 8)
    }

    pub fn rgba(&self) -> u32 {
        self.0[2]
    }

    pub fn intensity(&self) -> u16 {
        self.0[3] as u16
    }

    pub fn class(&self) -> i16 {
        (self.0[3] >> 16) as u16 as i16
    }
}

/// Page entry as the shader reads it. Words: 0-2 page origin `f32` in the render frame; 3 cell
/// side `f32`; 4 node mask slot; 5 octant bits; 6 box max x | y << 16; 7 box max z | page flags
/// << 16; 8-10 page minimum relative to the node corner at the page depth
/// ([`PageInfo::rel_origin`]); 11 page depth.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Pod, Zeroable)]
pub struct PageEntry(pub [u32; 12]);

impl PageEntry {
    pub fn origin(&self) -> [f32; 3] {
        [0, 1, 2].map(|word| f32::from_bits(self.0[word]))
    }

    pub fn side(&self) -> f32 {
        f32::from_bits(self.0[3])
    }
}

/// CPU data of one page, kept for picking and for rewriting the origin with the camera.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageInfo {
    pub node: NodeId,
    pub depth: u8,
    /// Absolute cell coordinate of the page min corner.
    pub origin: [u64; 3],
    /// First cell of the page in the level's `cells` (not an instance slot: the slot of cell `i`
    /// in page `p` is `p * PAGE_SLOTS + i - first_cell`).
    pub first_cell: usize,
    pub cells: u32,
    /// Largest relative coordinate per axis (the tight box).
    pub box_max: [u16; 3],
    /// `PAGE_HAS_*` bits of the whole (node, level), copied to each of its pages.
    pub flags: u32,
    /// Page minimum relative to the node corner at the page depth, in cells (`origin - node_lo <<
    /// (depth - node_depth)`). `u32::MAX` per axis when the page is above the node depth or more
    /// than 31 levels below it (octant-mode leaf tails): cell mode never activates those depths.
    /// Also `u32::MAX` for a node whose corner is off the global grid (see `is_grid_aligned`):
    /// the origin is only meaningful for grid-aligned nodes, which every V7 node is (ADR 0029).
    pub rel_origin: [u32; 3],
}

/// Camera-dependent inputs of a page entry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageFrame {
    pub grid: CellGrid,
    pub coordinate_origin: [f64; 3],
    pub visual_scale: f64,
    /// The camera eye in scaled render units (`OrbitCamera::position`), in `f64`. Pages are
    /// eye-relative so f32 keeps its precision where cells are closest; the view matrix then has
    /// the eye at the origin.
    pub eye_scaled: [f64; 3],
}

impl PageInfo {
    /// Cell side in dataset units.
    pub fn cell_side(&self, grid: &CellGrid) -> f64 {
        grid.root_side / (1_u64 << self.depth) as f64
    }

    /// Writes the GPU entry. Precision contract: the origin is computed in f64, so placement is
    /// exact to the f64 resolution of the frame (about `root_side * 2^-52`); from depth 53 on that
    /// is coarser than a cell (the deepest level in real data is 29). The cast to f32 then rounds
    /// to half an ulp of the eye-relative origin.
    pub fn entry(&self, frame: &PageFrame, mask_slot: u32, octant_bits: u32) -> PageEntry {
        let side = self.cell_side(&frame.grid);
        let origin: [f32; 3] = std::array::from_fn(|axis| {
            let corner = frame.grid.root_min[axis] + self.origin[axis] as f64 * side;
            ((corner - frame.coordinate_origin[axis]) * frame.visual_scale - frame.eye_scaled[axis])
                as f32
        });
        PageEntry([
            origin[0].to_bits(),
            origin[1].to_bits(),
            origin[2].to_bits(),
            ((side * frame.visual_scale) as f32).to_bits(),
            mask_slot,
            octant_bits,
            u32::from(self.box_max[0]) | u32::from(self.box_max[1]) << 16,
            u32::from(self.box_max[2]) | self.flags << 16,
            self.rel_origin[0],
            self.rel_origin[1],
            self.rel_origin[2],
            u32::from(self.depth),
        ])
    }
}

/// A packed (node, level): `instances` holds `pages.len() * PAGE_SLOTS` slots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackedLevel {
    pub node: NodeId,
    pub depth: u8,
    pub instances: Vec<CellInstance>,
    pub pages: Vec<PageInfo>,
}

impl PackedLevel {
    pub fn empty_slots(&self) -> usize {
        self.instances.len()
            - self
                .pages
                .iter()
                .map(|page| page.cells as usize)
                .sum::<usize>()
    }
}

/// Mean intensity as the CPU path stores it (`f32`), rounded into `u16`.
fn intensity_word(cell: &NodeCell) -> u16 {
    if cell.intensity_count == 0 {
        return MISSING_INTENSITY;
    }
    let mean = (cell.intensity_sum / f64::from(cell.intensity_count)) as f32;
    // A non-finite mean is missing too: `f32::min` would turn NaN into the largest intensity.
    if !mean.is_finite() || mean < 0.0 {
        MISSING_INTENSITY
    } else {
        mean.round().min(f32::from(MAX_INTENSITY)) as u16
    }
}

fn rgba_word(cell: &NodeCell) -> u32 {
    let count = u64::from(cell.count.max(1));
    let [r, g, b] = cell
        .rgb_sum
        .map(|sum| ((sum + count / 2) / count).min(255) as u32);
    r | g << 8 | b << 16 | 255 << 24
}

/// Mask-grid cell of `coord` on one axis, with the clamp of `octant_offsets`.
fn mask_axis(coord: u64, (lo, hi): (u64, u64), depth: u8, node_depth: u8) -> u32 {
    debug_assert!(
        (lo..=hi).contains(&coord),
        "cell coordinate {coord} outside the node range {lo}..={hi} at depth {depth}"
    );
    let relative = coord - lo;
    let cells = if depth >= node_depth + 3 {
        u128::from(relative >> (depth - node_depth - 3))
    } else {
        u128::from(relative) << (node_depth + 3 - depth)
    };
    cells.min(7) as u32
}

/// Whether the node corner lies on the global grid at the node depth, i.e. the 8^3 mask grid
/// starts at `node_lo << 3` at the representative depth. Cell mode requires it (ADR 0029: V7
/// nodes are a global integer octree); off-grid bounds hit `node_cells::axis_anchor`'s fallback
/// and yield no usable relative origin.
pub(crate) fn is_grid_aligned(grid: &CellGrid, bounds: SpatialBounds) -> bool {
    let rep = representative_depth(grid, bounds);
    let node_depth = rep.saturating_sub(3);
    rep < 3
        || (0..3).all(|axis| {
            node_range(grid, bounds, rep, axis).0
                == node_range(grid, bounds, node_depth, axis).0 << 3
        })
}

/// Packs one level of a node. `bounds` are the node bounds in the grid's frame. Precondition: every
/// cell lies in `node_range(depth)` of the node on each axis (the cell builder clamps to it);
/// checked with `debug_assert!`. Cell mode requires a grid-aligned node (ADR 0029); an off-grid
/// node packs with `u32::MAX` relative origins (octant mode keeps drawing those).
pub fn pack_level(
    grid: &CellGrid,
    bounds: SpatialBounds,
    node: NodeId,
    level: &NodeCellLevel,
) -> PackedLevel {
    let aligned = is_grid_aligned(grid, bounds);
    pack_level_aligned(grid, bounds, node, level, aligned)
}

/// [`pack_level`] with the alignment verdict given: an off-grid node (`aligned == false`) gets
/// `u32::MAX` relative origins instead of a wrong corner. Separate so tests reach that path
/// without tripping the `debug_assert!`.
pub(crate) fn pack_level_aligned(
    grid: &CellGrid,
    bounds: SpatialBounds,
    node: NodeId,
    level: &NodeCellLevel,
    aligned: bool,
) -> PackedLevel {
    let depth = level.depth;
    let node_depth = representative_depth(grid, bounds).saturating_sub(3);
    let lo: [(u64, u64); 3] = std::array::from_fn(|axis| node_range(grid, bounds, depth, axis));
    let node_lo: Option<[u64; 3]> =
        aligned.then(|| std::array::from_fn(|axis| node_range(grid, bounds, node_depth, axis).0));
    let spans = depth <= node_depth;
    // Constant per level, so they never split a page.
    let page_flags = level.cells.iter().fold(0, |flags, cell| {
        flags
            | if cell.intensity_count > 0 {
                PAGE_HAS_INTENSITY
            } else {
                0
            }
            | if cell.classification != -1 {
                PAGE_HAS_CLASS
            } else {
                0
            }
    });
    let mut packed = PackedLevel {
        node,
        depth,
        instances: Vec::new(),
        pages: Vec::new(),
    };
    let mut start = 0;
    let mut min = [0_u64; 3];
    let mut max = [0_u64; 3];
    for (index, cell) in level.cells.iter().enumerate() {
        if index > start && page_is_full(min, max, cell.coord, index - start) {
            close_page(
                &mut packed,
                level,
                lo,
                spans,
                node_depth,
                page_flags,
                node_lo,
                start..index,
                min,
                max,
            );
            start = index;
        }
        if index == start {
            (min, max) = (cell.coord, cell.coord);
        } else {
            for axis in 0..3 {
                min[axis] = min[axis].min(cell.coord[axis]);
                max[axis] = max[axis].max(cell.coord[axis]);
            }
        }
    }
    if start < level.cells.len() {
        let end = level.cells.len();
        close_page(
            &mut packed,
            level,
            lo,
            spans,
            node_depth,
            page_flags,
            node_lo,
            start..end,
            min,
            max,
        );
    }
    packed
}

/// Whether the page of `len` cells spanning `min..=max` must close before taking a cell at
/// `coord`: it holds `PAGE_SLOTS` cells already, or the cell would stretch it past
/// `MAX_PAGE_EXTENT` on an axis. The one split rule of `pack_level` and `page_count`.
fn page_is_full(min: [u64; 3], max: [u64; 3], coord: [u64; 3], len: usize) -> bool {
    let too_wide = (0..3)
        .any(|axis| max[axis].max(coord[axis]) - min[axis].min(coord[axis]) + 1 > MAX_PAGE_EXTENT);
    too_wide || len == PAGE_SLOTS
}

/// Page-use limit of a level: it may take at most `MAX_PAGE_FILL_RATIO` times the pages its cells
/// would fill packed tight (`ceil(cells / PAGE_SLOTS)`), or `MIN_PAGES_ALWAYS_OK` pages if that
/// is more; beyond it the level is too sparse to page. Neighbouring cells of such a level sit
/// farther apart than `MAX_PAGE_EXTENT`, so each cell takes a whole page (512 slots, 8 KiB): a
/// 64 000-point leaf at cell depth 63 asks for ~500 MiB, which traps the wasm build worker.
/// Those depths are cubes thousands of times smaller than the point spacing, so nothing is lost
/// by drawing the closest servable level instead (R3-Q21).
pub const MAX_PAGE_FILL_RATIO: usize = 8;

/// A level that takes at most this many pages (128 pages = 1 MiB) is always accepted, however
/// scattered, so tiny nodes keep their deep levels.
pub const MIN_PAGES_ALWAYS_OK: usize = 128;

/// Pages [`pack_level`] makes for `cells` (in Morton order, as the builder returns them), counted with the
/// same split rule and without allocating them.
pub(crate) fn page_count(cells: &[NodeCell]) -> usize {
    let (mut pages, mut start) = (0, 0);
    let mut min = [0_u64; 3];
    let mut max = [0_u64; 3];
    for (index, cell) in cells.iter().enumerate() {
        if index > start && page_is_full(min, max, cell.coord, index - start) {
            pages += 1;
            start = index;
        }
        if index == start {
            (min, max) = (cell.coord, cell.coord);
        } else {
            for axis in 0..3 {
                min[axis] = min[axis].min(cell.coord[axis]);
                max[axis] = max[axis].max(cell.coord[axis]);
            }
        }
    }
    pages + usize::from(start < cells.len())
}

/// Whether `cells` fill their pages well enough to pack (see [`MAX_PAGE_FILL_RATIO`]).
pub(crate) fn packs_efficiently(cells: &[NodeCell]) -> bool {
    page_count(cells)
        <= MIN_PAGES_ALWAYS_OK.max(MAX_PAGE_FILL_RATIO * cells.len().div_ceil(PAGE_SLOTS))
}

#[allow(clippy::too_many_arguments)] // private helper of `pack_level`
fn close_page(
    packed: &mut PackedLevel,
    level: &NodeCellLevel,
    lo: [(u64, u64); 3],
    spans: bool,
    node_depth: u8,
    page_flags: u32,
    node_lo: Option<[u64; 3]>,
    cells: std::ops::Range<usize>,
    min: [u64; 3],
    max: [u64; 3],
) {
    let first_cell = cells.start;
    let count = cells.len();
    for cell in &level.cells[cells] {
        let mask: [u32; 3] = std::array::from_fn(|axis| {
            mask_axis(cell.coord[axis], lo[axis], level.depth, node_depth)
        });
        let flags =
            mask[0] | mask[1] << 3 | mask[2] << 6 | if spans { FLAG_SPANS_OCTANTS } else { 0 };
        let rel: [u32; 3] = std::array::from_fn(|axis| (cell.coord[axis] - min[axis]) as u32);
        packed.instances.push(CellInstance([
            rel[0] | rel[1] << 16,
            rel[2] | flags << 16,
            rgba_word(cell),
            u32::from(intensity_word(cell)) | u32::from(cell.classification as u16) << 16,
        ]));
    }
    packed.instances.resize(
        packed.instances.len() + PAGE_SLOTS - count,
        CellInstance::EMPTY,
    );
    let rel_origin: [u32; 3] = std::array::from_fn(|axis| {
        // Beyond 31 levels the offset may not fit `u32`: `MAX`, as documented on the field.
        let span = level
            .depth
            .checked_sub(node_depth)
            .filter(|span| *span <= 31);
        span.and_then(|span| {
            let corner = u128::from(node_lo?[axis]) << span;
            u32::try_from(u128::from(min[axis]).checked_sub(corner)?).ok()
        })
        .unwrap_or(u32::MAX)
    });
    packed.pages.push(PageInfo {
        node: packed.node,
        depth: packed.depth,
        origin: min,
        first_cell,
        cells: count as u32,
        box_max: std::array::from_fn(|axis| (max[axis] - min[axis]) as u16),
        flags: page_flags,
        rel_origin,
    });
}

/// sRGB byte to linear table, as the shader's uniform `array<vec4<f32>, 64>` (entry `i` is
/// component `i % 4` of vector `i / 4`), generated by `ColorRgb::to_linear_rgba`.
pub fn srgb_to_linear_table() -> [[f32; 4]; 64] {
    let mut table = [[0.0; 4]; 64];
    for byte in 0..=255_u8 {
        table[usize::from(byte) / 4][usize::from(byte) % 4] =
            ColorRgb::new(byte, 0, 0).to_linear_rgba()[0];
    }
    table
}

/// Everything the shader derives from one slot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DecodedCell {
    pub rel: [u16; 3],
    pub flags: u16,
    pub mask_index: u32,
    pub octant: u32,
    pub spans_octants: bool,
    pub intensity: u16,
    pub class: i16,
    pub lo: [f32; 3],
    pub hi: [f32; 3],
    pub linear_rgb: [f32; 3],
}

/// Rust twin of the WGSL decode; `f32::mul_add` is the explicit `fma` (a plain `a * b + c` may or
/// may not be fused by the GPU compiler).
pub fn decode_cell(
    page: &PageEntry,
    instance: &CellInstance,
    table: &[[f32; 4]; 64],
) -> DecodedCell {
    let rel = instance.rel();
    let (origin, side) = (page.origin(), page.side());
    let rgba = instance.rgba();
    let channel = |shift: u32| {
        let byte = (rgba >> shift & 0xff) as usize;
        table[byte / 4][byte % 4]
    };
    DecodedCell {
        rel,
        flags: instance.flags(),
        mask_index: instance.mask_index(),
        octant: instance.octant(),
        spans_octants: instance.spans_octants(),
        intensity: instance.intensity(),
        class: instance.class(),
        lo: std::array::from_fn(|axis| f32::from(rel[axis]).mul_add(side, origin[axis])),
        hi: std::array::from_fn(|axis| (f32::from(rel[axis]) + 1.0).mul_add(side, origin[axis])),
        linear_rgb: [channel(0), channel(8), channel(16)],
    }
}

/// WGSL twin of [`decode_cell`] for Task 7's passes. It declares the sRGB table at group 0
/// binding 0 and expects `PageEntry` as `CellPage` and `CellInstance` as `vec4<u32>`; the includer
/// adds the other bindings.
pub const CELL_PAGES_WGSL: &str = include_str!("shaders/cell_pages.wgsl");
