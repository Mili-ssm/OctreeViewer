//! Joins the cell pool with a renderer (R3 Task 9a): the glue the viewer loop needs each frame.
//!
//! [`CellDriver`] owns the [`CellPool`] and the packed levels of every node it was given
//! (`Arc<PackedLevel>`, shared with the pool, never copied). A node arrives with its cells once
//! (`add_node`); the driver packs every level, hands the exact page counts to the pool and keeps no
//! `NodeCell`. Each [`CellDriver::frame`] runs the pool, answers its requests from the stored
//! levels in the same frame, executes the plan on a [`CellTarget`] in the plan's order (frees,
//! uploads, masks, page table) and returns what happened.
//!
//! Frames of reference: the pool and the page entries use the world grid with the renderer's
//! `coordinate_origin`; the cells of a node are built in the local frame of its decoded points, on
//! the local grid. Both grids have the same side and the root anchor shifted by the origin, so the
//! integer cell coordinates are identical; `add_node` checks that and refuses a node on any other
//! grid.
//!
//! Memory: `cpu_budget_bytes` bounds the packed levels kept. Above it the driver forgets, farthest
//! first, the nodes that are not in the cut and have no level in the pool; they are reported in
//! [`FrameReport::forgotten`] and the caller sends them again if the cut needs them (the pool
//! answers an unknown node with `CellCutError::UnknownNode`).
//!
//! Lost targets: when the target says it has no pool any more (the renderer dropped it), the next
//! frame resets the pool first; device loss is `reset()`. A target error makes the frame drop the
//! target's pool (`CellTarget::drop_pool`) and schedule the reset itself, so that `reset()` always
//! meets a fresh target: mask slots from 0 and every page free.
//!
//! No clock is used, so the module builds for wasm32; [`CellDriver::frame_marked`] tells the caller
//! when each phase ended and the caller times it.

use std::{
    collections::{HashMap, HashSet},
    fmt,
    sync::Arc,
};

use viewer_core::{NodeId, SpatialBounds};

use crate::cell_lod::{LodView, MAX_NODE_SPAN, NodeLod, node_record};
use crate::cell_pages::{MISSING_INTENSITY, PAGE_HAS_INTENSITY};

use crate::{
    CellCutError, CellGrid, CellPool, CellPoolEvent, CutApply, FramePlan, LevelKey, NODE_WORDS,
    NodeCells, PAGE_SLOTS, PackedLevel, PageEntry, PageFrame, PageInfo, PoolConfig, PoolCounters,
    PoolNode, ViewProjection, node_cells::anchor_tolerance, pack_level,
};

/// The renderer side of the driver: the pool of pages, mask slots and the page table of one
/// device. Implemented by `SurfaceRenderer` and by the bench and test renderers.
pub trait CellTarget {
    type Error: std::error::Error + 'static;

    /// Whether the renderer currently has a pool. It has none before the first upload, and after
    /// it dropped it (new dataset, color mode change): the driver then resets its own pool.
    fn has_pool(&self) -> bool;

    /// Frees the pool and everything in it (page slots, mask slots, the page table), as if the
    /// target had never received a write. The driver calls it after a failed frame, before its
    /// own reset, so the two stay in step.
    fn drop_pool(&mut self);

    /// The camera eye in scaled local units (`PageFrame::eye_scaled`) of the frame being drawn.
    fn eye(&self) -> [f64; 3];

    /// Writes `level` into `page_slots`; the first call may create the pool.
    fn upload_level(&mut self, level: &PackedLevel, page_slots: &[u32]) -> Result<(), Self::Error>;

    fn free_pages(&mut self, page_slots: &[u32]) -> Result<(), Self::Error>;

    /// The next mask slot, in order from 0 after a pool is created.
    fn alloc_mask_slot(&mut self) -> Result<Option<u32>, Self::Error>;

    fn set_node_record(&mut self, slot: u32, record: &[u32; NODE_WORDS])
    -> Result<(), Self::Error>;

    /// The intensity and elevation ranges of the cut drawn this frame, for the shader colour modes
    /// (R3-Q32). Called before `set_active_pages`. A target without shader modes ignores it.
    fn set_color_ranges(&mut self, _ranges: CellColorRanges) {}

    /// The per-cell decision's view of the frame being drawn (`Some` in cell mode, `None` in
    /// octant mode). Called before `set_active_pages`. A target that cannot draw cell mode
    /// ignores it.
    fn set_lod_view(&mut self, _view: Option<LodView>) {}

    /// The page table of this frame; `frame` is the one its entries were built with.
    fn set_active_pages(
        &mut self,
        pages: &[(u32, PageEntry)],
        frame: &PageFrame,
    ) -> Result<(), Self::Error>;
}

/// Value ranges of the cells of one node or of a cut, which the shader colour modes normalise
/// with (the CPU path takes them over the voxels it draws). Read from the packed cells, so the
/// blocks are not decoded again.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CellColorRanges {
    /// Min and max of the cell mean intensities (`u16` words), cells without one left out.
    pub intensity: Option<[f32; 2]>,
    /// Min and max of the cell centre heights in absolute dataset coordinates, in `f64`.
    pub z: Option<[f64; 2]>,
}

fn merge_range<T: PartialOrd + Copy>(a: Option<[T; 2]>, b: Option<[T; 2]>) -> Option<[T; 2]> {
    match (a, b) {
        (Some(a), Some(b)) => Some([
            if b[0] < a[0] { b[0] } else { a[0] },
            if b[1] > a[1] { b[1] } else { a[1] },
        ]),
        (a, b) => a.or(b),
    }
}

impl CellColorRanges {
    #[must_use]
    pub fn merge(self, other: Self) -> Self {
        Self {
            intensity: merge_range(self.intensity, other.intensity),
            z: merge_range(self.z, other.z),
        }
    }

