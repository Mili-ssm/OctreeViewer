//! Cells mode of `LodDataset` (R3 Task 9b, 10a2): the planner sees a node as resident only once
//! its cells exist, a few nodes are built per update from their decoded blocks, and the cut goes to
//! a driver the dataset never touches. An update has three steps, so the build and the render
//! sides can live in different workers:
//!
//! 1. [`LodDataset::plan_cell_update`] (dataset): select, plan, load, build and pack; gives the
//!    cut and the nodes the driver does not have yet ([`CellUpdate`]).
//! 2. [`add_cell_nodes`] and [`draw_cell_cut`] (render): store the nodes, drop the cut nodes the
//!    driver lacks, run one frame; gives a [`CellDraw`] with the [`PoolAck`].
//! 3. [`LodDataset::apply_evicted`], [`LodDataset::pool_reset`], [`LodDataset::apply_ack`] and
//!    [`LodDataset::driver_replaced`] (dataset): learn what the driver did.
//!
//! [`LodDataset::update_cells_at_stage`] runs the three back to back in one thread.

use std::{
    collections::{HashMap, HashSet},
    sync::atomic::{AtomicU64, Ordering},
};

use super::{
    LodDataset, LodDatasetError, LodTransitionPlan, NodeId, PointBlock, WorkClock,
    pending_page_anchors,
};
use viewer_core::{BlockId, LodSelection, ResidentBlockSource, SpatialBounds, SpatialHierarchy};
use viewer_render_wgpu::{
    CellDriver, CellDriverError, CellGrid, CellTarget, CutApply, FrameReport, NodeCellError,
    NodeCellSource, NodeCells, NodeInput, PackedNode, PointNodeCellSource, PoolCounters,
    ViewProjection, node_cell_level, pack_node, packed_node_bytes, representative_depth,
};

/// Deepest ancestor-descendant gap, in geometric cell depths (`representative_depth`), a node
/// mask can express (`MAX_MASK_LEVELS` of the cut).
pub const MAX_CELL_CUT_GAP: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellUpdateConfig {
    /// Nodes whose cells are built per update, nearest to the eye first.
    pub max_node_builds: usize,
    /// Cell levels built on demand per update (`LodDataset::apply_level_requests`), apart from
    /// the node builds: a level is one depth of a node already built.
    pub max_level_builds: usize,
    /// The driver's CPU budget (`DriverConfig::cpu_budget_bytes`), when the build side must fit
    /// the cut to it (ADR 0037). `None`: unbounded, as native callers and tests that give the
    /// driver no budget.
    pub cpu_budget_bytes: Option<u64>,
    /// Pages of the GPU pool the render side runs (`SurfaceRenderer::cell_pool_pages`), when the
    /// build side knows them. `None`: the driver's default pool and no fit to it.
    pub gpu_pool_pages: Option<u32>,
}

impl Default for CellUpdateConfig {
    fn default() -> Self {
        Self {
            max_node_builds: 2,
            max_level_builds: 8,
            cpu_budget_bytes: None,
            gpu_pool_pages: None,
        }
    }
}

/// Why the build side built a node, kept to tell which kind the driver forgets (diagnosis only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BuildRole {
    /// In the selection at the stage px (also the role of a node built outside a plan).
    #[default]
    Selected,
    /// Co-owner without cells of a block shared with a selected node.
    Waiter,
    /// Parents-first build of an ancestor that is not selected.
    Ancestor,
}

/// Diagnosis counters of the build/forget loop. They read state and change nothing.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CellDiag {
    /// Cumulative forgets by the driver, by the role the node was built with.
    pub forgotten_selected: u64,
    pub forgotten_waiter: u64,
    pub forgotten_ancestor: u64,
    /// Cumulative builds of a node built before in this dataset.
    pub rebuilt_total: u64,
    pub selection_nodes: usize,
    /// Built selected nodes absent from the cut, and of those, with a block not in the cache.
    pub selected_built_not_in_cut: usize,
    pub selected_built_blocks_missing: usize,
    /// The bytes the ADR 0037 admission counted (0 without a CPU budget).
    pub cpu_needed_bytes: u64,
    pub block_cache_bytes: u64,
    pub block_cache_limit_bytes: u64,
    /// Cumulative blocks the cache evicted (as seen by cells updates).
    pub block_evictions: u64,
    /// Cumulative nodes built and taken back because they did not fit a budget.
    pub rejected_builds: u64,
}

/// What step 1 of a cells update did.
#[derive(Debug, Clone, PartialEq)]
pub struct CellPlanStats {
    /// Nodes whose cells were built, in build order.
    pub built_nodes: Vec<NodeId>,
    /// Nodes ready to be built (blocks decoded, parents done) that the cap left for later.
    pub build_deferred: usize,
    /// The node part of `build_deferred` (without the level backlog).
    pub nodes_deferred: usize,
    pub build_capped: bool,
    pub build_ms: f64,
    /// Nodes of the cut sent to the driver (plan nodes with cells).
    pub cut_nodes: usize,
    /// Largest depth difference between a cut node and its nearest cut ancestor.
    pub max_gap: u32,
    /// Cut nodes whose gap is above 1.
    pub gaps_over_one: usize,
    pub pending_block_ids: Vec<BlockId>,
    pub pending_page_anchors: Vec<NodeId>,
    pub uncovered_nodes: usize,
    /// Candidates the CPU budget keeps from being built (ADR 0037): their zone is drawn with the
    /// ancestor that has cells.
    pub cpu_coarsened: usize,
    /// Candidates the GPU pool keeps from being built (ADR 0040), same meaning as `cpu_coarsened`
    /// for the pages of the coarsest level of each node.
    pub gpu_coarsened: usize,
    /// Pool pages the cut was fitted to; 0 when the build side was not told (no fit).
    pub gpu_pool_pages: u32,
    /// The rung of the px ladder this plan ran at (published as `lodStagePixels`).
    pub stage_pixels: f64,
    pub diag: CellDiag,
}

/// What one cells update did, with the driver's frame.
#[derive(Debug, Clone, PartialEq)]
pub struct CellUpdateStats {
    /// Nodes whose cells were built, in build order.
    pub built_nodes: Vec<NodeId>,
    /// Nodes ready to be built (blocks decoded, parents done) that the cap left for later.
    pub build_deferred: usize,
    /// The node part of `build_deferred` (without the level backlog).
    pub nodes_deferred: usize,
    pub build_capped: bool,
    pub build_ms: f64,
    /// Nodes of the cut given to the driver (plan nodes with cells).
    pub cut_nodes: usize,
    /// Largest depth difference between a cut node and its nearest cut ancestor.
    pub max_gap: u32,
    /// Cut nodes whose gap is above 1.
    pub gaps_over_one: usize,
    /// The driver's real cut failed with `DescendantTooFine` (it retains the previous one).
    pub descendant_too_fine: bool,
    pub apply: CutApply,
    pub counters: PoolCounters,
    pub cpu_cell_bytes: u64,
    pub pending_block_ids: Vec<BlockId>,
    pub pending_page_anchors: Vec<NodeId>,
    pub uncovered_nodes: usize,
    /// As [`CellPlanStats::cpu_coarsened`].
    pub cpu_coarsened: usize,
    /// As [`CellPlanStats::gpu_coarsened`].
    pub gpu_coarsened: usize,
    /// As [`CellPlanStats::gpu_pool_pages`].
    pub gpu_pool_pages: u32,
    pub diag: CellDiag,
}

impl CellUpdateStats {
    fn new(plan: CellPlanStats, report: &FrameReport, cpu_cell_bytes: u64) -> Self {
        Self {
            built_nodes: plan.built_nodes,
            build_deferred: plan.build_deferred,
            nodes_deferred: plan.nodes_deferred,
            build_capped: plan.build_capped,
            build_ms: plan.build_ms,
            cut_nodes: plan.cut_nodes,
            max_gap: plan.max_gap,
            gaps_over_one: plan.gaps_over_one,
            descendant_too_fine: report.counters().descendant_too_fine,
            apply: report.plan.apply,
            counters: *report.counters(),
            cpu_cell_bytes,
            pending_block_ids: plan.pending_block_ids,
            pending_page_anchors: plan.pending_page_anchors,
            uncovered_nodes: plan.uncovered_nodes,
            cpu_coarsened: plan.cpu_coarsened,
            gpu_coarsened: plan.gpu_coarsened,
            gpu_pool_pages: plan.gpu_pool_pages,
            diag: plan.diag,
        }
    }
}

pub struct LodCellUpdate {
    pub stats: CellUpdateStats,
    pub report: FrameReport,
}

/// Step 1: what the build side sends to the render side.
#[derive(Debug, Clone, PartialEq)]
pub struct CellUpdate {
    /// Identifies the cut; grows with every update.
    pub cut_epoch: u64,
    /// The planner's cut restricted to nodes the dataset believes the driver has.
    pub cut: Vec<NodeId>,
    /// Nodes with cells that the driver must keep although the cut does not name them: ancestors
    /// of nodes still waiting for their cells. Forgetting one would make parents-first (R3-Q22)
    /// build it again before those descendants, over and over under a tight CPU budget.
    pub support: Vec<NodeId>,
    /// Nodes built by this update, with the dataset generation, in build order (parents first).
    pub new_nodes: Vec<(u64, PackedNode)>,
    /// Levels built on demand for nodes the driver has, one packed node per node holding only the
    /// new depths (`CellDriver::add_packed_levels`), sent after `new_nodes`.
    pub new_levels: Vec<(u64, PackedNode)>,
    /// `(node, depth)` requests the build side cannot serve (`unservable_levels`) that arrived
    /// since the last update: the render side stops wanting them
    /// (`CellDriver::refuse_levels`).
    pub refused_levels: Vec<(NodeId, u8)>,
    pub stats: CellPlanStats,
}

