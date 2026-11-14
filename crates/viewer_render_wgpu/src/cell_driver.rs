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

use crate::{
    CellCutError, CellGrid, CellPool, CellPoolEvent, CutApply, FramePlan, LevelKey, MASK_WORDS,
    NodeCells, PAGE_SLOTS, PackedLevel, PageEntry, PageFrame, PageInfo, PoolConfig, PoolCounters,
    PoolNode, ViewProjection, pack_level,
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

    fn set_node_mask(&mut self, slot: u32, mask: &[u32; MASK_WORDS]) -> Result<(), Self::Error>;

    /// The page table of this frame; `frame` is the one its entries were built with.
    fn set_active_pages(
        &mut self,
        pages: &[(u32, PageEntry)],
        frame: &PageFrame,
    ) -> Result<(), Self::Error>;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeAdded {
    /// Stored; its non-empty levels are known to the pool.
    Stored { levels: usize },
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

struct StoredNode {
    pool: PoolNode,
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
    /// The target received writes since the last reset, so a missing pool there means loss.
    armed: bool,
    /// A target call failed: the target state is unknown until the next reset.
    failed: bool,
    /// Nodes were added or the pool was reset since the last frame.
    dirty: bool,
    busy: bool,
    /// Nodes added since the last cut was latched ([`CellDriver::cut_received`]): the cut being
    /// drawn was sent before they arrived, so `forget` spares them until the next cut.
    arrived_since_cut: HashSet<NodeId>,
    /// Nodes the sender needs stored although no cut names them (ancestors of cut nodes still
    /// being built, R3-Q22): `forget` spares them. Replaced by every cut ([`CellDriver::set_support`]).
    support: HashSet<NodeId>,
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
                <= 4.0 * f64::EPSILON * scale
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
            armed: false,
            failed: false,
            dirty: false,
            busy: false,
            arrived_since_cut: HashSet::new(),
            support: HashSet::new(),
        }
    }

    pub fn pool(&self) -> &CellPool {
        &self.pool
    }

    /// Pages of the pool this driver fits cuts to (`PoolConfig::capacity_pages`).
    pub fn pool_capacity_pages(&self) -> u32 {
        self.config.pool.capacity_pages
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
        self.add_packed(node.id, node.bounds, node.points, node.levels)
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
        self.store_packed(None, id, bounds, points, levels)
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
            node.levels,
        )
    }

    fn store_packed(
        &mut self,
        generation: Option<u64>,
        id: NodeId,
        bounds: SpatialBounds,
        points: u64,
        mut levels: Vec<PackedLevel>,
    ) -> Result<NodeAdded, NodeInputError> {
        if let Some(stored) = self.nodes.get(&id) {
            return if generation.is_some() && stored.generation == generation {
                Ok(NodeAdded::AlreadyStored)
            } else {
                Err(NodeInputError::Duplicate(id))
            };
        }
        levels.retain(|level| !level.pages.is_empty());
        levels.sort_by_key(|level| level.depth);
        if levels.iter().any(|level| level.node != id)
            || levels.windows(2).any(|pair| pair[0].depth == pair[1].depth)
        {
            return Err(NodeInputError::InvalidLevel(id));
        }
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
        self.nodes.insert(
            id,
            StoredNode {
                pool: PoolNode {
                    bounds,
                    stored: levels.iter().map(|level| level.depth).collect(),
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
        self.armed = false;
        self.failed = false;
        self.dirty = true;
        events
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

    /// A node stored with another generation than `generation` (or with none): the driver was
    /// filled by another dataset, so the caller must replace it (`driverReplaced`).
    pub fn foreign_node(&self, generation: u64) -> Option<NodeId> {
        self.nodes
            .iter()
            .filter(|(_, node)| node.generation != Some(generation))
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
        if let Err(error) = self.execute(target, view, &mut report, mark) {
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
        self.dirty = false;
        report.forgotten = self.forget(cut, view);
        Ok(report)
    }

    /// The writes of the plan, in its order: frees, uploads, masks, page table.
    fn execute<T: CellTarget>(
        &mut self,
        target: &mut T,
        view: &ViewProjection,
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
        for update in &plan.mask_updates {
            self.ensure_mask(target, update.slot)?;
            let words: [u32; MASK_WORDS] =
                std::array::from_fn(|i| (update.mask[i / 2] >> (32 * (i % 2))) as u32);
            target
                .set_node_mask(update.slot, &words)
                .map_err(CellDriverError::Target)?;
        }
        if let Some(max) = plan.active.iter().map(|active| active.mask_slot).max() {
            self.ensure_mask(target, max)?;
        }
        mark(FramePhase::Written);
        let frame = PageFrame {
            grid: self.config.grid,
            coordinate_origin: self.config.coordinate_origin,
            visual_scale: view.visual_scale(),
            eye_scaled: target.eye(),
        };
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
        mark(FramePhase::TableBuilt);
        // Nothing resident and no pool yet: there is no page table to write.
        if !table.is_empty() || target.has_pool() {
            target
                .set_active_pages(&table, &frame)
                .map_err(CellDriverError::Target)?;
        }
        mark(FramePhase::ActivePagesSet);
        Ok(())
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
        let mut candidates: Vec<(f64, NodeId)> = self
            .nodes
            .iter()
            .filter(|(id, node)| {
                node.live == 0
                    && !in_cut.contains(id)
                    && !self.arrived_since_cut.contains(id)
                    && !self.support.contains(id)
            })
            .map(|(id, node)| (distance_to(eye, node.pool.bounds), *id))
            .collect();
        candidates.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        let mut forgotten = Vec::new();
        for (_, id) in candidates {
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