    /// The ranges over every cell of `levels`; `grid` is the world grid the pages were packed on.
    pub fn of_levels(grid: &CellGrid, levels: &[PackedLevel]) -> Self {
        let mut ranges = Self::default();
        for level in levels {
            let side = grid.root_side / (1_u64 << level.depth) as f64;
            for (page, slots) in level.pages.iter().zip(level.instances.chunks(PAGE_SLOTS)) {
                let cells = &slots[..page.cells as usize];
                if cells.is_empty() {
                    continue;
                }
                let rel_z = cells.iter().map(|cell| cell.rel()[2]);
                let (lo, hi) = rel_z.fold((u16::MAX, 0), |(lo, hi), z| (lo.min(z), hi.max(z)));
                let center = |rel: u16| {
                    grid.root_min[2] + ((page.origin[2] + u64::from(rel)) as f64 + 0.5) * side
                };
                ranges.z = merge_range(ranges.z, Some([center(lo), center(hi)]));
                if page.flags & PAGE_HAS_INTENSITY != 0 {
                    let words = cells
                        .iter()
                        .map(|cell| cell.intensity())
                        .filter(|word| *word != MISSING_INTENSITY);
                    let (lo, hi) = words.fold((u16::MAX, 0), |(lo, hi), w| (lo.min(w), hi.max(w)));
                    if lo <= hi {
                        ranges.intensity =
                            merge_range(ranges.intensity, Some([f32::from(lo), f32::from(hi)]));
                    }
                }
            }
        }
        ranges
    }
}

fn level_ranges(grid: &CellGrid, levels: &[PackedLevel]) -> Vec<(u8, CellColorRanges)> {
    levels
        .iter()
        .map(|level| {
            (
                level.depth,
                CellColorRanges::of_levels(grid, std::slice::from_ref(level)),
            )
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DriverConfig {
    pub pool: PoolConfig,
    /// The grid in the dataset (world) frame, as the pool and the page entries use it.
    pub grid: CellGrid,
    /// The renderer's `coordinate_origin`: local frame = world frame minus this.
    pub coordinate_origin: [f64; 3],
    /// Bytes of packed levels kept on the CPU before nodes outside the cut are forgotten.
    pub cpu_budget_bytes: u64,
}

/// The cells of a node, as the caller builds them.
pub struct NodeInput<'a> {
    pub id: NodeId,
    /// Node bounds in the world frame.
    pub bounds: SpatialBounds,
    pub points: u64,
    /// The grid the cells were built on: the world grid shifted by the coordinate origin.
    pub local_grid: CellGrid,
    pub cells: &'a NodeCells,
}

/// The outcome of [`CellDriver::add_packed_levels`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LevelsAdded {
    /// This many new depths were merged into the stored node.
    Added(usize),
    /// Nothing changed: unknown node, another or older generation, or every depth stored already.
    Ignored,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeAdded {
    /// Stored; its non-empty levels are known to the pool.
    Stored { levels: usize },
    /// The node was stored before with an older colour generation: its cells were replaced by
    /// these, with the same geometry, and rewritten over the pool pages it already holds.
    Recolored { levels: usize },
    /// The message carries a colour generation older than one already seen: dropped.
    Stale,
    /// The node was stored before by the same generation (`add_packed_generation`): the message
    /// was repeated, and nothing changed.
    AlreadyStored,
    /// Every level was empty: nothing is stored, and the node must not be in a cut.
    NoLevels,
}

#[derive(Debug, Clone, PartialEq)]
pub enum NodeInputError {
    /// The local grid is not the world grid shifted by the coordinate origin.
    GridMismatch { expected: CellGrid, found: CellGrid },
    /// The node is already stored and the new one is not a repeat of the same generation (forget
    /// happens by the budget only).
    Duplicate(NodeId),
    /// Local bounds are degenerate or the cells belong to another node.
    InvalidNode(NodeId),
    /// A packed level of another node, or two levels of one depth.
    InvalidLevel(NodeId),
}

impl fmt::Display for NodeInputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GridMismatch { expected, found } => write!(
                f,
                "cell grid {found:?} is not the world grid in the local frame {expected:?}"
            ),
            Self::Duplicate(node) => write!(f, "node {node:?} is already stored"),
            Self::InvalidNode(node) => write!(f, "invalid cells or bounds for node {node:?}"),
            Self::InvalidLevel(node) => write!(f, "invalid packed levels for node {node:?}"),
        }
    }
}

impl std::error::Error for NodeInputError {}

#[derive(Debug)]
pub enum CellDriverError<E> {
    /// `UnknownNode` (not stored, or forgotten) or `NoStoredLevels`.
    Cut(CellCutError),
    Target(E),
    /// The pool refused a level it requested (an invariant of the driver).
    OfferRefused(LevelKey),
    /// The target did not hand out the next mask slot.
    MaskSlot {
        expected: u32,
        got: Option<u32>,
    },
}

impl<E: fmt::Display> fmt::Display for CellDriverError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cut(error) => write!(f, "cell cut: {error}"),
            Self::Target(error) => write!(f, "cell target: {error}"),
            Self::OfferRefused(key) => write!(f, "the pool refused the level {key:?}"),
            Self::MaskSlot { expected, got } => {
                write!(f, "mask slot {expected} expected, the target gave {got:?}")
            }
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for CellDriverError<E> {}

/// The ends of the phases of [`CellDriver::frame_marked`], in the order they happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FramePhase {
    /// The pool planned the frame (cut + fit).
    Planned,
    /// The requested levels were offered and the plan's events applied.
    Offered,
    /// Frees, uploads and node masks are written.
    Written,
    /// The page entries of the active pages are built.
    TableBuilt,
    /// `set_active_pages` returned (or was skipped).
    ActivePagesSet,
}

/// What one frame did, for the caller to show.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameReport {
    pub plan: FramePlan,
    /// Events of the reset the frame ran first, when the target had lost its pool.
    pub reset_events: Vec<CellPoolEvent>,
    pub uploaded_pages: usize,
    pub uploaded_levels: usize,
    pub freed_pages: usize,
    pub evicted: usize,
    pub cancelled: usize,
    pub requested: usize,
    pub active_pages: usize,
    /// Nodes dropped to stay within the CPU budget; send their cells again to use them.
    pub forgotten: Vec<NodeId>,
    /// The target had lost its pool and the driver reset it before planning (the pool draws
    /// only what this frame uploads).
    pub pool_reset: bool,
}

impl FrameReport {
    pub fn counters(&self) -> &PoolCounters {
        &self.plan.counters
    }
}

/// Bits of a cell generation that hold the colour generation; the dataset generation sits above
/// them. One number keeps the wire format and stays exact as a JS number.
const COLOR_BITS: u32 = 20;
const COLOR_MASK: u64 = (1 << COLOR_BITS) - 1;

/// The generation stamped on node messages: the dataset's build in the high bits and the colour
/// generation in the low ones (R3-Q33). Cells painted with other colours get a larger one;
/// the colour generation saturates, as it would take a million colour changes to reach it.
pub fn cell_generation(dataset: u64, color: u32) -> u64 {
    (dataset << COLOR_BITS) | u64::from(color).min(COLOR_MASK)
}