/// Step 2 to step 3: what the driver drew for a cut.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolAck {
    /// The `cut_epoch` the frame applied.
    pub cut_epoch: u64,
    /// Nodes the pool draws after the frame (applied or retained cut).
    pub drawn: Vec<NodeId>,
    /// Nodes the cut named and the driver did not have: to be built and sent again.
    pub missing: Vec<NodeId>,
}

/// Step 2: the outcome of one driver frame.
pub struct CellDraw {
    pub ack: PoolAck,
    /// Nodes the driver forgot (CPU budget), in order.
    pub evicted: Vec<NodeId>,
    /// The target had lost its pool and the driver reset it before drawing.
    pub pool_reset: bool,
    pub report: FrameReport,
}

/// Next dataset generation: a new dataset gets a new one, so a node of an old build is never
/// taken for a repeat of the new build's.
static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);

/// A node whose cells were sent to the driver.
#[derive(Debug, Clone, Copy)]
struct BuiltNode {
    /// Epoch of the first cut that follows the send: a `missing` report of an earlier cut says
    /// nothing about the resend.
    sent: u64,
    /// Packed bytes the driver keeps for it (`packed_node_bytes`), for the CPU budget fit.
    bytes: u64,
    /// Pool pages of its coarsest stored level: the least the GPU pool needs for it (ADR 0040).
    coarsest_pages: u64,
    role: BuildRole,
    /// The node had been built before in this dataset.
    rebuild: bool,
    /// The colour generation its cells were painted with. An older one than the dataset's is
    /// still drawn by the driver until the node is built again (see [`CellState::has_cells`]).
    color_gen: u32,
}

/// The CPU budget fit (ADR 0037) blocks admission above this share of the budget...
const CPU_ADMIT_PERCENT: u64 = 90;
/// ...and lifts the block once the bytes the cut needs fall below this one.
const CPU_RELEASE_PERCENT: u64 = 80;
/// The GPU pool fit (ADR 0040) uses the pool's own shares, of its usable pages.
const GPU_ADMIT_PERCENT: u64 = viewer_render_wgpu::HIGH_PERCENT;
const GPU_RELEASE_PERCENT: u64 = viewer_render_wgpu::LOW_PERCENT;

/// `percent` of `budget`, without overflowing on an unbounded budget.
fn share(budget: u64, percent: u64) -> u64 {
    (u128::from(budget) * u128::from(percent) / 100) as u64
}

/// Pages of the coarsest non-empty level of a packed node, the least the pool needs to draw it.
fn coarsest_level_pages(node: &PackedNode) -> u64 {
    node.levels
        .iter()
        .filter(|level| !level.pages.is_empty())
        .min_by_key(|level| level.depth)
        .map_or(0, |level| level.pages.len() as u64)
}

/// The release rule of one budget's block (ADR 0037, 0039, 0040): the block set when a candidate
/// did not fit lifts once what the cut needs (`needed`, in-progress children included, `in_progress`
/// of it) falls below the `release` share of `budget`. `released` is the selection `key` the
/// in-progress children were released for and the most they held; the driver drops them, and
/// counting them until the view changes keeps the block from lifting just because they are gone
/// (they would be rebuilt and released again).
fn update_block(
    blocked: &mut bool,
    released: &mut Option<(u64, u64)>,
    key: u64,
    (needed, in_progress): (u64, u64),
    (budget, release): (u64, u64),
) {
    let mut effective = needed;
    if *blocked {
        let held = match *released {
            Some((k, held)) if k == key => held.max(in_progress),
            _ => in_progress,
        };
        *released = Some((key, held));
        effective += held - in_progress;
    }
    if effective < share(budget, release) {
        *blocked = false;
    }
    if !*blocked {
        *released = None;
    }
}

pub(super) struct CellState {
    /// Nodes whose cells were sent to the driver.
    built: HashMap<NodeId, BuiltNode>,
    /// Nodes built at least once, and the diagnosis counters (`CellDiag`).
    ever_built: HashSet<NodeId>,
    pub(super) diag: CellDiag,
    /// A node overflowed the CPU budget and the bytes the cut needs have not dropped below
    /// [`CPU_RELEASE_PERCENT`] since: no node is built (see `plan_cell_update`).
    pub(super) cpu_blocked: bool,
    /// While blocked: the selection the in-progress children were released for and the most
    /// bytes they held. The driver drops them, and counting them until the view changes keeps
    /// the block from lifting just because they are gone (they would be rebuilt and released
    /// again).
    released: Option<(u64, u64)>,
    /// The same for the GPU pool (ADR 0040): a node's coarsest level did not fit in the pages
    /// the pool can use, until the cut needs less than [`GPU_RELEASE_PERCENT`] of them.
    pub(super) gpu_blocked: bool,
    gpu_released: Option<(u64, u64)>,
    /// The node a budget rejected for the selection `key`, with its bytes and pages when they
    /// were over a budget. While blocked they stay counted in what the cut needs, so the block does
    /// not lift just because the rejected node was taken back and it is not built again, over and
    /// over, until the selection changes.
    rejected: Option<(u64, u64, u64)>,
    /// Nodes whose every level came out empty: resident for the plan, never in the cut. That
    /// leaves no hole: a node has no levels only when none of its points is finite (a finite
    /// point always lands in a cell), and such points draw nothing on any path.
    pub(super) empty: HashSet<NodeId>,
    /// Nodes of the cut the pool draws (applied or retained) after the last `poolAck`. Cleared by
    /// `pool_reset` and by a failed frame.
    pub(super) pool_drawn: Vec<NodeId>,
    /// Epoch of the last cut sent and of the last one acknowledged.
    cut_epoch: u64,
    ack_epoch: u64,
    /// Cuts sent after the last acknowledged epoch: the pool may draw them any moment.
    unacked: Vec<(u64, Vec<NodeId>)>,
    generation: u64,
    /// R3-Q33: the colour generation of the cells and the attribute the blocks are painted with
    /// (`None`: the file's RGB). Every change of the attribute starts a new colour generation;
    /// nodes built with an older one are rebuilt, coarse to fine, by the same ladder that builds
    /// a node that was never built, while the driver keeps drawing the old cells of the nodes
    /// that were not rebuilt yet. The two never mix inside a node: the driver swaps a node whole.
    color_gen: u32,
    paint: Option<(u32, u32)>,
    /// The last block read was for an ancestor: the next read goes to the selection, so a block
    /// of the chain that cannot stay cached cannot starve the blocks in view.
    read_ancestor_last: bool,
    /// The `(node, depth)` levels the render side asked for, in priority order (the last
    /// `apply_level_requests`; served ones leave it).
    level_requests: Vec<(NodeId, u8)>,
    /// What the last `serve_level_requests` let through (`LevelRoom`): the cut nodes, whose
    /// levels are always servable, and whether the other nodes' levels were servable then.
    level_cut: HashSet<NodeId>,
    level_outside_open: bool,
    /// Requested depths `node_cell_level` cannot build (a leaf at the deepest cell depth whose
    /// positions are still merged): dropped, and not tried again.
    unservable_levels: HashSet<(NodeId, u8)>,
    /// Unservable requests not yet reported in a `CellUpdate`, without repeats.
    refused_levels: Vec<(NodeId, u8)>,
    /// Depths shipped on demand per built node. A node built again (new colour generation) is
    /// swapped whole by the driver, so these are built with it: the node never gets coarser than
    /// it was drawn.
    level_depths: HashMap<NodeId, Vec<u8>>,
}

/// The CPU budget as the level builds see it. Levels of nodes in the `cut` are exempt from the
/// gate (the pool holds a node until its level exists, and those bytes are the cut itself, which
/// the budget cannot shrink); their bytes add to `needed` when the node is `counted`, so they
/// gate the builds of other nodes. A level of any other node waits while the build side is
/// blocked or `needed` has reached `admit`.
struct LevelRoom<'a> {
    cut: &'a HashSet<NodeId>,
    admit: Option<u64>,
    needed: &'a mut u64,
    counted: &'a HashSet<NodeId>,
}

impl LevelRoom<'_> {
    /// Whether a level of a node outside the cut may be built now: the build side is not blocked
    /// and the bytes the cut needs are under the admit share.
    fn admits_outside_cut(&self, blocked: bool) -> bool {
        !blocked && self.admit.is_none_or(|admit| *self.needed < admit)
    }
}

impl Default for CellState {
    fn default() -> Self {
        Self {
            built: HashMap::new(),
            ever_built: HashSet::new(),
            diag: CellDiag::default(),
            cpu_blocked: false,
            released: None,
            gpu_blocked: false,
            gpu_released: None,
            rejected: None,
            empty: HashSet::new(),
            pool_drawn: Vec::new(),
            cut_epoch: 0,
            ack_epoch: 0,
            unacked: Vec::new(),
            generation: NEXT_GENERATION.fetch_add(1, Ordering::Relaxed),
            color_gen: 0,
            paint: None,
            read_ancestor_last: false,
            level_requests: Vec::new(),
            level_cut: HashSet::new(),
            level_outside_open: true,
            unservable_levels: HashSet::new(),
            refused_levels: Vec::new(),
            level_depths: HashMap::new(),
        }
    }
}

impl CellState {
    #[cfg(test)]
    pub(super) fn pool_drawn(&self) -> &[NodeId] {
        &self.pool_drawn
    }

    /// Nodes the pool may be drawing: the last ack's and every cut sent since its epoch.
    pub(super) fn extend_pins(&self, nodes: &mut Vec<NodeId>) {
        nodes.extend_from_slice(&self.pool_drawn);
        for (_, cut) in &self.unacked {
            nodes.extend_from_slice(cut);
        }
        nodes.sort_unstable();
        nodes.dedup();
    }