fn dataset_of(generation: u64) -> u64 {
    generation >> COLOR_BITS
}

fn color_of(generation: u64) -> u64 {
    generation & COLOR_MASK
}

struct StoredNode {
    pool: PoolNode,
    /// Ranges of each level by depth, read once when it is stored: a frame merges only the
    /// levels it draws (R3-Q32: the CPU path ranges over the voxels it draws).
    colors: Vec<(u8, CellColorRanges)>,
    /// One per depth of `pool.stored`.
    levels: Vec<Arc<PackedLevel>>,
    bytes: u64,
    /// Levels offered to the pool and not evicted or cancelled yet.
    live: u32,
    /// The generation of the message that brought it, when it came with one.
    generation: Option<u64>,
}

pub struct CellDriver {
    config: DriverConfig,
    pool: CellPool,
    nodes: HashMap<NodeId, StoredNode>,
    cpu_bytes: u64,
    /// Pool page slot of each page of a level that is uploading or resident.
    slots: HashMap<LevelKey, Vec<u32>>,
    masks_allocated: u32,
    /// What each written mask slot needs to rebuild its node record when the eye moves.
    node_slots: HashMap<u32, NodeSlot>,
    /// The frame the anchors of `node_slots` were last written with.
    anchor_frame: Option<([f64; 3], [f64; 3], f64)>,
    /// The mode the records of `node_slots` were last written in.
    records_mode: Option<crate::LodMode>,
    /// The target received writes since the last reset, so a missing pool there means loss.
    armed: bool,
    /// A target call failed: the target state is unknown until the next reset.
    failed: bool,
    /// Nodes were added or the pool was reset since the last frame.
    dirty: bool,
    busy: bool,
    /// Levels the last frame wanted that are buildable but not stored, and the set last handed
    /// out by [`CellDriver::take_level_requests`].
    level_requests: Vec<(NodeId, u8)>,
    level_requests_taken: Vec<(NodeId, u8)>,
    /// Nodes added since the last cut was latched ([`CellDriver::cut_received`]): the cut being
    /// drawn was sent before they arrived, so `forget` spares them until the next cut.
    arrived_since_cut: HashSet<NodeId>,
    /// Nodes the sender needs stored although no cut names them (ancestors of cut nodes still
    /// being built, R3-Q22): `forget` spares them. Replaced by every cut ([`CellDriver::set_support`]).
    support: HashSet<NodeId>,
    /// The newest cell generation any message brought: a node of an older colour generation of
    /// the same dataset is dropped, and one stored with an older one is forgotten first.
    newest_generation: Option<u64>,
}

/// The last record of a mask slot and the node corner it needs to follow the eye.
struct NodeSlot {
    node: NodeLod,
    /// Node corner in dataset units: `root_min + node_lo * side_at_node_depth`.
    corner: [f64; 3],
}

/// Bytes of a packed level on the CPU.
fn level_bytes(level: &PackedLevel) -> u64 {
    (level.instances.len() * size_of::<crate::CellInstance>()
        + level.pages.len() * size_of::<PageInfo>()) as u64
}

/// Bytes the driver keeps on the CPU for `node`: what `cpu_bytes` grows by when it stores it. The
/// build side measures nodes with it, so its CPU budget and the driver's agree.
pub fn packed_node_bytes(node: &PackedNode) -> u64 {
    node.levels
        .iter()
        .filter(|level| !level.pages.is_empty())
        .map(level_bytes)
        .sum()
}

fn shift(grid: &CellGrid, origin: [f64; 3]) -> CellGrid {
    CellGrid {
        root_min: std::array::from_fn(|axis| grid.root_min[axis] - origin[axis]),
        root_side: grid.root_side,
    }
}

/// A node with its levels packed, ready for [`CellDriver::add_packed_generation`] or the codec.
#[derive(Debug, Clone, PartialEq)]
pub struct PackedNode {
    pub id: NodeId,
    /// Node bounds in the world frame.
    pub bounds: SpatialBounds,
    pub points: u64,
    /// Depths the node can build on demand, inclusive (`NodeCells::buildable`).
    pub buildable: (u8, u8),
    /// Non-empty levels only.
    pub levels: Vec<PackedLevel>,
}

fn check_grid(world: &CellGrid, origin: [f64; 3], found: CellGrid) -> Result<(), NodeInputError> {
    // The side is not shifted, so it must be identical. The anchor only differs by the
    // rounding of `world - origin` (half an ulp of the larger operand) and of the sum back.
    let same = found.root_side == world.root_side
        && (0..3).all(|axis| {
            let scale = world.root_min[axis].abs().max(origin[axis].abs());
            (found.root_min[axis] + origin[axis] - world.root_min[axis]).abs()
                <= anchor_tolerance(scale)
        });
    if same {
        Ok(())
    } else {
        Err(NodeInputError::GridMismatch {
            expected: shift(world, origin),
            found,
        })
    }
}

/// Packs the non-empty levels of a node built in the local frame, without a driver, so the build
/// worker can do it. `grid` and `coordinate_origin` are the driver's `DriverConfig` ones; the
/// grid contract is checked as in [`CellDriver::add_node`]. The pages hold the integer cell
/// coordinates of the world grid (the local grid gives the same integers).
pub fn pack_node(
    grid: &CellGrid,
    coordinate_origin: [f64; 3],
    input: NodeInput<'_>,
) -> Result<PackedNode, NodeInputError> {
    check_grid(grid, coordinate_origin, input.local_grid)?;
    pack_checked(grid, coordinate_origin, input)
}

/// [`pack_node`] after the caller checked the grid contract.
fn pack_checked(
    grid: &CellGrid,
    coordinate_origin: [f64; 3],
    input: NodeInput<'_>,
) -> Result<PackedNode, NodeInputError> {
    let NodeInput {
        id,
        bounds,
        points,
        cells,
        ..
    } = input;
    if cells.node != id {
        return Err(NodeInputError::InvalidNode(id));
    }
    let local = SpatialBounds::new(
        std::array::from_fn(|axis| bounds.min()[axis] - coordinate_origin[axis]),
        std::array::from_fn(|axis| bounds.max()[axis] - coordinate_origin[axis]),
    )
    .map_err(|_| NodeInputError::InvalidNode(id))?;
    // Packed on the driver's own local grid (validated above, equal to `local_grid` up to the
    // rounding of the shift).
    let packing_grid = shift(grid, coordinate_origin);
    let levels = cells
        .levels
        .iter()
        .filter(|level| !level.cells.is_empty())
        .map(|level| pack_level(&packing_grid, local, id, level))
        .collect();
    Ok(PackedNode {
        id,
        bounds,
        points,
        buildable: cells.buildable,
        levels,
    })
}

/// Distance in dataset units from `point` to the box.
fn distance_to(point: [f64; 3], bounds: SpatialBounds) -> f64 {
    (0..3)
        .map(|axis| {
            let d = point[axis].clamp(bounds.min()[axis], bounds.max()[axis]) - point[axis];
            d * d
        })
        .sum::<f64>()
        .sqrt()
}

impl CellDriver {
    pub fn new(config: DriverConfig) -> Self {
        Self {
            pool: CellPool::new(config.pool),
            config,
            nodes: HashMap::new(),
            cpu_bytes: 0,
            slots: HashMap::new(),
            masks_allocated: 0,
            node_slots: HashMap::new(),
            anchor_frame: None,
            records_mode: None,
            armed: false,
            failed: false,
            dirty: false,
            busy: false,
            level_requests: Vec::new(),
            level_requests_taken: Vec::new(),
            arrived_since_cut: HashSet::new(),
            support: HashSet::new(),
            newest_generation: None,
        }
    }

    pub fn pool(&self) -> &CellPool {
        &self.pool
    }

    /// Pages of the pool this driver fits cuts to (`PoolConfig::capacity_pages`).
    pub fn pool_capacity_pages(&self) -> u32 {
        self.config.pool.capacity_pages
    }

    /// Continuity (RF-05): the smallest cell side the pool draws, or `None` for the literal mode.
    /// Takes effect on the next frame; no node is rebuilt or re-sent ([`CellPool::set_min_side`]).
    pub fn set_min_side(&mut self, min_side: Option<f64>) {
        if self.pool.min_side() != min_side {
            self.pool.set_min_side(min_side);
            self.dirty = true;
        }
    }

    /// Per-octant or per-cell level selection (ADR 0050). Takes effect on the next frame; the
    /// pool re-plans and re-sends every node's mask record ([`CellPool::set_lod_mode`]).
    pub fn set_lod_mode(&mut self, mode: crate::LodMode) {
        if self.pool.lod_mode() != mode {
            self.pool.set_lod_mode(mode);
            self.dirty = true;
        }
    }

    /// Bytes of packed levels kept.
    pub fn cpu_bytes(&self) -> u64 {
        self.cpu_bytes
    }

    pub fn has_node(&self, node: NodeId) -> bool {
        self.nodes.contains_key(&node)
    }

    /// Packs the levels of a node built in the local frame (see [`pack_node`]) and stores them.
    /// The grid contract is checked here.
    pub fn add_node(&mut self, input: NodeInput<'_>) -> Result<NodeAdded, NodeInputError> {
        check_grid(
            &self.config.grid,
            self.config.coordinate_origin,
            input.local_grid,
        )?;
        if self.nodes.contains_key(&input.id) {
            return Err(NodeInputError::Duplicate(input.id));
        }
        let node = pack_checked(&self.config.grid, self.config.coordinate_origin, input)?;
        self.store_packed(
            None,
            node.id,
            node.bounds,
            node.points,
            node.buildable,
            node.levels,
        )
    }

    /// Stores a node whose levels are already packed. The pages hold integer cell coordinates of
    /// `DriverConfig::grid` (the local grid gives the same integers); nothing here can check that,
    /// so the caller packs on that grid and `bounds` are in the world frame. Empty levels are
    /// dropped; a node left without levels is not stored.
    pub fn add_packed(
        &mut self,
        id: NodeId,
        bounds: SpatialBounds,
        points: u64,
        levels: Vec<PackedLevel>,
    ) -> Result<NodeAdded, NodeInputError> {
        let mut depths = levels
            .iter()
            .filter(|level| !level.pages.is_empty())
            .map(|level| level.depth);
        let first = depths.next().unwrap_or(0);
        let buildable = depths.fold((first, first), |(low, high), depth| {
            (low.min(depth), high.max(depth))
        });
        self.store_packed(None, id, bounds, points, buildable, levels)
    }

    /// [`add_packed`](Self::add_packed) for nodes that arrive in messages, which may repeat.
    ///
    /// `generation` is the dataset generation the sender stamps on its `nodeCells` messages. It is
    /// not the pool's `LevelKey::session` / `color_gen`: those are fixed per driver and say which
    /// pool a level lives in, while the generation says which build of the dataset a node comes
    /// from (a new dataset or builder gets a new one). A node already stored with the same
    /// generation is a repeated message and is ignored ([`NodeAdded::AlreadyStored`]). The same id
    /// with another generation, or stored without one, is `Duplicate`: replacing it would leave
    /// the pool with levels of the old build, so the caller replaces the driver
    /// (`driverReplaced`) instead. A repeat is assumed to carry the same content (the build is
    /// deterministic per generation) and is not compared.
    ///
    /// The generation is a [`cell_generation`]: the same dataset with a larger colour generation
    /// replaces the stored node in place (R3-Q33: same cells, other colours, the pool rewrites
    /// the pages it holds), and one with a smaller colour generation than already seen is
    /// dropped ([`NodeAdded::Stale`]).
    pub fn add_packed_generation(
        &mut self,
        generation: u64,
        node: PackedNode,
    ) -> Result<NodeAdded, NodeInputError> {
        self.store_packed(
            Some(generation),
            node.id,
            node.bounds,
            node.points,
            node.buildable,
            node.levels,
        )
    }