    /// `measure` summed over the built nodes among `ids`.
    fn sum_over<'a>(
        &self,
        ids: impl IntoIterator<Item = &'a NodeId>,
        measure: fn(&BuiltNode) -> u64,
    ) -> u64 {
        ids.into_iter()
            .filter_map(|id| self.built.get(id))
            .map(measure)
            .sum()
    }

    /// The node has cells of the current colour generation (or none to draw). A node with older
    /// ones is a candidate to be built again: it counts as without cells for the build ladder,
    /// and as with cells ([`CellState::has_drawable_cells`]) for the cut, so the cut never loses
    /// a node because of a colour change.
    pub(super) fn has_cells(&self, node: NodeId) -> bool {
        self.built
            .get(&node)
            .is_some_and(|built| built.color_gen == self.color_gen)
            || self.empty.contains(&node)
    }

    /// The driver holds cells to draw for the node, of any colour generation.
    pub(super) fn has_drawable_cells(&self, node: NodeId) -> bool {
        self.built.contains_key(&node) || self.empty.contains(&node)
    }

    /// Packed bytes of a built node (tests and simulations).
    #[cfg(test)]
    pub(super) fn built_bytes(&self, node: NodeId) -> Option<u64> {
        self.built.get(&node).map(|built| built.bytes)
    }
}

/// Residency as the planner sees it in cells mode (ADR 0038): a block is resident when every node
/// owning it has its cells, whether or not the decoded block is still cached, so the planner
/// covers unbuilt nodes with their ancestors. The point budget does not apply: the cell bytes
/// budget (ADR 0037) bounds the cut.
struct CellReadySource<'a> {
    cache: &'a viewer_core::DecodedBlockCache,
    ready_blocks: &'a HashSet<BlockId>,
}

impl ResidentBlockSource for CellReadySource<'_> {
    fn peek(&self, id: BlockId) -> Option<&PointBlock> {
        if self.ready_blocks.contains(&id) {
            self.cache.peek(id)
        } else {
            None
        }
    }

    fn contains(&self, id: BlockId) -> bool {
        self.ready_blocks.contains(&id)
    }

    fn budget_points(&self, id: BlockId) -> Option<u64> {
        self.ready_blocks.contains(&id).then_some(0)
    }
}

/// A node waiting for its cells.
struct BuildCandidate {
    /// A co-owner of a shared block, not a selected node: takes slots after the selected ones.
    waiter: bool,
    /// An ancestor that only waits because a descendant needs it (R3-Q22).
    ancestor: bool,
    distance: f64,
    id: NodeId,
}

/// Decides when a published digest counts as `cutDigestAtRest`: the build side is idle at epoch
/// E, the render side applied E, the driver has no work and the last ack is for E, and the digest
/// is the same in two consecutive polls.
#[derive(Debug, Default)]
pub struct CutRestProbe {
    last: Option<u64>,
}

impl CutRestProbe {
    /// `applied_epoch` and `needs_frame` come from the render side; `digest` is
    /// `cell_cut_digest` of the cut it drew.
    pub fn poll(
        &mut self,
        dataset: &LodDataset,
        build_idle: bool,
        applied_epoch: u64,
        needs_frame: bool,
        digest: u64,
    ) -> Option<u64> {
        let rest = build_idle
            && !needs_frame
            && applied_epoch == dataset.cells.cut_epoch
            && dataset.cells.ack_epoch == applied_epoch;
        if !rest {
            self.last = None;
            return None;
        }
        let stable = self.last == Some(digest);
        self.last = Some(digest);
        stable.then_some(digest)
    }
}

/// Step 2a: stores the nodes of a `nodeCells` batch in order. A repeat of a node with the same
/// generation is ignored (`AlreadyStored`); any other refusal is an error.
pub fn add_cell_nodes(
    driver: &mut CellDriver,
    nodes: impl IntoIterator<Item = (u64, PackedNode)>,
) -> Result<(), LodDatasetError> {
    for (generation, node) in nodes {
        driver
            .add_packed_generation(generation, node)
            .map_err(|error| LodDatasetError::CellDriver(error.to_string()))?;
    }
    Ok(())
}

/// Step 2a for the levels built on demand: merges them into the nodes the driver stores.
/// Levels for a node the driver does not hold (forgotten meanwhile) are ignored.
pub fn add_cell_levels(
    driver: &mut CellDriver,
    levels: impl IntoIterator<Item = (u64, PackedNode)>,
) -> Result<(), LodDatasetError> {
    for (generation, node) in levels {
        driver
            .add_packed_levels(generation, node)
            .map_err(|error| LodDatasetError::CellDriver(error.to_string()))?;
    }
    Ok(())
}

/// Step 2b: one driver frame for `cut`, restricted to the nodes the driver has. The cut must have
/// been latched with `CellDriver::cut_received` when its message arrived.
pub fn draw_cell_cut<T: CellTarget>(
    driver: &mut CellDriver,
    target: &mut T,
    cut_epoch: u64,
    cut: &[NodeId],
    view: &ViewProjection,
    target_px: f64,
) -> Result<CellDraw, LodDatasetError> {
    let (known, missing): (Vec<NodeId>, Vec<NodeId>) =
        cut.iter().copied().partition(|id| driver.has_node(*id));
    let report = driver
        .frame_latched(target, &known, view, target_px, &mut |_| {})
        .map_err(|error| match error {
            CellDriverError::Cut(error) => LodDatasetError::CellCut(error),
            other => LodDatasetError::CellDriver(other.to_string()),
        })?;
    Ok(CellDraw {
        ack: PoolAck {
            cut_epoch,
            drawn: report.plan.entries.iter().map(|entry| entry.node).collect(),
            missing,
        },
        evicted: report.forgotten.clone(),
        pool_reset: report.pool_reset,
        report,
    })
}

impl LodDataset {
    /// Whether cells can be built for this dataset: global-frame formats with replacement
    /// semantics (R3-Q5: the others keep the CPU path).
    pub fn supports_cells(&self) -> bool {
        self.global_frame
            && self.metadata.hierarchy().semantics() == viewer_core::HierarchySemantics::Replacement
    }

    /// The cell grid in the dataset frame, for `DriverConfig::grid`.
    pub fn cell_grid(&self) -> CellGrid {
        let root = self.metadata.hierarchy().stats().root_bounds;
        CellGrid {
            root_min: root.min(),
            root_side: root.max_side(),
        }
    }

    /// The tight extent of the data for `CellDriver::set_data_bounds`; `None` when it is the root
    /// cube itself (nothing tighter is known).
    pub fn cell_data_bounds(&self) -> Option<SpatialBounds> {
        let stats = self.metadata.hierarchy().stats();
        (stats.data_bounds != stats.root_bounds).then_some(stats.data_bounds)
    }

    /// Z span of the tight data extent (absolute coordinates) for `CellDriver::set_elevation_range`;
    /// unlike `cell_data_bounds` it is given when the extent is the root cube.
    pub fn cell_elevation_range(&self) -> [f64; 2] {
        let data = self.metadata.hierarchy().stats().data_bounds;
        [data.min()[2], data.max()[2]]
    }

    /// The dataset's `coordinate_origin`, for `DriverConfig::coordinate_origin`.
    pub fn coordinate_origin(&self) -> [f64; 3] {
        self.metadata.coordinate_frame().origin()
    }

    /// The generation stamped on the nodes this dataset sends.
    pub fn cell_generation(&self) -> u64 {
        viewer_render_wgpu::cell_generation(self.cells.generation, self.cells.color_gen)
    }

    /// R3-Q33: the blocks of the cells path are painted with attribute `paint` (`None`: the
    /// file's RGB). A change starts a new colour generation: the decoded blocks are dropped (they
    /// carry the old paint) and every built node is built again, nearest the cut first. Returns
    /// whether the paint changed.
    pub fn set_cell_paint(&mut self, paint: Option<(u32, u32)>) -> bool {
        if self.cells.paint == paint {
            return false;
        }
        self.cells.paint = paint;
        self.cells.color_gen = self.cells.color_gen.saturating_add(1);
        // Pending requests survive: the rebuilt node (swapped whole) is served them later with
        // the current colours, and the render side does not ask again for an unchanged set.
        self.clear_loaded_blocks();
        true
    }

    /// The levels the render side wants built on demand (`CellDriver::take_level_requests`), in
    /// priority order. Replaces the pending set: a request not in it is dropped. The next update
    /// builds up to `CellUpdateConfig::max_level_builds` of them (nearest the front first) for
    /// nodes it has built, reads the blocks of the others, and ships the levels in
    /// `CellUpdate::new_levels`.
    pub fn apply_level_requests(&mut self, requests: &[(NodeId, u8)]) {
        self.cells.level_requests = requests.to_vec();
    }

    /// Whether some requested level is servable now, so another update makes progress: its node
    /// has cells, its blocks are cached or the host can read them (`can_load_blocks`), and the
    /// gate of the last update (`LevelRoom`) let it through: a cut node always, another node only
    /// while the build side is not blocked and under the admit share.
    #[cfg(any(target_arch = "wasm32", test))]
    pub(crate) fn has_pending_level_requests(&self, can_load_blocks: bool) -> bool {
        self.cells.level_requests.iter().any(|request| {
            !self.cells.unservable_levels.contains(request)
                && self.level_node_state(request.0).is_some()
                && (can_load_blocks || self.level_blocks_cached(request.0))
                && (self.cells.level_cut.contains(&request.0) || self.cells.level_outside_open)
        })
    }

    fn level_blocks_cached(&self, id: NodeId) -> bool {
        self.metadata.hierarchy().node(id).is_some_and(|node| {
            node.blocks()
                .iter()
                .all(|block| self.cache.contains(*block))
        })
    }