    /// Adds levels built on demand to a stored node: `levels.levels` holds only the new depths,
    /// merged in depth order next to the ones already stored (their pool sizes and CPU bytes
    /// included). `Ignored` when the node is unknown (forgotten, or never stored), when the
    /// generation is not the one the node was stored with or its colour is older than one already
    /// seen, and when every depth is stored already.
    pub fn add_packed_levels(
        &mut self,
        generation: u64,
        levels: PackedNode,
    ) -> Result<LevelsAdded, NodeInputError> {
        let PackedNode { id, mut levels, .. } = levels;
        levels.retain(|level| !level.pages.is_empty());
        levels.sort_by_key(|level| level.depth);
        if levels.iter().any(|level| level.node != id)
            || levels.windows(2).any(|pair| pair[0].depth == pair[1].depth)
        {
            return Err(NodeInputError::InvalidLevel(id));
        }
        let outdated = self.newest_generation.is_some_and(|newest| {
            dataset_of(newest) == dataset_of(generation) && color_of(generation) < color_of(newest)
        });
        let Some(stored) = self.nodes.get_mut(&id) else {
            return Ok(LevelsAdded::Ignored);
        };
        if outdated || stored.generation != Some(generation) {
            return Ok(LevelsAdded::Ignored);
        }
        levels.retain(|level| !stored.pool.stored.contains(&level.depth));
        if levels.is_empty() {
            return Ok(LevelsAdded::Ignored);
        }
        let sizes: Vec<(u8, usize)> = levels
            .iter()
            .map(|level| (level.depth, level.pages.len()))
            .collect();
        self.pool.set_level_sizes(id, &sizes);
        let added = levels.len();
        let colors = level_ranges(&self.config.grid, &levels);
        for (level, color) in levels.into_iter().zip(colors) {
            let bytes = level_bytes(&level);
            let at = stored
                .pool
                .stored
                .partition_point(|depth| *depth < level.depth);
            stored.pool.stored.insert(at, level.depth);
            stored.colors.insert(at, color);
            let level = Arc::new(level);
            // The pool may still hold this depth with the colours of before a recolour; its new
            // colours replace them (a vacant key is requested from `stored` as usual). A level
            // with another page count cannot be refreshed and stays until it is evicted.
            let _ = self
                .pool
                .refresh_level(self.pool.key(id, level.depth), Arc::clone(&level));
            stored.levels.insert(at, level);
            stored.bytes += bytes;
            self.cpu_bytes += bytes;
        }
        self.dirty = true;
        self.arrived_since_cut.insert(id);
        Ok(LevelsAdded::Added(added))
    }

    fn store_packed(
        &mut self,
        generation: Option<u64>,
        id: NodeId,
        bounds: SpatialBounds,
        points: u64,
        buildable: (u8, u8),
        mut levels: Vec<PackedLevel>,
    ) -> Result<NodeAdded, NodeInputError> {
        let same_dataset = |other: Option<u64>| match (generation, other) {
            (Some(a), Some(b)) => dataset_of(a) == dataset_of(b),
            _ => false,
        };
        if let (Some(new), Some(newest)) = (generation, self.newest_generation)
            && same_dataset(Some(newest))
            && color_of(new) < color_of(newest)
        {
            return Ok(NodeAdded::Stale);
        }
        let stored_generation = self.nodes.get(&id).map(|stored| stored.generation);
        if let Some(stored) = stored_generation {
            let recolor = same_dataset(stored)
                && matches!((generation, stored), (Some(new), Some(old)) if color_of(new) > color_of(old));
            if !recolor {
                return if generation.is_some() && stored == generation {
                    Ok(NodeAdded::AlreadyStored)
                } else {
                    Err(NodeInputError::Duplicate(id))
                };
            }
        }
        levels.retain(|level| !level.pages.is_empty());
        levels.sort_by_key(|level| level.depth);
        if levels.iter().any(|level| level.node != id)
            || levels.windows(2).any(|pair| pair[0].depth == pair[1].depth)
        {
            return Err(NodeInputError::InvalidLevel(id));
        }
        if stored_generation.is_some() {
            return self.recolor_node(id, generation, buildable, levels);
        }
        self.newest_generation = self.newest_generation.max(generation);
        if levels.is_empty() {
            return Ok(NodeAdded::NoLevels);
        }
        let sizes: Vec<(u8, usize)> = levels
            .iter()
            .map(|level| (level.depth, level.pages.len()))
            .collect();
        self.pool.set_level_sizes(id, &sizes);
        let bytes = levels.iter().map(level_bytes).sum::<u64>();
        let count = levels.len();
        self.cpu_bytes += bytes;
        let colors = level_ranges(&self.config.grid, &levels);
        self.nodes.insert(
            id,
            StoredNode {
                colors,
                pool: PoolNode {
                    bounds,
                    stored: levels.iter().map(|level| level.depth).collect(),
                    buildable,
                    points,
                },
                levels: levels.into_iter().map(Arc::new).collect(),
                bytes,
                live: 0,
                generation,
            },
        );
        self.dirty = true;
        self.arrived_since_cut.insert(id);
        Ok(NodeAdded::Stored { levels: count })
    }

    /// Replaces the cells of a stored node with the same cells in other colours. The geometry
    /// must be the same (same depths, same pages per depth): the pool keeps its slots and page
    /// table and only rewrites the pages, so the node never leaves the image.
    fn recolor_node(
        &mut self,
        id: NodeId,
        generation: Option<u64>,
        buildable: (u8, u8),
        levels: Vec<PackedLevel>,
    ) -> Result<NodeAdded, NodeInputError> {
        // Depths built on demand are not in the incoming node: they keep the old colours, so they
        // go, and the cut asks for them again. Checked on the kept ones first: a refused
        // recolour changes nothing.
        let stored = &self.nodes[&id];
        let kept: Vec<usize> = (0..stored.levels.len())
            .filter(|at| {
                levels
                    .iter()
                    .any(|new| new.depth == stored.levels[*at].depth)
            })
            .collect();
        let same_shape = levels.len() == kept.len()
            && levels.iter().zip(&kept).all(|(new, at)| {
                let old = &stored.levels[*at];
                new.depth == old.depth && new.pages.len() == old.pages.len()
            });
        if !same_shape {
            return Err(NodeInputError::InvalidLevel(id));
        }
        if kept.len() < stored.levels.len() {
            let gone: Vec<u8> = (0..stored.levels.len())
                .filter(|at| !kept.contains(at))
                .map(|at| stored.levels[at].depth)
                .collect();
            let stored = self.nodes.get_mut(&id).expect("stored");
            let mut at = 0;
            let mut dropped = 0;
            stored.levels.retain(|level| {
                at += 1;
                let keep = kept.contains(&(at - 1));
                if !keep {
                    dropped += level_bytes(level);
                }
                keep
            });
            let mut at = 0;
            stored.pool.stored.retain(|_| {
                at += 1;
                kept.contains(&(at - 1))
            });
            let mut at = 0;
            stored.colors.retain(|_| {
                at += 1;
                kept.contains(&(at - 1))
            });
            stored.bytes -= dropped;
            self.cpu_bytes -= dropped;
            let remaining = stored.pool.stored.clone();
            self.pool.reconcile_cap(id, &remaining);
            // A resident copy would keep the old colours under the same key. One the cut draws
            // or the pool pins stays (no coarsening while a level waits, R3-Q21): it is replaced
            // when the depth returns (`add_packed_levels`).
            for depth in gone {
                let _ = self.pool.evict(self.pool.key(id, depth));
            }
        }
        let bytes = levels.iter().map(level_bytes).sum::<u64>();
        let count = levels.len();
        let colors = level_ranges(&self.config.grid, &levels);
        let levels: Vec<Arc<PackedLevel>> = levels.into_iter().map(Arc::new).collect();
        for level in &levels {
            let refreshed = self
                .pool
                .refresh_level(self.pool.key(id, level.depth), Arc::clone(level));
            debug_assert!(refreshed, "page counts were checked");
        }
        let stored = self.nodes.get_mut(&id).expect("stored");
        self.cpu_bytes = self.cpu_bytes - stored.bytes + bytes;
        stored.bytes = bytes;
        stored.pool.buildable = buildable;
        stored.levels = levels;
        stored.colors = colors;
        stored.generation = generation;
        self.newest_generation = self.newest_generation.max(generation);
        self.dirty = true;
        self.arrived_since_cut.insert(id);
        Ok(NodeAdded::Recolored { levels: count })
    }

    /// Empties the pool and the bookkeeping of the target (device lost, or the renderer dropped
    /// its pool): the stored levels stay and the next frames request them again. Returns the
    /// events of the levels that left.
    #[must_use]
    pub fn reset(&mut self) -> Vec<CellPoolEvent> {
        let events = self.pool.reset();
        self.slots.clear();
        for node in self.nodes.values_mut() {
            node.live = 0;
        }
        self.masks_allocated = 0;
        self.node_slots.clear();
        self.anchor_frame = None;
        self.records_mode = None;
        self.armed = false;
        self.failed = false;
        self.dirty = true;
        events
    }

    /// The levels to build on demand (wanted by the last frame, buildable, not stored), coarse
    /// to fine, only when the set changed since the last call. A level that arrives through
    /// `add_packed_levels` leaves the set on the next frame.
    #[must_use]
    pub fn take_level_requests(&mut self) -> Option<Vec<(NodeId, u8)>> {
        // The order is the request priority and moves with the camera: only the set counts.
        let sorted = |requests: &[(NodeId, u8)]| {
            let mut sorted = requests.to_vec();
            sorted.sort_unstable();
            sorted
        };
        if sorted(&self.level_requests) == sorted(&self.level_requests_taken) {
            return None;
        }
        self.level_requests_taken.clone_from(&self.level_requests);
        Some(self.level_requests.clone())
    }

    /// Depths the build side cannot build (`node_cell_level` refused them): the node's buildable
    /// maximum drops below each, so the cut stops wanting it and no hold waits for it.
    pub fn refuse_levels(&mut self, refused: &[(NodeId, u8)]) {
        for (id, depth) in refused {
            let Some(node) = self.nodes.get_mut(id) else {
                continue;
            };
            let buildable = &mut node.pool.buildable;
            if *depth > buildable.0 && *depth <= buildable.1 {
                buildable.1 = *depth - 1;
                self.dirty = true;
            }
        }
    }

    /// Whether another frame has something to do: new nodes, requests, uploads in flight, the
    /// previous cut retained or nodes waiting outside the cut or held back. The viewer draws while this is true. Under a permanent pool overflow
    /// (the cut does not fit even coarsened) the pool keeps retrying and this stays true; the
    /// caller stops on `counters().pool_overflow > 0`. After a failed frame it stays true until a
    /// frame succeeds, so a caller looping on it must also stop on errors that keep repeating.
    pub fn needs_frame(&self) -> bool {
        self.dirty || self.busy || self.failed
    }

    /// A new cut was received (a `cellCut` message): nodes added before it were sent before it
    /// and are subject to `forget`; nodes added after it are spared until the next cut. `frame`
    /// and `frame_marked` latch the cut themselves (their cut comes with the call), so only a
    /// caller of [`CellDriver::frame_latched`] needs this.
    pub fn cut_received(&mut self) {
        self.arrived_since_cut.clear();
    }

    /// The nodes the sender wants kept besides the cut: ancestors that parents-first (R3-Q22)
    /// built ahead of the cut, whose descendants are still being built. Forgetting one would
    /// make the sender build it again before those descendants, so `forget` spares them. The
    /// last cut's set replaces the earlier one (it travels with the `cellCut` message).
    pub fn set_support(&mut self, nodes: &[NodeId]) {
        self.support.clear();
        self.support.extend(nodes.iter().copied());
    }

    /// A node stored with another dataset generation than that of `generation` (or with none): the driver was
    /// filled by another dataset, so the caller must replace it (`driverReplaced`).
    pub fn foreign_node(&self, generation: u64) -> Option<NodeId> {
        self.nodes
            .iter()
            .filter(|(_, node)| node.generation.map(dataset_of) != Some(dataset_of(generation)))
            .map(|(id, _)| *id)
            .min()
    }

    /// [`needs_frame`](Self::needs_frame) also true while `target` has lost the pool the driver
    /// wrote to: the next frame resets it, and until then the drawn image is gone. A caller that
    /// decides whether the view is at rest must use this one.
    pub fn needs_frame_on<T: CellTarget>(&self, target: &T) -> bool {
        self.needs_frame() || (self.armed && !target.has_pool())
    }

    /// One frame of the viewer loop, without drawing. See the module doc for the order.
    pub fn frame<T: CellTarget>(
        &mut self,
        target: &mut T,
        cut: &[NodeId],
        view: &ViewProjection,
        target_px: f64,
    ) -> Result<FrameReport, CellDriverError<T::Error>> {
        self.frame_marked(target, cut, view, target_px, &mut |_| {})
    }

    /// [`CellDriver::frame`] calling `mark` as each [`FramePhase`] ends, so the caller can time
    /// the phases with its own clock.
    pub fn frame_marked<T: CellTarget>(
        &mut self,
        target: &mut T,
        cut: &[NodeId],
        view: &ViewProjection,
        target_px: f64,
        mark: &mut dyn FnMut(FramePhase),
    ) -> Result<FrameReport, CellDriverError<T::Error>> {
        self.cut_received();
        self.frame_latched(target, cut, view, target_px, mark)
    }