    /// `Some(true)` when the node's cells are of the current colour generation, `Some(false)`
    /// when they are an older one being rebuilt, `None` when the node has none to add levels to.
    fn level_node_state(&self, id: NodeId) -> Option<bool> {
        self.cells
            .built
            .get(&id)
            .map(|built| built.color_gen == self.cells.color_gen)
    }

    /// Blocks of the nodes with pending level requests that are not cached yet.
    fn level_request_blocks(&self) -> Vec<BlockId> {
        let hierarchy = self.metadata.hierarchy();
        let mut blocks: Vec<BlockId> = self
            .cells
            .level_requests
            .iter()
            .filter(|request| !self.cells.unservable_levels.contains(request))
            .filter(|request| self.level_node_state(request.0) == Some(true))
            .filter_map(|request| hierarchy.node(request.0))
            .flat_map(|node| node.blocks().iter().copied())
            .filter(|block| !self.cache.contains(*block))
            .collect();
        blocks.sort();
        blocks.dedup();
        blocks
    }

    /// Builds the pending levels of nodes whose blocks are cached, at most `max` of them. Returns
    /// the packed levels, one node per message, and the number of servable requests the budget
    /// left for the next update. Requests of nodes without cells are dropped, those of nodes
    /// being rebuilt or with uncached blocks stay, and a depth `node_cell_level` cannot build is
    /// remembered as unservable and dropped.
    fn serve_level_requests(
        &mut self,
        max: usize,
        generation: u64,
        room: LevelRoom<'_>,
    ) -> Result<(Vec<(u64, PackedNode)>, usize), LodDatasetError> {
        let blocked = self.cells.cpu_blocked || self.cells.gpu_blocked;
        let requests = std::mem::take(&mut self.cells.level_requests);
        let mut keep = Vec::new();
        let mut deferred = 0;
        let mut chosen: Vec<(NodeId, Vec<u8>)> = Vec::new();
        let mut taken = 0;
        for request in requests {
            let (id, depth) = request;
            if self.cells.unservable_levels.contains(&request) {
                self.report_refused(request);
                continue;
            }
            match self.level_node_state(id) {
                // No cells for the node: evicted or being rebuilt, so the request is stale, not
                // unservable. Dropped without a refusal: the render side asks again for what
                // it still wants once it holds the node, and a refusal would end that for good.
                None => continue,
                Some(false) => {
                    keep.push(request);
                    continue;
                }
                Some(true) => {}
            }
            if !self.level_blocks_cached(id) {
                keep.push(request);
                continue;
            }
            if taken == max {
                deferred += 1;
                keep.push(request);
                continue;
            }
            if !room.cut.contains(&id) && !room.admits_outside_cut(blocked) {
                keep.push(request);
                continue;
            }
            taken += 1;
            match chosen.iter_mut().find(|(node, _)| *node == id) {
                Some((_, depths)) => depths.push(depth),
                None => chosen.push((id, vec![depth])),
            }
        }
        let mut served = Vec::new();
        let mut chosen = chosen.into_iter();
        let mut failure = None;
        while let Some((id, depths)) = chosen.next() {
            let outcome = self.build_node_levels(id, &depths);
            let (packed, unservable) = match outcome {
                Ok(built) => built,
                Err(error) => {
                    // The packs already served ship; this node and the rest stay pending and
                    // the next update tries them again.
                    keep.extend(depths.iter().map(|depth| (id, *depth)));
                    for (rest, depths) in chosen.by_ref() {
                        keep.extend(depths.iter().map(|depth| (rest, *depth)));
                        deferred += depths.len();
                    }
                    deferred += depths.len();
                    failure = Some(error);
                    break;
                }
            };
            for depth in unservable {
                self.cells.unservable_levels.insert((id, depth));
                self.report_refused((id, depth));
            }
            if let Some(packed) = packed {
                let shipped = self.cells.level_depths.entry(id).or_default();
                shipped.extend(packed.levels.iter().map(|level| level.depth));
                shipped.sort_unstable();
                shipped.dedup();
                let bytes = packed_node_bytes(&packed);
                if let Some(built) = self.cells.built.get_mut(&id) {
                    built.bytes += bytes;
                }
                if room.counted.contains(&id) {
                    *room.needed += bytes;
                }
                served.push((generation, packed));
            }
        }
        self.cells.level_requests = keep;
        self.cells.level_cut = room.cut.clone();
        self.cells.level_outside_open = room.admits_outside_cut(blocked);
        match failure {
            Some(error) if served.is_empty() => Err(error),
            _ => Ok((served, deferred)),
        }
    }

    fn report_refused(&mut self, request: (NodeId, u8)) {
        if !self.cells.refused_levels.contains(&request) {
            self.cells.refused_levels.push(request);
        }
    }

    /// The attribute the cells are painted with.
    pub fn cell_paint(&self) -> Option<(u32, u32)> {
        self.cells.paint
    }

    /// Epoch of the last cut sent.
    pub fn cell_cut_epoch(&self) -> u64 {
        self.cells.cut_epoch
    }

    /// Epoch of the last cut the render side acknowledged.
    pub fn cell_ack_epoch(&self) -> u64 {
        self.cells.ack_epoch
    }

    /// Blocks whose owners all have cells.
    pub(super) fn cell_ready_blocks(&self) -> HashSet<BlockId> {
        let hierarchy = self.metadata.hierarchy();
        self.cells
            .built
            .keys()
            .chain(&self.cells.empty)
            .filter_map(|id| hierarchy.node(*id))
            .flat_map(|node| node.blocks().iter().copied())
            .filter(|block| {
                self.block_owners.get(block).is_none_or(|owners| {
                    owners
                        .iter()
                        .all(|owner| self.cells.has_drawable_cells(*owner))
                })
            })
            .collect()
    }

    /// Other owners of the blocks of selected nodes that have cells, whether they have cells or
    /// not. Limited to the selection so the closure cannot spread across the hierarchy away from
    /// the view.
    fn shared_block_owners(&self, selection: &[NodeId]) -> Vec<NodeId> {
        let hierarchy = self.metadata.hierarchy();
        selection
            .iter()
            .filter(|id| self.cells.has_cells(**id))
            .filter_map(|id| hierarchy.node(*id).map(|node| (*id, node)))
            .flat_map(|(id, node)| {
                node.blocks()
                    .iter()
                    .filter_map(|block| self.block_owners.get(block))
                    .flatten()
                    .copied()
                    .filter(move |owner| *owner != id)
            })
            .collect()
    }

    /// Owners without cells of the blocks of selected nodes that have them.
    fn shared_block_waiters(&self, selection: &[NodeId]) -> Vec<NodeId> {
        let mut owners = self.shared_block_owners(selection);
        owners.retain(|owner| !self.cells.has_cells(*owner));
        owners
    }

    /// Ancestors of `id` that have blocks and no cells, nearest first (R3-Q22: they are built
    /// before `id`, in the plan's cut or not).
    pub(super) fn ancestors_without_cells(&self, id: NodeId) -> Vec<NodeId> {
        let hierarchy = self.metadata.hierarchy();
        let mut out = Vec::new();
        let mut parent = hierarchy.node(id).and_then(|node| node.parent());
        while let Some(up) = parent {
            let Some(node) = hierarchy.node(up) else {
                break;
            };
            if !node.blocks().is_empty() && !self.cells.has_cells(up) {
                out.push(up);
            }
            parent = node.parent();
        }
        out
    }

    /// Whether the camera jumped: no node selected at `stage_px` has cells or an ancestor with
    /// cells. The px ladder restarts then (R3-Q22) and only then, besides the dataset opening.
    #[cfg(any(target_arch = "wasm32", test))]
    #[cfg_attr(not(test), allow(dead_code))] // only the ladder in `lod_driver` calls it
    pub(crate) fn camera_jumped(
        &mut self,
        view: &ViewProjection,
        stage_px: f64,
    ) -> Result<bool, LodDatasetError> {
        let (_, selection) = self.select_target(view, stage_px)?;
        let hierarchy = self.metadata.hierarchy();
        Ok(!selection.node_ids().iter().any(|id| {
            let mut node = Some(*id);
            while let Some(current) = node {
                if self.cells.has_cells(current) {
                    return true;
                }
                node = hierarchy.node(current).and_then(|n| n.parent());
            }
            false
        }))
    }

    pub(super) fn prepare_cell_plan(&mut self, target: &LodSelection) -> LodTransitionPlan {
        self.refinement_work.prepares += 1;
        let planning_started = WorkClock::start();
        let ready_blocks = self.cell_ready_blocks();
        let source = CellReadySource {
            cache: &self.cache,
            ready_blocks: &ready_blocks,
        };
        let plan = self
            .planner
            .prepare(self.metadata.hierarchy(), target, &source);
        self.refinement_work.planning_ms += planning_started.elapsed_ms();
        plan
    }

    /// Whether the cut selected at `target_px` is complete with only built nodes resident: the
    /// ladder may start at the target stage.
    #[cfg(any(target_arch = "wasm32", test))]
    #[cfg_attr(not(test), allow(dead_code))] // only the ladder in `lod_driver` calls it, and that is test-only for now
    pub(crate) fn target_cells_are_ready(
        &mut self,
        view: &ViewProjection,
        target_px: f64,
    ) -> Result<bool, LodDatasetError> {
        let (_, selection) = self.select_target(view, target_px)?;
        let plan = self.prepare_cell_plan(&selection);
        Ok(super::resident_target_plan_is_complete(&selection, &plan))
    }

    /// Step 3, `poolReset`: the pool was emptied (device lost, or the renderer dropped it) but the
    /// driver keeps its nodes: the pool draws nothing until the next frame, so no node is pinned
    /// for it. Process it before the ack of the frame that reset the pool.
    pub fn pool_reset(&mut self) {
        self.cells.pool_drawn.clear();
    }

    /// Step 3, `evict`: the driver forgot these nodes (CPU budget): they are built again when
    /// the cut needs them.
    pub fn apply_evicted(&mut self, nodes: &[NodeId]) {
        for node in nodes {
            if let Some(built) = self.cells.built.remove(node) {
                let diag = &mut self.cells.diag;
                match built.role {
                    BuildRole::Selected => diag.forgotten_selected += 1,
                    BuildRole::Waiter => diag.forgotten_waiter += 1,
                    BuildRole::Ancestor => diag.forgotten_ancestor += 1,
                }
            }
        }
    }

    /// Step 3, `driverReplaced`: the driver is new (another dataset or worker) and has none of
    /// the nodes sent so far: they are all sent again.
    pub fn driver_replaced(&mut self) {
        self.cells.built.clear();
        self.cells.pool_drawn.clear();
        // The new driver asks for what it wants anew; a stale set would keep reading blocks.
        self.cells.level_requests.clear();
    }

    /// Step 3, `poolAck`. The last one wins: an older epoch than the last ack is ignored.
    /// `missing` nodes are built again, unless they were sent after the acknowledged cut.
    pub fn apply_ack(&mut self, ack: PoolAck) {
        if ack.cut_epoch < self.cells.ack_epoch {
            return;
        }
        self.cells.ack_epoch = ack.cut_epoch;
        self.cells.pool_drawn = ack.drawn;
        self.cells
            .unacked
            .retain(|(epoch, _)| *epoch > ack.cut_epoch);
        for node in ack.missing {
            if self
                .cells
                .built
                .get(&node)
                .is_some_and(|built| built.sent <= ack.cut_epoch)
            {
                self.cells.built.remove(&node);
            }
        }
    }

    /// A frame of the in-process path failed: the target may have dropped its pool and the cut
    /// was not applied.
    fn cell_frame_failed(&mut self, cut_epoch: u64) {
        self.pool_reset();
        self.cells.unacked.retain(|(epoch, _)| *epoch != cut_epoch);
    }

    /// In-process path: the driver is the truth about stored cells (a caller may swap it). Nodes
    /// it lacks are sent again. A driver that holds nodes of another dataset generation cannot be
    /// reconciled (resending would be refused as `Duplicate` every update): the caller must
    /// replace it, as `driverReplaced` does across workers.
    pub(crate) fn reconcile_driver(&mut self, driver: &CellDriver) -> Result<(), LodDatasetError> {
        if let Some(node) = driver.foreign_node(self.cell_generation()) {
            return Err(LodDatasetError::CellDriver(format!(
                "the driver holds {node:?} from another dataset generation: replace the driver"
            )));
        }
        self.cells.built.retain(|id, _| driver.has_node(*id));
        Ok(())
    }

    /// Distance from the camera to the node, in the camera's scaled local units.
    pub(super) fn distance_to_eye(&self, view: &ViewProjection, bounds: SpatialBounds) -> f64 {
        let origin = self.coordinate_origin();
        let eye = view.camera().position();
        let scale = view.visual_scale();
        (0..3)
            .map(|axis| {
                let min = (bounds.min()[axis] - origin[axis]) * scale;
                let max = (bounds.max()[axis] - origin[axis]) * scale;
                let d = eye[axis].clamp(min, max) - eye[axis];
                d * d
            })
            .sum::<f64>()
            .sqrt()
    }

    /// Builds and packs the cells of `id` from its decoded blocks. `None` when a block is not
    /// cached. A node without levels goes to `empty` and gives no message.
    pub(super) fn build_node_cells(
        &mut self,
        id: NodeId,
    ) -> Result<Option<PackedNode>, LodDatasetError> {
        let hierarchy = self.metadata.hierarchy();
        let Some(node) = hierarchy.node(id) else {
            return Ok(None);
        };
        let blocks: Option<Vec<PointBlock>> = node
            .blocks()
            .iter()
            .map(|block| self.cache.peek(*block).cloned())
            .collect();
        let Some(blocks) = blocks else {
            return Ok(None);
        };
        let origin = self.coordinate_origin();
        let world = self.cell_grid();
        let local_grid = CellGrid {
            root_min: std::array::from_fn(|axis| world.root_min[axis] - origin[axis]),
            root_side: world.root_side,
        };
        let local = SpatialBounds::new(
            std::array::from_fn(|axis| node.bounds().min()[axis] - origin[axis]),
            std::array::from_fn(|axis| node.bounds().max()[axis] - origin[axis]),
        )
        .map_err(|error| LodDatasetError::CellDriver(error.to_string()))?;
        let is_leaf = cell_leaf(hierarchy, id);
        let mut cells = PointNodeCellSource::default().node_cells(
            &local_grid,
            id,
            local,
            node.spacing(),
            is_leaf,
            &blocks,
        )?;
        // A node built again keeps the levels it was served on demand; a node built from
        // scratch starts over (the driver asks again for what it wants).
        let extra = if self.cells.built.contains_key(&id) {
            self.cells
                .level_depths
                .get(&id)
                .cloned()
                .unwrap_or_default()
        } else {
            self.cells.level_depths.remove(&id);
            Vec::new()
        };
        for depth in extra {
            if cells.levels.iter().any(|level| level.depth == depth) {
                continue;
            }
            match node_cell_level(&local_grid, local, node.spacing(), is_leaf, &blocks, depth) {
                Ok(level) => cells.levels.push(level),
                // Unservable depths (merged or too sparse to page) are skipped, not an error.
                Err(NodeCellError::AbsoluteDepthExceeded | NodeCellError::TooSparseToPage) => {}
                Err(error) => return Err(LodDatasetError::CellDriver(error.to_string())),
            }
        }
        cells.levels.sort_by_key(|level| level.depth);
        let packed = pack_node(
            &world,
            origin,
            NodeInput {
                id,
                bounds: node.bounds(),
                points: node.point_count(),
                local_grid,
                cells: &cells,
            },
        )
        .map_err(|error| LodDatasetError::CellDriver(error.to_string()))?;
        if packed.levels.is_empty() {
            debug_assert!(
                blocks.iter().all(|block| block
                    .points()
                    .points()
                    .iter()
                    .all(|point| point.position.iter().any(|value| !value.is_finite()))),
                "node {id:?} has no cell levels but has a finite point"
            );
            self.cells.empty.insert(id);
            return Ok(None);
        }
        // The epoch of the first cut that can name it.
        let sent = self.cells.cut_epoch + 1;
        let bytes = packed_node_bytes(&packed);
        let coarsest_pages = coarsest_level_pages(&packed);
        let rebuild = !self.cells.ever_built.insert(id);
        let role = BuildRole::default();
        self.cells.built.insert(
            id,
            BuiltNode {
                sent,
                bytes,
                coarsest_pages,
                role,
                rebuild,
                color_gen: self.cells.color_gen,
            },
        );
        Ok(Some(packed))
    }

    /// Builds and packs the cells of `depths` of `id` from its decoded blocks, alone (the rest of
    /// the node is not touched). The packed node holds the non-empty levels, if any, and the
    /// depths `node_cell_level` could not build (see [`apply_level_requests`](Self::apply_level_requests)).
    /// `Ok((None, ..))` also when a block is not cached.
    fn build_node_levels(
        &self,
        id: NodeId,
        depths: &[u8],
    ) -> Result<(Option<PackedNode>, Vec<u8>), LodDatasetError> {
        let hierarchy = self.metadata.hierarchy();
        let Some(node) = hierarchy.node(id) else {
            return Ok((None, Vec::new()));
        };
        let blocks: Option<Vec<PointBlock>> = node
            .blocks()
            .iter()
            .map(|block| self.cache.peek(*block).cloned())
            .collect();
        let Some(blocks) = blocks else {
            return Ok((None, Vec::new()));
        };
        let origin = self.coordinate_origin();
        let world = self.cell_grid();
        let local_grid = CellGrid {
            root_min: std::array::from_fn(|axis| world.root_min[axis] - origin[axis]),
            root_side: world.root_side,
        };
        let local = SpatialBounds::new(
            std::array::from_fn(|axis| node.bounds().min()[axis] - origin[axis]),
            std::array::from_fn(|axis| node.bounds().max()[axis] - origin[axis]),
        )
        .map_err(|error| LodDatasetError::CellDriver(error.to_string()))?;
        let is_leaf = cell_leaf(hierarchy, id);
        let mut depths = depths.to_vec();
        depths.sort_unstable();
        depths.dedup();
        let (mut levels, mut unservable) = (Vec::new(), Vec::new());
        for depth in depths {
            match node_cell_level(&local_grid, local, node.spacing(), is_leaf, &blocks, depth) {
                Ok(level) => levels.push(level),
                Err(NodeCellError::AbsoluteDepthExceeded) => unservable.push(depth),
                Err(NodeCellError::TooSparseToPage) => {
                    // Sparser with depth: refuse down to the first sparse depth too, so the
                    // render side's maximum drops there at once instead of one depth per round.
                    let sparse = |depth| {
                        matches!(
                            node_cell_level(
                                &local_grid,
                                local,
                                node.spacing(),
                                is_leaf,
                                &blocks,
                                depth
                            ),
                            Err(NodeCellError::TooSparseToPage)
                        )
                    };
                    let (mut ok, mut first_sparse) = (0, depth);
                    while first_sparse - ok > 1 {
                        let middle = ok + (first_sparse - ok) / 2;
                        if sparse(middle) {
                            first_sparse = middle;
                        } else {
                            ok = middle;
                        }
                    }
                    unservable.push(depth);
                    if first_sparse < depth {
                        unservable.push(first_sparse);
                    }
                }
                Err(error) => return Err(LodDatasetError::CellDriver(error.to_string())),
            }
        }
        let (Some(first), Some(last)) = (
            levels.first().map(|level| level.depth),
            levels.last().map(|level| level.depth),
        ) else {
            return Ok((None, unservable));
        };
        // `add_packed_levels` ignores `buildable`: the node keeps the range it was stored with.
        let cells = NodeCells {
            node: id,
            levels,
            buildable: (first, last),
        };
        let packed = pack_node(
            &world,
            origin,
            NodeInput {
                id,
                bounds: node.bounds(),
                points: node.point_count(),
                local_grid,
                cells: &cells,
            },
        )
        .map_err(|error| LodDatasetError::CellDriver(error.to_string()))?;
        Ok(((!packed.levels.is_empty()).then_some(packed), unservable))
    }