    /// [`CellDriver::frame_marked`] for a cut latched earlier with [`CellDriver::cut_received`]:
    /// nodes that arrived after that call are not forgotten, so a node is not dropped before the
    /// first cut that names it.
    pub fn frame_latched<T: CellTarget>(
        &mut self,
        target: &mut T,
        cut: &[NodeId],
        view: &ViewProjection,
        target_px: f64,
        mark: &mut dyn FnMut(FramePhase),
    ) -> Result<FrameReport, CellDriverError<T::Error>> {
        let lost = self.failed || (self.armed && !target.has_pool());
        let reset_events = if lost { self.reset() } else { Vec::new() };
        let nodes = &self.nodes;
        let plan = self
            .pool
            .frame(
                cut,
                &|id| nodes.get(&id).map(|node| node.pool.clone()),
                view,
                &self.config.grid,
                self.config.coordinate_origin,
                target_px,
            )
            .map_err(CellDriverError::Cut)?;
        mark(FramePhase::Planned);
        let mut report = FrameReport {
            pool_reset: lost,
            reset_events,
            uploaded_pages: 0,
            uploaded_levels: 0,
            freed_pages: plan.page_frees.len(),
            evicted: 0,
            cancelled: 0,
            requested: plan.fetch_requests.len(),
            active_pages: 0,
            forgotten: Vec::new(),
            plan,
        };
        let mut refused = None;
        for key in &report.plan.fetch_requests {
            let level = self.nodes.get(&key.node).and_then(|node| {
                let at = node.pool.stored.iter().position(|d| *d == key.depth)?;
                Some(Arc::clone(&node.levels[at]))
            });
            // Counted only once the pool holds the level, so a refusal leaves the node idle.
            let offered = level.is_some_and(|level| self.pool.offer(*key, level));
            if !offered {
                refused = Some(*key);
                break;
            }
            if let Some(node) = self.nodes.get_mut(&key.node) {
                node.live += 1;
            }
        }
        for event in &report.plan.events {
            let key = match event {
                CellPoolEvent::Evict(key) => {
                    report.evicted += 1;
                    key
                }
                CellPoolEvent::Cancel(key) => {
                    report.cancelled += 1;
                    key
                }
            };
            self.slots.remove(key);
            if let Some(node) = self.nodes.get_mut(&key.node) {
                node.live = node.live.saturating_sub(1);
            }
        }
        mark(FramePhase::Offered);
        if let Some(key) = refused {
            // The pool has already planned this frame and the target will not see it: fail the
            // frame like a target error so the next one starts from a fresh target.
            target.drop_pool();
            self.failed = true;
            return Err(CellDriverError::OfferRefused(key));
        }
        if let Err(error) = self.execute(target, view, target_px, &mut report, mark) {
            // The target kept part of this frame: empty it so the reset of the next frame meets
            // the fresh target it assumes.
            target.drop_pool();
            self.failed = true;
            return Err(error);
        }
        let plan = &report.plan;
        self.busy = !plan.fetch_requests.is_empty()
            || !plan.uploads.is_empty()
            || plan.apply == CutApply::RetainedPrevious
            // Nodes waiting outside the cut or held back (R3-Q20/Q21) enter on a later frame.
            || plan.counters.excluded_nodes > 0
            || plan.counters.held_nodes > 0;
        self.level_requests = plan
            .level_requests
            .iter()
            .map(|key| (key.node, key.depth))
            .collect();
        self.dirty = false;
        report.forgotten = self.forget(cut, view);
        Ok(report)
    }