    /// Largest and over-one depth gaps between each cut node and its nearest cut ancestor. The
    /// gap is the difference of `representative_depth` on the world grid, which is what the node
    /// mask of the cut limits (not the hierarchy level).
    pub(super) fn cut_gaps(&self, cut: &[NodeId]) -> Result<(u32, usize), LodDatasetError> {
        let hierarchy = self.metadata.hierarchy();
        let grid = self.cell_grid();
        let depth =
            |node: &viewer_core::SpatialNode| u32::from(representative_depth(&grid, node.bounds()));
        let in_cut: HashSet<NodeId> = cut.iter().copied().collect();
        let (mut max_gap, mut over_one) = (0, 0);
        for id in cut {
            let Some(node) = hierarchy.node(*id) else {
                continue;
            };
            let mut parent = node.parent();
            while let Some(ancestor_id) = parent {
                let Some(ancestor) = hierarchy.node(ancestor_id) else {
                    break;
                };
                if in_cut.contains(&ancestor_id) {
                    let gap = depth(node).saturating_sub(depth(ancestor));
                    if gap > MAX_CELL_CUT_GAP {
                        return Err(LodDatasetError::CellCutGap {
                            ancestor: ancestor_id,
                            descendant: *id,
                            gap,
                            max: MAX_CELL_CUT_GAP,
                        });
                    }
                    max_gap = max_gap.max(gap);
                    over_one += usize::from(gap > 1);
                    break;
                }
                parent = ancestor.parent();
            }
        }
        Ok((max_gap, over_one))
    }

    /// Step 1 of a cells update: select at `stage_px`, read at most one block, build the cells of
    /// up to `config.max_node_builds` nodes (nearest first, parents before children: R3-Q22)
    /// and plan with only built nodes resident. The driver choosing cell levels for the target
    /// px is step 2's business.
    ///
    /// On an error the update stops where it failed: blocks read stay in the cache, the nodes
    /// built by this call are forgotten (their cells were never sent; they are built again),
    /// the plan is not committed and `visible_plan` keeps the previous one.
    pub fn plan_cell_update(
        &mut self,
        view: &ViewProjection,
        stage_px: f64,
        config: CellUpdateConfig,
        async_priority_order: Option<&mut Vec<BlockId>>,
        mut load_block: impl FnMut(BlockId) -> Result<Option<PointBlock>, String>,
    ) -> Result<CellUpdate, LodDatasetError> {
        if !self.supports_cells() {
            return Err(LodDatasetError::CellPathUnsupported);
        }
        self.refinement_work.update_calls += 1;
        let coordinate_origin = self.coordinate_origin();
        let (frustum, selection) = self.select_target(view, stage_px)?;

        // Nodes without cells that this update works on: the selected ones, the co-owners of a
        // block shared with a node that has cells (a block turns resident only when all its
        // owners have cells, so they would wait forever otherwise) and, before any of them, the
        // ancestors that have no cells yet, in the cut or not (R3-Q22).
        let waiters = self.shared_block_waiters(selection.node_ids());
        let shared_owners = self.shared_block_owners(selection.node_ids());
        let selected = selection.node_ids().len();
        let mut seen = HashSet::new();
        let mut candidates: Vec<BuildCandidate> = Vec::new();
        for (index, id) in selection.node_ids().iter().chain(&waiters).enumerate() {
            if self.cells.has_cells(*id) {
                continue;
            }
            let waiter = index >= selected;
            let ancestors = self.ancestors_without_cells(*id);
            let hierarchy = self.metadata.hierarchy();
            let own = hierarchy
                .node(*id)
                .filter(|node| !node.blocks().is_empty())
                .map(|_| (*id, false));
            for (node_id, ancestor) in own
                .into_iter()
                .chain(ancestors.into_iter().map(|a| (a, true)))
            {
                if !seen.insert(node_id) {
                    continue;
                }
                let Some(node) = hierarchy.node(node_id) else {
                    continue;
                };
                candidates.push(BuildCandidate {
                    waiter,
                    ancestor,
                    distance: self.distance_to_eye(view, node.bounds()),
                    id: node_id,
                });
            }
        }
        candidates.sort_by(|a, b| {
            a.waiter
                .cmp(&b.waiter)
                .then(a.distance.total_cmp(&b.distance))
                .then(a.id.cmp(&b.id))
        });
        let uncached_blocks = |this: &Self, only_ancestors: bool| -> Vec<BlockId> {
            let hierarchy = this.metadata.hierarchy();
            let mut blocks: Vec<BlockId> = candidates
                .iter()
                .filter(|c| c.ancestor || !only_ancestors)
                .filter_map(|c| hierarchy.node(c.id))
                .flat_map(|node| node.blocks().iter().copied())
                .filter(|block| !this.cache.contains(*block))
                .collect();
            blocks.sort();
            blocks.dedup();
            blocks
        };

        let initial_plan = self.prepare_cell_plan(&selection);
        let in_initial_cut: HashSet<NodeId> = initial_plan.node_ids().iter().copied().collect();
        // CPU budget fit (ADR 0037). `cpu_needed` is what the driver cannot forget for this
        // update: the packed bytes of the built nodes in the cut or in the support set; every
        // node admitted below adds its own. A candidate is admitted only while that stays within
        // `CPU_ADMIT_PERCENT` of the budget. The first one that does not fit is built (its size
        // is known only then) and taken back, and `cpu_blocked` is set: while it holds no
        // candidate is admitted, so nothing is read or built for them, until the bytes the cut
        // needs fall below `CPU_RELEASE_PERCENT` (the camera moved to a cut that is cheaper).
        // Blocked candidates stay candidates without cells: their built ancestors remain support
        // and keep drawing their zone, and they are not deferred work, so the px ladder ends.
        //
        // The GPU pool fit (ADR 0040) is the same with another measure: `gpu_needed` adds the
        // pages of the coarsest stored level of each of those nodes (the least the pool needs to
        // draw them, since it coarsens levels within nodes first, ADR 0033), against the pages
        // the pool can use; it blocks, releases and counts apart (`gpu_blocked`).
        let in_progress = self.in_progress_children(&selection, initial_plan.node_ids());
        let selection_key = {
            let mut hasher = std::hash::DefaultHasher::new();
            std::hash::Hash::hash(selection.node_ids(), &mut hasher);
            std::hash::Hasher::finish(&hasher)
        };
        let (mut cpu_needed, mut gpu_needed) = (0, 0);
        // The built nodes the budgets above already count.
        let mut counted: HashSet<NodeId> = HashSet::new();
        let gpu_budget = config.gpu_pool_pages.map(viewer_render_wgpu::usable_pages);
        if config.cpu_budget_bytes.is_some() || gpu_budget.is_some() {
            // In-progress children count while blocked too: the driver keeps them until it is
            // over budget, and counting them keeps the block from flapping.
            let needed: HashSet<NodeId> = initial_plan
                .node_ids()
                .iter()
                .copied()
                .chain(self.support_nodes(&candidates, &shared_owners))
                .chain(in_progress.iter().copied())
                .collect();
            let bytes = |built: &BuiltNode| built.bytes;
            let pages = |built: &BuiltNode| built.coarsest_pages;
            counted = needed.clone();
            cpu_needed = self.cells.sum_over(&needed, bytes);
            gpu_needed = self.cells.sum_over(&needed, pages);
            let in_progress_cpu = self.cells.sum_over(&in_progress, bytes);
            let in_progress_gpu = self.cells.sum_over(&in_progress, pages);
            let (rejected_cpu, rejected_gpu) = match self.cells.rejected {
                Some((k, cpu, gpu)) if k == selection_key => (cpu, gpu),
                _ => (0, 0),
            };
            if let Some(budget) = config.cpu_budget_bytes {
                update_block(
                    &mut self.cells.cpu_blocked,
                    &mut self.cells.released,
                    selection_key,
                    (cpu_needed + rejected_cpu, in_progress_cpu),
                    (budget, CPU_RELEASE_PERCENT),
                );
            }
            if let Some(budget) = gpu_budget {
                update_block(
                    &mut self.cells.gpu_blocked,
                    &mut self.cells.gpu_released,
                    selection_key,
                    (gpu_needed + rejected_gpu, in_progress_gpu),
                    (budget, GPU_RELEASE_PERCENT),
                );
            }
            if !self.cells.cpu_blocked && !self.cells.gpu_blocked {
                self.cells.rejected = None;
            }
        }
        // Blocks that are decoded but wait for their cells are not read again. The ancestor whose
        // own ancestors all have cells waits for its blocks before anything else: its
        // descendants cannot be built without it, and it is built the moment it is read. In the
        // one-block sync step, reads alternate with the selection's, so a chain block that cannot
        // stay cached cannot starve the blocks in view. The async order puts the chain first
        // instead (alternating cancels reads in flight): a chain block that cannot stay cached
        // would keep its read slot there.
        let top_blocks: Vec<BlockId> = {
            let hierarchy = self.metadata.hierarchy();
            let mut blocks: Vec<BlockId> = candidates
                .iter()
                .filter(|c| c.ancestor && self.ancestors_without_cells(c.id).is_empty())
                .filter_map(|c| hierarchy.node(c.id))
                .flat_map(|node| node.blocks().iter().copied())
                .filter(|block| !self.cache.contains(*block))
                .collect();
            blocks.sort();
            blocks.dedup();
            blocks
        };
        // Nodes built again for a new colour generation have their blocks counted as resident by
        // the plan (the old cells are drawable), so their blocks are asked for here: they were
        // dropped with the old paint.
        let recoloring = candidates
            .iter()
            .any(|c| self.cells.built.contains_key(&c.id));
        let mut selection_blocks: Vec<BlockId> = initial_plan
            .pending_blocks()
            .iter()
            .copied()
            .filter(|block| !self.cache.contains(*block))
            .collect();
        if recoloring {
            // Only those whose ancestors have their new cells: parents come first.
            let hierarchy = self.metadata.hierarchy();
            selection_blocks.extend(
                candidates
                    .iter()
                    .filter(|c| self.ancestors_without_cells(c.id).is_empty())
                    .filter_map(|c| hierarchy.node(c.id))
                    .flat_map(|node| node.blocks().iter().copied())
                    .filter(|block| !self.cache.contains(*block)),
            );
            selection_blocks.sort();
            selection_blocks.dedup();
        }
        // Blocks of the nodes with requested levels: read through the same list.
        let level_blocks = self.level_request_blocks();
        if !level_blocks.is_empty() {
            selection_blocks.extend(level_blocks);
            selection_blocks.sort();
            selection_blocks.dedup();
        }
        let mut async_priority_order = async_priority_order;
        let async_order_wanted = async_priority_order.is_some();
        let chain_blocks: HashSet<BlockId> = if async_order_wanted {
            top_blocks.iter().copied().collect()
        } else {
            HashSet::new()
        };
        let read_ancestors = !top_blocks.is_empty()
            && !(self.cells.read_ancestor_last && !selection_blocks.is_empty());
        let to_read = if self.cells.cpu_blocked || self.cells.gpu_blocked {
            Vec::new()
        } else if async_order_wanted {
            // The host reads the whole order at once and cancels the reads that leave it: an
            // order alternating between the chain and the selection would cancel every read
            // before its bytes arrive. The alternation above is for the one-block sync step.
            let mut blocks = top_blocks;
            blocks.extend(selection_blocks);
            blocks.sort();
            blocks.dedup();
            blocks
        } else if read_ancestors {
            top_blocks
        } else {
            selection_blocks
        };
        let mut next_block = self.next_block_to_load(
            view,
            &frustum,
            &to_read,
            initial_plan.uncovered_nodes(),
            async_priority_order.as_deref_mut(),
        );
        if let Some(order) = async_priority_order {
            // Parents first (R3-Q22): the chain's blocks lead the order, each part keeping its
            // priority order, so the selection cannot take the read slots ahead of its ancestors.
            let (mut ordered, selection): (Vec<_>, Vec<_>) = order
                .iter()
                .copied()
                .partition(|block| chain_blocks.contains(block));
            ordered.extend(selection);
            *order = ordered;
            next_block = order.first().copied();
        }
        let mut evicted_blocks = std::mem::take(&mut self.arrival_evictions);
        self.arrived_blocks = 0;
        if let Some(requested) = next_block {
            if !async_order_wanted {
                self.cells.read_ancestor_last = read_ancestors;
            }
            self.load_block_step(requested, &mut load_block, &mut evicted_blocks)?;
        }
        self.cells.diag.block_evictions += evicted_blocks.len() as u64;

        // A candidate is ready when its blocks are decoded and every ancestor with blocks has
        // its cells.
        let ready = |this: &Self, id: NodeId| {
            this.metadata
                .hierarchy()
                .node(id)
                .is_some_and(|node| node.blocks().iter().all(|b| this.cache.contains(*b)))
                && this.ancestors_without_cells(id).is_empty()
        };
        let build_started = WorkClock::start();
        let mut built_nodes: Vec<NodeId> = Vec::new();
        let mut new_nodes: Vec<(u64, PackedNode)> = Vec::new();
        let generation = self.cell_generation();
        // What the driver holds for the nodes built below, to give back when a build is taken
        // back: a node of an older colour generation is still drawn.
        let mut previous: HashMap<NodeId, BuiltNode> = HashMap::new();
        while built_nodes.len() < config.max_node_builds
            && !(self.cells.cpu_blocked || self.cells.gpu_blocked)
        {
            let Some(id) = candidates
                .iter()
                .map(|c| c.id)
                .find(|id| !built_nodes.contains(id) && ready(self, *id))
            else {
                break;
            };
            if let Some(before) = self.cells.built.get(&id) {
                previous.insert(id, *before);
            }
            match self.build_node_cells(id) {
                Ok(packed) => {
                    // A node built again replaces its own bytes in the budgets.
                    let (old_bytes, old_pages) = previous
                        .get(&id)
                        .filter(|_| counted.contains(&id))
                        .map_or((0, 0), |built| (built.bytes, built.coarsest_pages));
                    let bytes = self.cells.built.get(&id).map_or(0, |built| built.bytes);
                    let pages = self
                        .cells
                        .built
                        .get(&id)
                        .map_or(0, |built| built.coarsest_pages);
                    let cpu_over = config.cpu_budget_bytes.is_some_and(|b| {
                        cpu_needed + bytes - old_bytes.min(bytes) > share(b, CPU_ADMIT_PERCENT)
                    });
                    let gpu_over = gpu_budget.is_some_and(|b| {
                        gpu_needed + pages - old_pages.min(pages) > share(b, GPU_ADMIT_PERCENT)
                    });
                    if cpu_over || gpu_over {
                        if let Some(before) = previous.remove(&id) {
                            self.cells.built.insert(id, before);
                        } else if let Some(taken) = self.cells.built.remove(&id)
                            && !taken.rebuild
                        {
                            self.cells.ever_built.remove(&id);
                        }
                        self.cells.diag.rejected_builds += 1;
                        self.cells.cpu_blocked |= cpu_over;
                        self.cells.gpu_blocked |= gpu_over;
                        self.cells.rejected = Some((
                            selection_key,
                            if cpu_over { bytes } else { 0 },
                            if gpu_over { pages } else { 0 },
                        ));
                        break;
                    }
                    if let Some(built) = self.cells.built.get_mut(&id) {
                        built.role = candidates.iter().find(|c| c.id == id).map_or(
                            BuildRole::Selected,
                            |c| match (c.ancestor, c.waiter) {
                                (true, _) => BuildRole::Ancestor,
                                (false, true) => BuildRole::Waiter,
                                (false, false) => BuildRole::Selected,
                            },
                        );
                        if built.rebuild {
                            self.cells.diag.rebuilt_total += 1;
                        }
                    }
                    cpu_needed = cpu_needed + bytes - old_bytes.min(bytes);
                    gpu_needed = gpu_needed + pages - old_pages.min(pages);
                    built_nodes.push(id);
                    new_nodes.extend(packed.map(|node| (generation, node)));
                }
                Err(error) => {
                    self.forget_unsent(&new_nodes, &previous);
                    return Err(error);
                }
            }
        }
        let (new_levels, levels_deferred) = match self.serve_level_requests(
            config.max_level_builds,
            generation,
            LevelRoom {
                cut: &in_initial_cut,
                admit: config
                    .cpu_budget_bytes
                    .map(|budget| share(budget, CPU_ADMIT_PERCENT)),
                needed: &mut cpu_needed,
                counted: &counted,
            },
        ) {
            Ok(served) => served,
            Err(error) => {
                self.forget_unsent(&new_nodes, &previous);
                return Err(error);
            }
        };
        let build_ms = build_started.elapsed_ms();
        let (cpu_blocked, gpu_blocked) = (self.cells.cpu_blocked, self.cells.gpu_blocked);
        let any_blocked = cpu_blocked || gpu_blocked;
        let nodes_deferred = if any_blocked {
            0
        } else {
            candidates
                .iter()
                .filter(|c| !built_nodes.contains(&c.id) && ready(self, c.id))
                .count()
        };
        let build_deferred = levels_deferred + nodes_deferred;
        let unbuilt = candidates
            .iter()
            .filter(|c| !built_nodes.contains(&c.id))
            .count();
        let cpu_coarsened = if cpu_blocked { unbuilt } else { 0 };
        let gpu_coarsened = if gpu_blocked { unbuilt } else { 0 };
        if !built_nodes.is_empty() || !new_levels.is_empty() || next_block.is_some() {
            self.state_epoch += 1;
        }
        let mut waiting_blocks = uncached_blocks(self, !recoloring);
        let plan = self.prepare_cell_plan(&selection);
        let mut support =
            self.support_nodes(&candidates, &self.shared_block_owners(selection.node_ids()));
        if !any_blocked {
            // Protected so the driver does not drop them before their group completes. While
            // blocked the group may never complete: they are released, the cut stays at the
            // parent (counted in `cpu_coarsened`) and they are not built again.
            if built_nodes.is_empty() {
                support.extend(in_progress);
            } else {
                support.extend(self.in_progress_children(&selection, plan.node_ids()));
            }
            support.sort_unstable();
            support.dedup();
        }
        let evicted_pending = evicted_blocks
            .iter()
            .filter(|id| plan.pending_blocks().binary_search(id).is_ok())
            .count() as u64;
        self.refinement_work.evicted_pending_blocks += evicted_pending;
        let cut: Vec<NodeId> = plan
            .node_ids()
            .iter()
            .copied()
            .filter(|id| self.cells.built.contains_key(id))
            .collect();
        let diag = {
            let in_cut: HashSet<NodeId> = cut.iter().copied().collect();
            let hierarchy = self.metadata.hierarchy();
            let mut not_in_cut = 0;
            let mut blocks_missing = 0;
            for id in selection.node_ids() {
                if !self.cells.built.contains_key(id) || in_cut.contains(id) {
                    continue;
                }
                not_in_cut += 1;
                if hierarchy
                    .node(*id)
                    .is_some_and(|node| node.blocks().iter().any(|b| !self.cache.contains(*b)))
                {
                    blocks_missing += 1;
                }
            }
            CellDiag {
                selection_nodes: selection.node_ids().len(),
                selected_built_not_in_cut: not_in_cut,
                selected_built_blocks_missing: blocks_missing,
                cpu_needed_bytes: cpu_needed,
                block_cache_bytes: self.cache.used_bytes() as u64,
                block_cache_limit_bytes: self.cache.max_bytes() as u64,
                ..self.cells.diag
            }
        };
        let (max_gap, gaps_over_one) = match self.cut_gaps(&cut) {
            Ok(gaps) => gaps,
            Err(error) => {
                self.forget_unsent(&new_nodes, &previous);
                return Err(error);
            }
        };

        self.state_epoch += 1;
        self.planner.commit_unprotected(&plan, &mut self.cache);
        self.touch_pages(plan.node_ids());
        self.cells.cut_epoch += 1;
        let cut_epoch = self.cells.cut_epoch;
        self.cells.unacked.push((cut_epoch, cut.clone()));
        let pending_anchors = pending_page_anchors(self.metadata.hierarchy(), &selection, |node| {
            view.projected_error_pixels(
                node.bounds(),
                node.representative_spacing(),
                coordinate_origin,
            )
        });
        let mut pending_block_ids = plan.pending_blocks().to_vec();
        pending_block_ids.append(&mut waiting_blocks);
        if any_blocked {
            // The blocks of candidates the budget keeps from being built are not asked for.
            let hierarchy = self.metadata.hierarchy();
            let unwanted: HashSet<BlockId> = candidates
                .iter()
                .filter(|c| !built_nodes.contains(&c.id))
                .filter_map(|c| hierarchy.node(c.id))
                .flat_map(|node| node.blocks().iter().copied())
                .collect();
            pending_block_ids.retain(|id| !unwanted.contains(id));
        }
        if !any_blocked {
            pending_block_ids.extend(self.level_request_blocks());
        }
        pending_block_ids.sort();
        pending_block_ids.dedup();
        let stats = CellPlanStats {
            built_nodes,
            build_deferred,
            nodes_deferred,
            build_capped: build_deferred > 0,
            build_ms,
            cut_nodes: cut.len(),
            max_gap,
            gaps_over_one,
            pending_block_ids,
            pending_page_anchors: pending_anchors,
            uncovered_nodes: plan.uncovered_nodes().len(),
            cpu_coarsened,
            gpu_coarsened,
            gpu_pool_pages: config.gpu_pool_pages.unwrap_or(0),
            stage_pixels: stage_px,
            diag,
        };
        self.visible_plan = Some(plan);
        Ok(CellUpdate {
            cut_epoch,
            cut,
            support,
            new_nodes,
            new_levels,
            refused_levels: std::mem::take(&mut self.cells.refused_levels),
            stats,
        })
    }