    /// The writes of the plan, in its order: frees, uploads, masks, page table.
    fn execute<T: CellTarget>(
        &mut self,
        target: &mut T,
        view: &ViewProjection,
        target_px: f64,
        report: &mut FrameReport,
        mark: &mut dyn FnMut(FramePhase),
    ) -> Result<(), CellDriverError<T::Error>> {
        let plan = &report.plan;
        let target_err = CellDriverError::Target;
        if !plan.page_frees.is_empty() {
            target.free_pages(&plan.page_frees).map_err(target_err)?;
        }
        for upload in &plan.uploads {
            let level = self
                .pool
                .level(upload.key)
                .expect("an uploaded level has data");
            let table = self
                .slots
                .entry(upload.key)
                .or_insert_with(|| vec![u32::MAX; level.pages.len()]);
            table[upload.page_range.clone()].copy_from_slice(&upload.slots);
            if upload.page_range.len() == level.pages.len() {
                target.upload_level(level, &upload.slots)
            } else {
                let part = PackedLevel {
                    node: level.node,
                    depth: level.depth,
                    instances: level.instances
                        [upload.page_range.start * PAGE_SLOTS..upload.page_range.end * PAGE_SLOTS]
                        .to_vec(),
                    pages: level.pages[upload.page_range.clone()].to_vec(),
                };
                target.upload_level(&part, &upload.slots)
            }
            .map_err(CellDriverError::Target)?;
            self.armed = true;
            report.uploaded_pages += upload.page_range.len();
            report.uploaded_levels += 1;
        }
        let frame = PageFrame {
            grid: self.config.grid,
            coordinate_origin: self.config.coordinate_origin,
            visual_scale: view.visual_scale(),
            eye_scaled: target.eye(),
        };
        let stamp = (
            frame.eye_scaled,
            frame.coordinate_origin,
            frame.visual_scale,
        );
        // The anchors of the live slots follow the frame like the page origins do: a changed eye,
        // origin or scale rewrites them below, after the records of this plan.
        let anchors_stale = self.anchor_frame != Some(stamp);
        // A mode switch rewrites every record: a retained frame sends no `MaskUpdate`, and the
        // records of the other mode hold other active bits.
        let mode = self.pool.lod_mode();
        let mode_changed = self.records_mode != Some(mode);
        // In cell mode the shader's active bits are the depths this frame emits for the node
        // (R3-Q21: a retained frame rebuilds the active list without a `MaskUpdate`).
        let cell_mode = mode == crate::LodMode::Cell;
        let mut emitted: HashMap<u32, Vec<u8>> = HashMap::new();
        if cell_mode {
            for active in &plan.active {
                emitted
                    .entry(active.mask_slot)
                    .or_default()
                    .push(active.key.depth);
            }
        }
        let emitted_bits = |slot: u32, node_depth: u8| {
            emitted.get(&slot).map_or(0, |depths| {
                depths
                    .iter()
                    .filter(|depth| **depth >= node_depth && **depth - node_depth <= MAX_NODE_SPAN)
                    .fold(0u64, |bits, depth| bits | 1 << (depth - node_depth))
            })
        };
        // A slot the pool freed holds no node: it needs no anchor until a node takes it again.
        for slot in self.pool.free_mask_slots() {
            self.node_slots.remove(&slot);
        }
        for update in &plan.mask_updates {
            self.ensure_mask(target, update.slot)?;
            let grid = &self.config.grid;
            let side = grid.root_side / (1_u64 << update.node_depth) as f64;
            let slot = NodeSlot {
                node: NodeLod {
                    anchor: [0.0; 3],
                    node_depth: update.node_depth,
                    active: if cell_mode {
                        emitted_bits(update.slot, update.node_depth)
                    } else {
                        update.active
                    },
                    mask: update.mask,
                },
                corner: std::array::from_fn(|axis| {
                    grid.root_min[axis] + update.node_lo[axis] as f64 * side
                }),
            };
            self.node_slots.insert(update.slot, slot);
            self.write_record(target, update.slot, &frame)?;
        }
        {
            let rewritten: HashSet<u32> = plan.mask_updates.iter().map(|u| u.slot).collect();
            let mut slots: Vec<u32> = self
                .node_slots
                .keys()
                .copied()
                .filter(|slot| !rewritten.contains(slot))
                .collect();
            slots.sort_unstable();
            for slot in slots {
                let state = self.node_slots.get_mut(&slot).expect("a live slot");
                let active = if cell_mode {
                    emitted_bits(slot, state.node.node_depth)
                } else {
                    state.node.active
                };
                if anchors_stale || mode_changed || active != state.node.active {
                    state.node.active = active;
                    self.write_record(target, slot, &frame)?;
                }
            }
        }
        self.records_mode = Some(mode);
        self.anchor_frame = Some(stamp);
        if let Some(max) = plan.active.iter().map(|active| active.mask_slot).max() {
            self.ensure_mask(target, max)?;
        }
        mark(FramePhase::Written);
        let mut table: Vec<(u32, PageEntry)> = Vec::new();
        for active in &plan.active {
            let level = self
                .pool
                .level(active.key)
                .expect("an active level has data");
            for (page, slot) in level.pages.iter().zip(&self.slots[&active.key]) {
                table.push((
                    *slot,
                    page.entry(&frame, active.mask_slot, u32::from(active.octant_bits)),
                ));
            }
        }
        report.active_pages = table.len();
        let colors = plan
            .active
            .iter()
            .filter_map(|active| {
                let node = self.nodes.get(&active.key.node)?;
                node.colors
                    .iter()
                    .find(|(depth, _)| *depth == active.key.depth)
                    .map(|(_, ranges)| *ranges)
            })
            .fold(CellColorRanges::default(), CellColorRanges::merge);
        mark(FramePhase::TableBuilt);
        // Nothing resident and no pool yet: there is no page table to write.
        if !table.is_empty() || target.has_pool() {
            target.set_color_ranges(colors);
            target.set_lod_view(cell_mode.then(|| {
                let min_side = self
                    .pool
                    .min_side()
                    .map_or(0.0, |side| side * view.visual_scale());
                let [width, height] = view.viewport();
                LodView::from_camera(
                    &view.camera(),
                    [width as f32, height as f32],
                    target_px as f32,
                    min_side as f32,
                )
            }));
            target
                .set_active_pages(&table, &frame)
                .map_err(CellDriverError::Target)?;
        }
        mark(FramePhase::ActivePagesSet);
        Ok(())
    }

    /// Writes the record of `slot` with its anchor in `frame`, computed like a page origin.
    fn write_record<T: CellTarget>(
        &mut self,
        target: &mut T,
        slot: u32,
        frame: &PageFrame,
    ) -> Result<(), CellDriverError<T::Error>> {
        let state = self
            .node_slots
            .get_mut(&slot)
            .expect("a slot with a record");
        state.node.anchor = std::array::from_fn(|axis| {
            ((state.corner[axis] - frame.coordinate_origin[axis]) * frame.visual_scale
                - frame.eye_scaled[axis]) as f32
        });
        target
            .set_node_record(slot, &node_record(&state.node))
            .map_err(CellDriverError::Target)
    }

    fn ensure_mask<T: CellTarget>(
        &mut self,
        target: &mut T,
        slot: u32,
    ) -> Result<(), CellDriverError<T::Error>> {
        while self.masks_allocated <= slot {
            let got = target.alloc_mask_slot().map_err(CellDriverError::Target)?;
            if got != Some(self.masks_allocated) {
                return Err(CellDriverError::MaskSlot {
                    expected: self.masks_allocated,
                    got,
                });
            }
            self.masks_allocated += 1;
        }
        Ok(())
    }

    /// Forgets, farthest first, nodes outside the cut with no level in the pool, until the CPU
    /// budget holds. Nodes that arrived after the cut and the cut's support set are spared.
    fn forget(&mut self, cut: &[NodeId], view: &ViewProjection) -> Vec<NodeId> {
        if self.cpu_bytes <= self.config.cpu_budget_bytes {
            return Vec::new();
        }
        let eye: [f64; 3] = std::array::from_fn(|axis| {
            view.camera().position()[axis] / view.visual_scale()
                + self.config.coordinate_origin[axis]
        });
        let in_cut: HashSet<NodeId> = cut.iter().copied().collect();
        // Nodes of an older colour generation go first: they only stand in until recoloured.
        let newest = self.newest_generation.map(color_of).unwrap_or(0);
        let outdated = |node: &StoredNode| node.generation.is_some_and(|g| color_of(g) < newest);
        let mut candidates: Vec<(bool, f64, NodeId)> = self
            .nodes
            .iter()
            .filter(|(id, node)| {
                node.live == 0
                    && !in_cut.contains(id)
                    && !self.arrived_since_cut.contains(id)
                    && !self.support.contains(id)
            })
            .map(|(id, node)| (outdated(node), distance_to(eye, node.pool.bounds), *id))
            .collect();
        candidates.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.total_cmp(&a.1)).then(a.2.cmp(&b.2)));
        let mut forgotten = Vec::new();
        for (_, _, id) in candidates {
            if self.cpu_bytes <= self.config.cpu_budget_bytes {
                break;
            }
            if let Some(node) = self.nodes.remove(&id) {
                self.cpu_bytes -= node.bytes;
                self.arrived_since_cut.remove(&id);
                forgotten.push(id);
            }
        }
        forgotten
    }
}

#[cfg(test)]
#[path = "cell_driver_tests.rs"]
mod tests;