    /// Nodes with cells the driver must keep although the cut does not name them: ancestors of
    /// candidates still without cells, and the built co-owners of blocks of selected nodes with
    /// cells (`shared_owners`). A block is resident for the planner only when all its
    /// owners have cells, so forgetting one of them would make it a waiter again, to be built and
    /// forgotten over and over at the CPU budget. `shared_owners` also count in the CPU fit.
    fn support_nodes(
        &self,
        candidates: &[BuildCandidate],
        shared_owners: &[NodeId],
    ) -> Vec<NodeId> {
        let hierarchy = self.metadata.hierarchy();
        let mut ancestors = HashSet::new();
        for candidate in candidates.iter().filter(|c| !self.cells.has_cells(c.id)) {
            let mut parent = hierarchy.node(candidate.id).and_then(|node| node.parent());
            while let Some(up) = parent {
                if self.cells.built.contains_key(&up) && !ancestors.insert(up) {
                    break;
                }
                parent = hierarchy.node(up).and_then(|node| node.parent());
            }
        }
        let mut support = ancestors;
        support.extend(
            shared_owners
                .iter()
                .copied()
                .filter(|id| self.cells.built.contains_key(id)),
        );
        let mut support: Vec<NodeId> = support.into_iter().collect();
        support.sort_unstable();
        support
    }

    /// Built selected nodes the cut does not name: children of sibling groups that are not
    /// complete yet, so `resolve_replacement` keeps their parent in the cut (B1c.6).
    fn in_progress_children(&self, selection: &LodSelection, cut: &[NodeId]) -> Vec<NodeId> {
        let in_cut: HashSet<NodeId> = cut.iter().copied().collect();
        selection
            .node_ids()
            .iter()
            .copied()
            .filter(|id| self.cells.built.contains_key(id) && !in_cut.contains(id))
            .collect()
    }

    /// Takes back nodes whose message was never produced.
    fn forget_unsent(
        &mut self,
        nodes: &[(u64, PackedNode)],
        previous: &HashMap<NodeId, BuiltNode>,
    ) {
        for (_, node) in nodes {
            match previous.get(&node.id) {
                Some(before) => self.cells.built.insert(node.id, *before),
                None => self.cells.built.remove(&node.id),
            };
        }
    }

    /// One update of the cells path in one thread: [`plan_cell_update`](Self::plan_cell_update),
    /// the driver's side on `driver` and `target`, and the dataset's side again, back to back.
    #[allow(clippy::too_many_arguments)]
    pub fn update_cells<T: CellTarget>(
        &mut self,
        view: &ViewProjection,
        target_px: f64,
        config: CellUpdateConfig,
        driver: &mut CellDriver,
        target: &mut T,
        async_priority_order: Option<&mut Vec<BlockId>>,
        load_block: impl FnMut(BlockId) -> Result<Option<PointBlock>, String>,
    ) -> Result<LodCellUpdate, LodDatasetError> {
        self.update_cells_at_stage(
            view,
            target_px,
            target_px,
            config,
            driver,
            target,
            async_priority_order,
            load_block,
        )
    }

    /// [`LodDataset::update_cells`] selecting the nodes at `stage_px` (a rung of the px ladder,
    /// see `lod_driver::update_cells_staged`) while the driver still chooses cell levels for
    /// `target_px`. Coarser nodes get their cells first and stay as the fallback of the finer
    /// ones (R3-Q19).
    ///
    /// The caller owns the ladder (`lod_driver::update_cells_staged` restarts it after a camera
    /// jump, R3-Q22). On an error the dataset keeps what was built and read so far, and the pool
    /// pins change only as `pool_drawn` documents.
    #[allow(clippy::too_many_arguments)]
    pub fn update_cells_at_stage<T: CellTarget>(
        &mut self,
        view: &ViewProjection,
        stage_px: f64,
        target_px: f64,
        config: CellUpdateConfig,
        driver: &mut CellDriver,
        target: &mut T,
        async_priority_order: Option<&mut Vec<BlockId>>,
        load_block: impl FnMut(BlockId) -> Result<Option<PointBlock>, String>,
    ) -> Result<LodCellUpdate, LodDatasetError> {
        self.reconcile_driver(driver)?;
        let update =
            self.plan_cell_update(view, stage_px, config, async_priority_order, load_block)?;
        self.draw_planned_cells(update, view, target_px, driver, target)
    }

    /// Steps 2 and 3 of an update planned by [`plan_cell_update`](Self::plan_cell_update), in
    /// one thread.
    pub(crate) fn draw_planned_cells<T: CellTarget>(
        &mut self,
        update: CellUpdate,
        view: &ViewProjection,
        target_px: f64,
        driver: &mut CellDriver,
        target: &mut T,
    ) -> Result<LodCellUpdate, LodDatasetError> {
        let CellUpdate {
            cut_epoch,
            cut,
            support,
            new_nodes,
            new_levels,
            refused_levels,
            stats,
        } = update;
        if let Err(error) =
            add_cell_nodes(driver, new_nodes).and_then(|()| add_cell_levels(driver, new_levels))
        {
            self.cell_frame_failed(cut_epoch);
            return Err(error);
        }
        driver.refuse_levels(&refused_levels);
        driver.cut_received();
        driver.set_support(&support);
        let draw = match draw_cell_cut(driver, target, cut_epoch, &cut, view, target_px) {
            Ok(draw) => draw,
            Err(error) => {
                self.cell_frame_failed(cut_epoch);
                return Err(error);
            }
        };
        let CellDraw {
            ack,
            evicted,
            pool_reset,
            report,
        } = draw;
        if pool_reset {
            self.pool_reset();
        }
        self.apply_evicted(&evicted);
        self.apply_ack(ack);
        // One thread: the driver's requests of this frame are served by the next update.
        if let Some(requests) = driver.take_level_requests() {
            self.apply_level_requests(&requests);
        }
        let stats = CellUpdateStats::new(stats, &report, driver.cpu_bytes());
        Ok(LodCellUpdate { stats, report })
    }
}

/// Whether `id` is a leaf for its cell levels: a page anchor that is not resident has children
/// we cannot see, so it is not a leaf.
pub(super) fn cell_leaf(hierarchy: &SpatialHierarchy, id: NodeId) -> bool {
    hierarchy.is_leaf(id) && !hierarchy.is_unloaded(id)
}
