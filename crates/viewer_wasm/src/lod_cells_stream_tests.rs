//! R3 Task 9b2a: the px ladder of the cells path, streamed block by block (R3-Q19), with a
//! strict recording `CellTarget` and a cell-level coverage oracle. The real-data tests need
//! `belvedere-v8.oct` in `RENDER_DATA_DIR` (default `target/render-data`, see
//! `cells_tests::render_data_dir`) and skip with a message without it, unless
//! `R3_REQUIRE_REAL_DATA=1` makes the missing file a failure.

use super::cells::MAX_CELL_CUT_GAP;
use super::cells_tests::{
    SIDE, driver_for, node_block, octree_dataset, octree_nodes, render_data_dir,
};
use super::*;
use crate::lod_driver::{CellStep, update_cells_staged};
use std::fmt::Write as _;
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
    time::Instant,
};
use viewer_core::{
    BlockId, ColorRgb, HierarchySemantics, Point, PointBatch, PointBlock, SpatialBounds,
    SpatialHierarchy, SpatialNode,
};
use viewer_format::{FormatReader, FormatReaderLimits, OctreeReader};
use viewer_render_wgpu::cell_lod::{
    LodView, MAX_NODE_SPAN, NodeLod, cell_lo, drawn_depth_at, edge_px, min_child_edge_px, refines,
};
use viewer_render_wgpu::{
    CellCutEntry, CellDriver, CellPoolError, CellTarget, CutApply, FramePlan, MASK_WORDS,
    NODE_WORDS, OrbitCamera, PAGE_SLOTS, PackedLevel, PageEntry, PageFrame, PageInfo, SceneBounds,
    ViewProjection, VoxelBuildLimits, cell_cut_digest, gpu_input_digest, node_view_range,
    representative_depth,
};
use viewer_render_wgpu::{
    CellGrid, MAX_CELL_DEPTH, node_cell_level, representative_depth_for_spacing,
};

/// Longest run of `RetainedPrevious` frames the pool may take while blocks stream in.
const MAX_RETAINED_STREAK: usize = 60;
/// Longest wait of a held or excluded node whose threshold level is requested (R3-Q20/Q21).
const MAX_HOLD_FRAMES: u32 = 120;

// ---------------------------------------------------------------------------------------------
// Strict recording target
// ---------------------------------------------------------------------------------------------

/// A `CellTarget` that records what the driver writes and fails (as a renderer would corrupt)
/// on the hazards of the pool protocol: a slot written while it is still owned or while the
/// current or the previous page table reads it, a slot freed while the previous table reads it,
/// and a table naming a slot nobody uploaded. `check_plan` also compares what the driver wrote
/// (masks, page table) with the plan it ran. It is the strict fake of the render crate (`test_support::FakeGpu`, `cfg(test)` there and
/// unreachable from this crate) restated over real `PackedLevel`s.
#[derive(Default)]
struct StrictTarget {
    pool: bool,
    eye: [f64; 3],
    /// Slot -> (node, depth) of the level that owns it.
    owner: HashMap<u32, (NodeId, u8)>,
    /// Slots written per (node, depth), in upload order.
    slots_of: HashMap<(NodeId, u8), Vec<u32>>,
    masks: HashMap<u32, [u32; MASK_WORDS]>,
    /// The page the upload put in each slot.
    page_at: HashMap<u32, PageInfo>,
    next_mask: u32,
    /// Slots written since the last page table.
    frame_writes: Vec<u32>,
    prev_active: HashSet<u32>,
    table: Vec<(u32, PageEntry)>,
    page_frame: Option<PageFrame>,
    uploads_total: usize,
    /// Hash of the cells each slot holds (colours included), to tell recoloured pages apart.
    page_cells: HashMap<u32, u64>,
    /// Node records and page tables written, and the last LOD view the driver passed.
    records_written: usize,
    tables_written: usize,
    lod_view: Option<LodView>,
}

impl CellTarget for StrictTarget {
    type Error = CellPoolError;
    fn has_pool(&self) -> bool {
        self.pool
    }
    fn drop_pool(&mut self) {
        *self = Self {
            eye: self.eye,
            ..Self::default()
        };
    }
    fn eye(&self) -> [f64; 3] {
        self.eye
    }
    fn upload_level(
        &mut self,
        level: &PackedLevel,
        page_slots: &[u32],
    ) -> Result<(), CellPoolError> {
        self.pool = true;
        if level.pages.len() != page_slots.len() {
            return Err(CellPoolError::PageCountMismatch {
                pages: level.pages.len(),
                slots: page_slots.len(),
            });
        }
        for (index, (slot, page)) in page_slots.iter().zip(&level.pages).enumerate() {
            let cells = &level.instances[index * PAGE_SLOTS..(index + 1) * PAGE_SLOTS];
            let hash = {
                let mut hasher = std::hash::DefaultHasher::new();
                for cell in cells {
                    std::hash::Hash::hash(&cell.0, &mut hasher);
                }
                std::hash::Hasher::finish(&hasher)
            };
            // A recolour writes the page its slot already holds: the queue orders it after the
            // draws already submitted, so it is not a hazard.
            if self.owner.get(slot) == Some(&(level.node, level.depth)) {
                self.page_cells.insert(*slot, hash);
                self.uploads_total += 1;
                continue;
            }
            self.page_cells.insert(*slot, hash);
            if self.owner.contains_key(slot) {
                return Err(CellPoolError::InvalidLevel(
                    "slot written while still owned",
                ));
            }
            self.owner.insert(*slot, (level.node, level.depth));
            self.page_at.insert(*slot, page.clone());
            self.slots_of
                .entry((level.node, level.depth))
                .or_default()
                .push(*slot);
            self.frame_writes.push(*slot);
            self.uploads_total += 1;
        }
        Ok(())
    }
    fn free_pages(&mut self, page_slots: &[u32]) -> Result<(), CellPoolError> {
        if !self.pool {
            return Err(CellPoolError::NoPool);
        }
        for slot in page_slots {
            if self.prev_active.contains(slot) {
                return Err(CellPoolError::InvalidLevel(
                    "slot freed while the previous table reads it",
                ));
            }
            self.page_at.remove(slot);
            if let Some(key) = self.owner.remove(slot)
                && let Some(slots) = self.slots_of.get_mut(&key)
            {
                slots.retain(|owned| owned != slot);
            }
        }
        Ok(())
    }
    fn alloc_mask_slot(&mut self) -> Result<Option<u32>, CellPoolError> {
        self.pool = true;
        self.next_mask += 1;
        Ok(Some(self.next_mask - 1))
    }
    fn set_node_record(
        &mut self,
        slot: u32,
        record: &[u32; NODE_WORDS],
    ) -> Result<(), CellPoolError> {
        if slot >= self.next_mask {
            return Err(CellPoolError::MaskSlotNotAllocated(slot));
        }
        self.records_written += 1;
        self.masks
            .insert(slot, record[..MASK_WORDS].try_into().unwrap());
        Ok(())
    }
    fn set_lod_view(&mut self, view: Option<LodView>) {
        self.lod_view = view;
    }
    fn set_active_pages(
        &mut self,
        pages: &[(u32, PageEntry)],
        frame: &PageFrame,
    ) -> Result<(), CellPoolError> {
        if !self.pool {
            return Err(CellPoolError::NoPool);
        }
        let mut current = HashSet::new();
        for (slot, _) in pages {
            if !self.owner.contains_key(slot) {
                return Err(CellPoolError::InvalidLevel(
                    "active slot was never uploaded",
                ));
            }
            if !current.insert(*slot) {
                return Err(CellPoolError::DuplicatePageSlot(*slot));
            }
        }
        if self
            .frame_writes
            .iter()
            .any(|slot| current.contains(slot) || self.prev_active.contains(slot))
        {
            return Err(CellPoolError::InvalidLevel(
                "slot written while the current or the previous table reads it",
            ));
        }
        self.frame_writes.clear();
        self.prev_active = current;
        self.tables_written += 1;
        self.table = pages.to_vec();
        self.page_frame = Some(*frame);
        Ok(())
    }
}

impl StrictTarget {
    /// What the target would show in colour: the sum over the active pages of a hash of the
    /// page's identity and cells. Independent of the slots the pages sit in.
    fn color_digest(&self) -> u64 {
        self.table.iter().fold(0_u64, |acc, (slot, _)| {
            let mut hasher = std::hash::DefaultHasher::new();
            std::hash::Hash::hash(&format!("{:?}", self.page_at[slot]), &mut hasher);
            std::hash::Hash::hash(&self.page_cells[slot], &mut hasher);
            acc.wrapping_add(std::hash::Hasher::finish(&hasher))
        })
    }

    /// The hash of the cells of every active page, by (node, depth, page of the level).
    fn active_cells(&self) -> HashMap<(NodeId, u8, usize), u64> {
        let active: HashSet<u32> = self.table.iter().map(|(slot, _)| *slot).collect();
        let mut out = HashMap::new();
        for ((node, depth), slots) in &self.slots_of {
            for (page, slot) in slots.iter().enumerate() {
                if active.contains(slot) {
                    out.insert((*node, *depth, page), self.page_cells[slot]);
                }
            }
        }
        out
    }

    /// What the driver left in the target equals what the plan asked for: each active node's
    /// mask slot holds the mask of its cut entry, and the page table lists, level by level, every
    /// page that was uploaded for it with the entry that page computes for the plan's mask slot
    /// and octants.
    fn check_plan(&self, plan: &FramePlan) {
        let mut expected: Vec<(u32, PageEntry)> = Vec::new();
        for active in &plan.active {
            let entry = plan
                .entries
                .iter()
                .find(|entry| entry.node == active.key.node)
                .expect("an active node is in the cut");
            let words: [u32; MASK_WORDS] =
                std::array::from_fn(|i| (entry.mask[i / 2] >> (32 * (i % 2))) as u32);
            assert_eq!(
                self.masks.get(&active.mask_slot),
                Some(&words),
                "mask slot {} of node {:?} differs from its cut entry",
                active.mask_slot,
                active.key.node
            );
            let frame = self.page_frame.as_ref().expect("a table was written");
            let slots = self
                .slots_of
                .get(&(active.key.node, active.key.depth))
                .unwrap_or_else(|| panic!("active level {:?} was never uploaded", active.key));
            for slot in slots {
                expected.push((
                    *slot,
                    self.page_at[slot].entry(
                        frame,
                        active.mask_slot,
                        u32::from(active.octant_bits),
                    ),
                ));
            }
        }
        assert_eq!(self.table, expected, "page table differs from the plan");
    }
}

// ---------------------------------------------------------------------------------------------
// Oracles
// ---------------------------------------------------------------------------------------------

/// Octant of the cell `c` of an 8^3 node grid (x is bit 2, z bit 0, as the cut does).
fn octant_of_cell(c: [usize; 3]) -> usize {
    ((c[0] / 4) << 2) | ((c[1] / 4) << 1) | (c[2] / 4)
}

fn cell_hidden(mask: &[u64; 8], c: [usize; 3]) -> bool {
    mask[c[2]] >> (c[0] + 8 * c[1]) & 1 == 1
}

/// The twin's per-cell wanted depth at the point `rel` of a node: the first depth of its span
/// (capped by the octant's wanted depth, which bounds the buildable range) whose cell holding the
/// point does not refine.
fn cell_wanted_depth(
    view: &LodView,
    node: &NodeLod,
    rel: [f64; 3],
    node_lo: [f64; 3],
    side_at_node_depth: f64,
    octant_wanted: u8,
) -> u8 {
    let last = octant_wanted.min(node.node_depth + MAX_NODE_SPAN);
    (node.node_depth..=last)
        .find(|depth| {
            let level = depth - node.node_depth;
            let side = side_at_node_depth / (1_u64 << level) as f64;
            let cells = (1_u64 << level) as f64;
            let cell: [u32; 3] = std::array::from_fn(|axis| {
                ((rel[axis] - node_lo[axis]) / side)
                    .floor()
                    .clamp(0.0, cells - 1.0) as u32
            });
            !refines(view, cell_lo(node, cell, side as f32), side as f32)
        })
        .unwrap_or(last)
}

struct CutView {
    nodes: Vec<NodeId>,
    bounds: Vec<SpatialBounds>,
    /// Drawn depth per octant: `plan.active` is the drawn cut (the retained one on
    /// `RetainedPrevious`, like `plan.entries`), not the planner's wish.
    active: Vec<[Option<u8>; 8]>,
    entries: Vec<CellCutEntry>,
    ancestors: Vec<Vec<usize>>,
    descendants: Vec<Vec<usize>>,
}

impl CutView {
    fn new(dataset: &LodDataset, plan: &FramePlan) -> Self {
        let hierarchy = dataset.metadata.hierarchy();
        let nodes: Vec<NodeId> = plan.entries.iter().map(|entry| entry.node).collect();
        let index: HashMap<NodeId, usize> =
            nodes.iter().enumerate().map(|(i, id)| (*id, i)).collect();
        let bounds = nodes
            .iter()
            .map(|id| hierarchy.node(*id).expect("cut node is resident").bounds())
            .collect();
        let mut active = vec![[None; 8]; nodes.len()];
        for level in &plan.active {
            let at = index[&level.key.node];
            for (octant, slot) in active[at].iter_mut().enumerate() {
                if level.octant_bits >> octant & 1 == 1 {
                    *slot = Some(level.key.depth);
                }
            }
        }
        let mut ancestors = vec![Vec::new(); nodes.len()];
        let mut descendants = vec![Vec::new(); nodes.len()];
        for (i, id) in nodes.iter().enumerate() {
            let mut parent = hierarchy.node(*id).and_then(SpatialNode::parent);
            while let Some(up) = parent {
                if let Some(j) = index.get(&up) {
                    ancestors[i].push(*j);
                    descendants[*j].push(i);
                }
                parent = hierarchy.node(up).and_then(SpatialNode::parent);
            }
        }
        Self {
            nodes,
            bounds,
            active,
            entries: plan.entries.clone(),
            ancestors,
            descendants,
        }
    }

    /// The cell of entry `i` holding `p`, if `p` is inside its bounds.
    fn cell_at(&self, i: usize, p: [f64; 3]) -> Option<[usize; 3]> {
        let b = self.bounds[i];
        let mut cell = [0; 3];
        for axis in 0..3 {
            let (min, max) = (b.min()[axis], b.max()[axis]);
            if p[axis] < min || p[axis] > max {
                return None;
            }
            cell[axis] = (((p[axis] - min) / (max - min) * 8.0) as usize).min(7);
        }
        Some(cell)
    }

    fn cell_center(&self, i: usize, c: [usize; 3]) -> [f64; 3] {
        let b = self.bounds[i];
        std::array::from_fn(|axis| {
            b.min()[axis] + (b.max()[axis] - b.min()[axis]) * (c[axis] as f64 + 0.5) / 8.0
        })
    }

    /// Entry `j` draws at `p`: its octant is drawn by a resident level and the cell is not hidden.
    fn draws_at(&self, j: usize, p: [f64; 3]) -> bool {
        let Some(c) = self.cell_at(j, p) else {
            return false;
        };
        self.active[j][octant_of_cell(c)].is_some() && !cell_hidden(&self.entries[j].mask, c)
    }

    /// Cell-level coverage: every cell of every octant the node wants is drawn by the node's own
    /// resident level, or lies under an ancestor of the cut that draws it (and does not hide it),
    /// or is hidden under a descendant of the cut that draws it. Independent of the render
    /// crate's `assert_no_holes` (octant level, tied to its fixtures): this one works on the 8^3
    /// cells and the masks of the real plan.
    fn holes(&self) -> Vec<String> {
        let mut holes = Vec::new();
        for i in 0..self.nodes.len() {
            for cz in 0..8 {
                for cy in 0..8 {
                    for cx in 0..8 {
                        let c = [cx, cy, cz];
                        let o = octant_of_cell(c);
                        if self.entries[i].wanted[o].is_none() {
                            continue;
                        }
                        let hidden = cell_hidden(&self.entries[i].mask, c);
                        if !hidden && self.active[i][o].is_some() {
                            continue;
                        }
                        let p = self.cell_center(i, c);
                        let covered = self.ancestors[i].iter().any(|j| self.draws_at(*j, p))
                            || self.descendants[i].iter().any(|j| self.draws_at(*j, p));
                        if !covered {
                            holes.push(format!(
                                "node {:?} cell {c:?} (octant {o}, hidden {hidden})",
                                self.nodes[i]
                            ));
                        }
                    }
                }
            }
        }
        holes
    }
}

// ---------------------------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------------------------

type Blocks = Box<dyn FnMut(BlockId) -> Result<Option<PointBlock>, String>>;
type Pages = Box<dyn FnMut(NodeId) -> Result<PageInsert, String>>;

#[derive(Debug, Clone, Copy, PartialEq)]
struct FrameInfo {
    apply: CutApply,
    stage: f64,
    uploaded_pages: usize,
    built: usize,
    deferred: usize,
    nodes_deferred: usize,
    max_gap: u32,
    dtf: bool,
    digest: u64,
    pending_blocks: usize,
    cut_nodes: usize,
    build_ms: f64,
    cpu_cell_bytes: u64,
    resident_pages: u32,
    fallback_pages: u32,
    held_nodes: u32,
    excluded_nodes: u32,
    cpu_coarsened: usize,
    gpu_coarsened: usize,
    pool_overflow: u32,
    pool_coarsened: u32,
}

#[derive(Default, Debug)]
struct Totals {
    frames: usize,
    longest_retained: usize,
    max_gap: u32,
    gaps_over_one: usize,
    dtf: usize,
    uploads: usize,
    graft_refused: usize,
    grafted: usize,
    stage_drops: usize,
    /// Longest wait of a held or excluded node (`hold_age_max`) and the peaks of the pool counters.
    hold_age_max: u32,
    held_peak: u32,
    excluded_peak: u32,
    fallback_peak: u32,
    resident_peak: u32,
    hold_unsettled: u64,
    /// Held or excluded waits seen (frames with at least one).
    wait_frames: usize,
    /// The rungs of the ladder in the order they ran, each once.
    stages: Vec<f64>,
    /// Digest of the applied (drawn) cut of the last frame.
    drawn_digest: u64,
    /// Times each node was built, and nodes the driver forgot (CPU budget).
    builds: HashMap<NodeId, usize>,
    forgotten: usize,
    /// FNV-1a chain of `gpu_input_digest` over every drawn frame, and the digest of the last one.
    gpu_digest: u64,
    gpu_digest_last: u64,
    /// Nodes of a frame whose active set skips a buildable depth that is not usable
    /// (`check_active_gaps`), summed over frames, and the first such case.
    active_gaps: usize,
    first_gap: Option<String>,
    /// Where the single run of `node_run` differs from today's emitted set (`check_run_diff`).
    run_diff: RunDiff,
}

/// Classes of a difference between today's active set of a node and its `node_run` run, in the
/// order they are tried.
const RUN_CLASSES: [&str; 7] = [
    "gap",
    "entering",
    "prev_unusable",
    "above_hi",
    "below_lo",
    "fine_end",
    "unclassified",
];

#[derive(Default, Debug)]
struct RunDiff {
    /// Nodes compared, and the ones that differ per class.
    compared: usize,
    classes: [usize; 7],
    /// First example per class.
    examples: [Option<String>; 7],
    /// Differing nodes whose run top is below / above today's top.
    coarser: usize,
    finer: usize,
    first_coarser: Option<String>,
    /// Applied runs of the previous drawn frame, the `prev` of the next one.
    prev: HashMap<NodeId, (u8, u8)>,
}

/// `(node, wanted, drawn)` of an entry of the last plan.
type LastEntry = (NodeId, [Option<u8>; 8], [Option<u8>; 8]);

/// A cut node of a frame with its active depths and its range.
type NodeLevels = (NodeId, Vec<u8>, Option<(u8, u8)>);

/// The depths a node's active set skips: between its coarsest and finest active depth (`run`, its
/// D5 depth exempt), the buildable depths (`span`) that are not usable (not resident, or uploaded
/// this frame). A usable depth missing from the run is `check_active_runs`' case, not this one.
fn active_gaps_of(
    run: &[u8],
    d5: Option<u8>,
    span: (u8, u8),
    usable: impl Fn(u8) -> bool,
) -> Vec<u8> {
    let (Some(&first), Some(&last)) = (run.iter().min(), run.iter().max()) else {
        return Vec::new();
    };
    (first..=last)
        .filter(|depth| {
            !run.contains(depth)
                && Some(*depth) != d5
                && (span.0..=span.1).contains(depth)
                && !usable(*depth)
        })
        .collect()
}

struct Harness {
    dataset: LodDataset,
    driver: CellDriver,
    target: StrictTarget,
    stage: Option<f64>,
    dirty: bool,
    target_px: f64,
    config: CellUpdateConfig,
    blocks: Blocks,
    pages: Pages,
    retained_run: usize,
    /// Cross-node coarsenings: a sample point drawn at depth `before` is drawn at `after` with
    /// `after < min(before, wanted now)`.
    zone_coarsenings: usize,
    /// `zone_coarsenings` without the stage excuse: a staged octant drawn coarser than before,
    /// while its raw wanted depth did not get coarser, counts too. Each such drop is excused when
    /// the octant that draws the sample is staged below its wish and its node was capped by the
    /// fit that frame or the ladder ran coarser than the target; the rest are
    /// `unexcused_drops` (asserted 0).
    staged_coarsenings: usize,
    stage_excused_drops: usize,
    unexcused_drops: usize,
    /// The nodes the fit capped in the frame being checked.
    frame_capped: Vec<NodeId>,
    last_entries: Vec<LastEntry>,
    /// Per cut node of the last frame: its active depths and its range (cell mode).
    last_levels: Vec<NodeLevels>,
    frame_stage_px: f64,
    /// Sample points drawn in the previous cut, still inside the planner's cut, drawn by nothing.
    lost_samples: usize,
    /// The planner's nodes of the cut being drawn, when the dataset has already planned further
    /// (the worker sim): the lost-sample oracle must ask the plan of that cut, not the newest one.
    /// `None` reads `dataset.visible_plan`, which is the drawn cut's plan in one thread.
    planner_of_drawn_cut: Option<Vec<NodeId>>,
    /// Frames whose cut had holes (only counted, not asserted, when `soft_checks` is set).
    hole_frames: usize,
    /// First failed check, kept when `soft_checks` is set.
    first_failure: Option<String>,
    /// Buildable span of the nodes seen by `check_active_gaps` (`buildable_span` is costly).
    buildable_cache: HashMap<NodeId, (u8, u8)>,
    /// The bench counts holes, lost samples and long waits instead of failing the frame.
    soft_checks: bool,
    /// The plan of the last frame (cell oracle self-test: it is corrupted and checked again).
    last_plan: Option<FramePlan>,
    last_stage: Option<f64>,
    totals: Totals,
    /// The camera of the frame being checked, for the cell-mode twin.
    cell_ctx: Option<CellCtx>,
    /// Cell mode: what the last evaluation was made over, to skip identical frames.
    cell_signature: Option<u64>,
    /// Cell mode: drawn depth of each lattice sample (node, index) in the last evaluation.
    cell_memory: HashMap<(NodeId, u16), ([f64; 3], u8)>,
    /// Samples drawn before, for the lost-sample check: unlike `cell_memory` (the R3-Q21
    /// baseline) it survives `forget_zones`.
    cell_seen: HashMap<(NodeId, u16), ([f64; 3], u8)>,
    /// Cell-mode counters (reported, never asserted).
    cell_counters: CellCounters,
    /// ADR 0053 temporal gate counters.
    gate: StepGate,
    /// Cell mode: (drawing node, depth) of each lattice sample at the previous evaluation.
    cell_gate_prev: HashMap<(NodeId, u16), (NodeId, u8)>,
    /// Cell mode: the nodes of the last evaluated cut as the twin sees them.
    cell_nodes: Vec<CellNode>,
    /// Zoom-in lag measurement (ADR 0055 plan, task 7a); off unless a diagnostic turns it on.
    lag: LagLog,
}

/// Counters of the ADR 0053 temporal gate (measured, never asserted).
#[derive(Default, Debug, Clone, Copy)]
struct StepGate {
    /// Timeout hops the pool counted (global per frame).
    hops: usize,
    /// Samples whose depth changed by more than one level
    /// and whose drawing node changed between the two frames (hand-off between nodes).
    cell_node_changed: usize,
    /// ... and whose drawing node is the same (within-node jumps).
    cell_within_node: usize,
    /// Spatial gate: pairs of adjacent lattice samples drawn
    /// by different nodes, summed over the checked cuts ...
    cell_border_pairs: usize,
    /// ... whose drawn depths differ by more than one level (ADR 0053, between neighbouring cells).
    cell_border_over_one: usize,
    /// ... the same two counts over the pairs whose both samples are on screen (inside the
    /// frustum, between the render near and far planes): RF-31 concerns what the viewer sees.
    cell_border_visible_pairs: usize,
    cell_border_visible_over_one: usize,
}

/// Cell-mode counters of the sample oracle.
#[derive(Default, Debug, Clone, Copy)]
struct CellCounters {
    evaluations: usize,
    samples: usize,
    /// Samples drawn finer than the per-cell wanted depth: the active set had nothing closer.
    finer_than_wanted: usize,
    /// Samples drawn coarser than the per-cell wanted depth.
    coarser_than_wanted: usize,
}

/// A node of the cut for the cell-mode twin: bounds in the dataset frame, the rest eye-relative.
#[derive(Clone)]
struct CellNode {
    id: NodeId,
    min: [f64; 3],
    max: [f64; 3],
    lod: NodeLod,
    lo: [f64; 3],
    side: f64,
}

/// The frame of reference of the cell-mode twin: eye-relative scaled units.
struct CellCtx {
    lod: LodView,
    eye: [f64; 3],
    scale: f64,
    origin: [f64; 3],
    root_side: f64,
    /// Viewport size in pixels and render clip distances (scaled units) for the visibility test.
    viewport: [f64; 2],
    clip: (f64, f64),
    /// `lod_near_depth` (scaled units); `ViewFrustum`'s near plane is `max(near, clip.0)`.
    near: f64,
}

impl CellCtx {
    /// Whether an eye-relative point lies inside the view frustum and between the render clip
    /// planes.
    fn on_screen(&self, rel: [f64; 3]) -> bool {
        let dot = |v: [f32; 3]| (0..3).map(|a| rel[a] * f64::from(v[a])).sum::<f64>();
        let depth = dot(self.lod.forward);
        let focal = f64::from(self.lod.focal);
        depth >= self.clip.0
            && depth <= self.clip.1
            && dot(self.lod.right).abs() * focal <= depth * self.viewport[0] * 0.5
            && dot(self.lod.up).abs() * focal <= depth * self.viewport[1] * 0.5
    }
}

impl Harness {
    fn new(dataset: LodDataset, target_px: f64, blocks: Blocks, pages: Pages) -> Self {
        let driver = driver_for(&dataset, u64::MAX);
        Self {
            dataset,
            driver,
            target: StrictTarget::default(),
            stage: None,
            dirty: true,
            target_px,
            config: CellUpdateConfig::default(),
            blocks,
            pages,
            retained_run: 0,
            zone_coarsenings: 0,
            staged_coarsenings: 0,
            stage_excused_drops: 0,
            unexcused_drops: 0,
            frame_capped: Vec::new(),
            last_entries: Vec::new(),
            last_levels: Vec::new(),
            frame_stage_px: 0.0,
            lost_samples: 0,
            planner_of_drawn_cut: None,
            hole_frames: 0,
            first_failure: None,
            buildable_cache: HashMap::new(),
            soft_checks: false,
            last_plan: None,
            last_stage: None,
            totals: Totals::default(),
            cell_ctx: None,
            cell_signature: None,
            cell_memory: HashMap::new(),
            cell_seen: HashMap::new(),
            cell_counters: CellCounters::default(),
            gate: StepGate::default(),
            cell_gate_prev: HashMap::new(),
            cell_nodes: Vec::new(),
            lag: LagLog::default(),
        }
    }

    /// The camera moved: the zone memory of the old view is dropped. The ladder restarts by
    /// itself only after a jump (R3-Q22), so `stage` is left alone.
    fn camera_moved(&mut self) {
        self.dirty = true;
        self.forget_zones();
        self.last_stage = None;
        self.retained_run = 0;
    }

    /// Drops the zone memory only: what was drawn before no longer bounds what is drawn now.
    fn forget_zones(&mut self) {
        self.cell_memory.clear();
        self.cell_signature = None;
    }

    /// Whether a counted stage explains a drop below the raw wish for an octant of `node`: the
    /// octant is staged below its wish and its node was capped by the fit this frame (R3-Q11)
    /// or the ladder runs coarser than the target (R3-Q3 stage pixels).
    fn stage_excuses(&self, staged: bool, node: NodeId) -> bool {
        staged && (self.frame_capped.contains(&node) || self.frame_stage_px > self.target_px)
    }

    /// A failed check: panics, or is counted when `soft_checks` is set.
    fn fail(&mut self, message: String) {
        assert!(self.soft_checks, "{message}");
        self.first_failure.get_or_insert(message);
    }

    /// The oracle on the cut a frame drew (holes, coarsenings, lost samples).
    fn check_drawn_cut(&mut self, number: usize, plan: &FramePlan, digest: u64) {
        self.gate.hops += plan.counters.step_hops as usize;
        let gpu = {
            let grid = self.dataset.cell_grid();
            let hierarchy = self.dataset.metadata.hierarchy();
            gpu_input_digest(&plan.entries, &plan.active, |id| {
                let bounds = hierarchy.node(id).expect("cut node is resident").bounds();
                representative_depth(&grid, bounds).saturating_sub(3)
            })
        };
        self.totals.gpu_digest = (self.totals.gpu_digest ^ gpu).wrapping_mul(0x0000_0100_0000_01b3);
        self.totals.gpu_digest_last = gpu;
        self.check_node_presence(number, plan);
        self.check_active_gaps(number, plan);
        self.check_run_diff(number, plan);
        self.check_cell_cut(number, plan, digest);
    }

    /// Presence invariant of the node anchor (plan task 10, A4): a node has an anchor exactly
    /// when some octant is drawn, and an octant is drawn exactly when it is not fully masked and
    /// the node has an anchor.
    fn check_node_presence(&self, number: usize, plan: &FramePlan) {
        let pool = self.driver.pool();
        for entry in &plan.entries {
            let depths = |keep: &dyn Fn(viewer_render_wgpu::LevelKey) -> bool| -> Vec<u8> {
                (0..=40)
                    .filter(|depth| keep(pool.key(entry.node, *depth)))
                    .collect()
            };
            let (usable, resident) = (
                depths(&|key| pool.is_usable(key)),
                depths(&|key| pool.is_resident(key)),
            );
            let anchored = entry.node_anchor.is_some();
            let drawn_any = entry.drawn.iter().any(Option::is_some);
            assert_eq!(
                anchored, drawn_any,
                "A4 at frame {number}: node {:?} anchor {:?} drawn {:?} mask {:x?} range {:?} \\
                 wanted {:?} usable {usable:?} resident {resident:?}",
                entry.node, entry.node_anchor, entry.drawn, entry.mask, entry.range, entry.wanted
            );
            for octant in 0..8 {
                assert_eq!(
                    entry.drawn[octant].is_some(),
                    !entry.octant_hidden(octant) && anchored,
                    "presence at frame {number}: node {:?} octant {octant} anchor {:?} drawn {:?} \
                     mask {:x?} wanted {:?}",
                    entry.node,
                    entry.node_anchor,
                    entry.drawn,
                    entry.mask,
                    entry.wanted
                );
            }
        }
    }

    /// Measurement, no assertion: for every node of the applied cut, compares the depths the GPU
    /// gets today (the active levels) with the run of `node_run` (`CellCutEntry::node_run`) plus
    /// the D5 depth the emission adds (`new_d5`), and classifies each difference. The classes are
    /// decided on the sets without the D5 depth.
    fn check_run_diff(&mut self, number: usize, plan: &FramePlan) {
        let pool = self.driver.pool();
        let hierarchy = self.dataset.metadata.hierarchy();
        let grid = self.dataset.cell_grid();
        let mut diff = std::mem::take(&mut self.totals.run_diff);
        for entry in &plan.entries {
            let (Some((lo, hi)), Some(run), Some(node)) =
                (entry.range, entry.node_run, hierarchy.node(entry.node))
            else {
                continue;
            };
            let base = representative_depth(&grid, node.bounds()).saturating_sub(3);
            let d5 = entry
                .mask
                .iter()
                .any(|word| *word != 0)
                .then(|| {
                    (base + 3..=base + MAX_NODE_SPAN)
                        .find(|depth| pool.is_resident(pool.key(entry.node, *depth)))
                })
                .flatten()
                .filter(|depth| pool.is_usable(pool.key(entry.node, *depth)));
            let mut today: Vec<u8> = plan
                .active
                .iter()
                .filter(|level| level.key.node == entry.node)
                .map(|level| level.key.depth)
                .collect();
            today.sort_unstable();
            let (Some(&top), Some(&bottom)) = (today.last(), today.first()) else {
                continue;
            };
            diff.compared += 1;
            let mut ours: Vec<u8> = (run.0..=run.1).collect();
            if let Some(d5) = d5
                && !ours.contains(&d5)
            {
                ours.push(d5);
                ours.sort_unstable();
            }
            if today == ours {
                continue;
            }
            // Without the D5 depth, which is a part apart: the run against its own set.
            let today: Vec<u8> = today
                .into_iter()
                .filter(|depth| Some(*depth) != d5)
                .collect();
            let wanted_set: Vec<u8> = (run.0..=run.1).collect();
            let (top, bottom) = (
                today.last().copied().unwrap_or(top),
                today.first().copied().unwrap_or(bottom),
            );
            let prev = diff.prev.get(&entry.node).copied();
            let usable: Vec<u8> = (0..=40)
                .filter(|depth| pool.is_usable(pool.key(entry.node, *depth)))
                .collect();
            let class = if today.windows(2).any(|pair| pair[1] != pair[0] + 1) {
                0
            } else if prev.is_none() {
                1
            } else if prev.is_some_and(|(_, fine)| !usable.contains(&fine)) {
                2
            } else if top > hi
                && today.iter().filter(|depth| **depth > hi).count()
                    != wanted_set.iter().filter(|depth| **depth > hi).count()
            {
                3
            } else if entry.node_anchor.is_some_and(|anchor| anchor < lo)
                && run.0 < lo
                && today.iter().all(|depth| (run.0..=run.1).contains(depth))
            {
                // The anchor stays at `prev` below `lo`: the extra coarse depths refine in every
                // visible cell, so nothing of them is drawn. Decided on the anchor and the run's
                // coarse end, and only when today's set lies inside the run (plus D5, which is
                // already out of `today`): today's set may also hold a depth below `lo`.
                4
            } else if bottom == run.0 && top != run.1 {
                5
            } else {
                6
            };
            diff.classes[class] += 1;
            if run.1 < top {
                diff.coarser += 1;
            } else if run.1 > top {
                diff.finer += 1;
            }
            let example = || {
                format!(
                    "{} frame {number}: node {:?} today {today:?} node_run {run:?} range {:?} \
                     target {:?} anchor {:?} prev {prev:?} usable {usable:?} stage {} step {:?} \
                     D5 {d5:?}",
                    RUN_CLASSES[class],
                    entry.node,
                    entry.range,
                    entry.node_target,
                    entry.node_anchor,
                    entry.node_stage,
                    entry.node_step
                )
            };
            if diff.examples[class].is_none() {
                diff.examples[class] = Some(example());
            }
            if run.1 < top && diff.first_coarser.is_none() {
                diff.first_coarser = Some(example());
            }
        }
        diff.prev = plan
            .entries
            .iter()
            .filter_map(|entry| Some((entry.node, pool.applied_run_of(entry.node)?)))
            .collect();
        self.totals.run_diff = diff;
    }

    /// Cell-mode oracle: a 7^3 lattice over the bounds of every node of the applied cut, asked of
    /// every node containing each sample through the twin of the GPU decision (`drawn_depth_at`
    /// over the node's active depths and mask). A sample is drawn by at most one node, by none
    /// only where no containing node wants it, and never ends coarser than
    /// `min(before, wanted)` (R3-Q21) unless a counted stage explains it.
    fn check_cell_cut(&mut self, number: usize, plan: &FramePlan, digest: u64) {
        let Some(ctx) = self.cell_ctx.take() else {
            return;
        };
        let signature = {
            let mut hasher = std::hash::DefaultHasher::new();
            std::hash::Hash::hash(&digest, &mut hasher);
            for level in &plan.active {
                std::hash::Hash::hash(&(level.key.node.get(), level.key.depth), &mut hasher);
            }
            for axis in ctx.eye {
                std::hash::Hash::hash(&axis.to_bits(), &mut hasher);
            }
            std::hash::Hash::hash(&self.frame_stage_px.to_bits(), &mut hasher);
            std::hash::Hasher::finish(&hasher)
        };
        if self.cell_signature == Some(signature) {
            self.cell_ctx = Some(ctx);
            return;
        }
        self.cell_signature = Some(signature);
        let grid = self.dataset.cell_grid();
        let hierarchy = self.dataset.metadata.hierarchy();
        let to_eye = |p: [f64; 3]| -> [f64; 3] {
            std::array::from_fn(|a| (p[a] - ctx.origin[a]) * ctx.scale - ctx.eye[a])
        };
        let nodes: Vec<CellNode> = plan
            .entries
            .iter()
            .map(|entry| {
                let bounds = hierarchy
                    .node(entry.node)
                    .expect("cut node is resident")
                    .bounds();
                let node_depth = representative_depth(&grid, bounds).saturating_sub(3);
                let lo = to_eye(bounds.min());
                let active = plan
                    .active
                    .iter()
                    .filter(|level| level.key.node == entry.node)
                    .fold(0_u64, |bits, level| {
                        assert!(
                            level.key.depth >= node_depth,
                            "an active depth is in the span"
                        );
                        bits | 1 << (level.key.depth - node_depth)
                    });
                CellNode {
                    id: entry.node,
                    min: bounds.min(),
                    max: bounds.max(),
                    lod: NodeLod {
                        anchor: lo.map(|v| v as f32),
                        node_depth,
                        active,
                        mask: entry.mask,
                    },
                    lo,
                    side: ctx.root_side / (1_u64 << node_depth) as f64 * ctx.scale,
                }
            })
            .collect();
        // Samples: the ones remembered (nodes of the previous cut) and the lattice of this cut.
        let mut points: Vec<((NodeId, u16), [f64; 3])> = self
            .cell_seen
            .iter()
            .map(|(key, (p, _))| (*key, *p))
            .collect();
        let known: HashSet<(NodeId, u16)> = points.iter().map(|(key, _)| *key).collect();
        for node in &nodes {
            for n in 0..343_u16 {
                if known.contains(&(node.id, n)) {
                    continue;
                }
                let at = [n % 7, n / 7 % 7, n / 49];
                points.push((
                    (node.id, n),
                    std::array::from_fn(|axis| {
                        let t = 0.01 + 0.98 * (f64::from(at[axis]) + 0.37) / 6.37;
                        node.min[axis] + (node.max[axis] - node.min[axis]) * t
                    }),
                ));
            }
        }
        let planned: Vec<NodeId> = match &self.planner_of_drawn_cut {
            Some(ids) => ids.clone(),
            None => self
                .dataset
                .visible_plan
                .as_ref()
                .map_or_default(|plan| plan.node_ids().to_vec()),
        };
        let planner: Vec<SpatialBounds> = planned
            .iter()
            .filter(|id| self.driver.has_node(**id))
            .filter_map(|id| hierarchy.node(*id).map(SpatialNode::bounds))
            .collect();
        self.check_active_runs(number, plan, &nodes);
        let mut memory = HashMap::new();
        let mut seen = HashMap::new();
        let mut gate_prev = HashMap::new();
        let mut on_screen: HashSet<(NodeId, u16)> = HashSet::new();
        let mut within_node = 0;
        let mut node_changed = 0;
        let mut lost = 0;
        for (key, p) in points {
            let rel = to_eye(p);
            let mut drawers = Vec::new();
            let mut wanted_here = false;
            for (j, node) in nodes.iter().enumerate() {
                if !(0..3).all(|a| p[a] >= node.min[a] && p[a] <= node.max[a]) {
                    continue;
                }
                let cell: [usize; 3] = std::array::from_fn(|a| {
                    (((rel[a] - node.lo[a]) / (node.side / 8.0)).floor().max(0.0) as usize).min(7)
                });
                wanted_here |= plan.entries[j].wanted[octant_of_cell(cell)].is_some();
                if let Some(depth) = drawn_depth_at(&ctx.lod, &node.lod, rel, node.lo, node.side) {
                    drawers.push((j, depth, cell));
                }
            }
            self.cell_counters.samples += 1;
            let before = self.cell_memory.get(&key).map(|(_, depth)| *depth);
            match drawers.as_slice() {
                [] => {
                    if wanted_here {
                        self.hole_frames += 1;
                        self.fail(format!(
                            "frame {number}: sample {key:?} at {p:?} is drawn by no node \
                             (before {before:?})"
                        ));
                    } else if self.cell_seen.contains_key(&key)
                        && planner
                            .iter()
                            .any(|b| (0..3).all(|a| p[a] >= b.min()[a] && p[a] <= b.max()[a]))
                    {
                        lost += 1;
                    }
                    if let Some(old) = self.cell_memory.get(&key) {
                        memory.insert(key, *old);
                    }
                    if let Some(old) = self.cell_seen.get(&key) {
                        seen.insert(key, *old);
                    }
                }
                [(j, depth, cell)] => {
                    let node = &nodes[*j];
                    let octant_wanted = plan.entries[*j].wanted[octant_of_cell(*cell)];
                    if let Some(octant_wanted) = octant_wanted {
                        let wanted = cell_wanted_depth(
                            &ctx.lod,
                            &node.lod,
                            rel,
                            node.lo,
                            node.side,
                            octant_wanted,
                        );
                        self.cell_counters.finer_than_wanted += usize::from(*depth > wanted);
                        self.cell_counters.coarser_than_wanted += usize::from(*depth < wanted);
                        if let Some(before) = before
                            && *depth < before.min(wanted)
                        {
                            let staged = plan.entries[*j].stage[octant_of_cell(*cell)];
                            let excused = self.stage_excuses(staged, node.id);
                            // As in the octant oracle, whose wish of a staged octant is lowered
                            // to what it draws: only a drop of an unstaged octant is a zone
                            // coarsening, the staged ones are counted apart.
                            self.zone_coarsenings += usize::from(!staged);
                            self.staged_coarsenings += 1;
                            if excused {
                                self.stage_excused_drops += 1;
                            } else {
                                self.unexcused_drops += 1;
                                self.fail(format!(
                                    "frame {number}: sample {key:?} at {p:?} went from depth \
                                     {before} to {depth} (wanted {wanted}, node {:?})",
                                    node.id
                                ));
                            }
                        }
                    }
                    if let Some(&(was_node, was)) = self.cell_gate_prev.get(&key)
                        && was.abs_diff(*depth) > 1
                    {
                        if was_node == node.id {
                            within_node += 1;
                        } else {
                            node_changed += 1;
                        }
                    }
                    gate_prev.insert(key, (node.id, *depth));
                    if ctx.on_screen(rel) {
                        on_screen.insert(key);
                    }
                    memory.insert(key, (p, *depth));
                    seen.insert(key, (p, *depth));
                }
                several => {
                    self.fail(format!(
                        "frame {number}: sample {key:?} at {p:?} is drawn by {} nodes: {:?}",
                        several.len(),
                        several
                            .iter()
                            .map(|(j, depth, _)| (nodes[*j].id, *depth))
                            .collect::<Vec<_>>()
                    ));
                }
            }
        }
        if lost > 0 {
            self.lost_samples += lost;
            self.fail(format!("frame {number}: {lost} lost samples"));
        }
        // Only the samples of the nodes that are in the cut now stay: the next frame compares
        // against them (and against the ones its own cut adds).
        let current: HashSet<NodeId> = nodes.iter().map(|node| node.id).collect();
        self.gate.cell_node_changed += node_changed;
        self.gate.cell_within_node += within_node;
        gate_prev.retain(|(node, _), _| current.contains(node));
        // Spatial gate: adjacent lattice points (one step along an axis) of the same node lattice
        // that different nodes draw.
        for (&(lattice, n), &(drawer, depth)) in &gate_prev {
            let at = [n % 7, n / 7 % 7, n / 49];
            for (axis, stride) in [1_u16, 7, 49].into_iter().enumerate() {
                if at[axis] == 6 {
                    continue;
                }
                if let Some(&(other, other_depth)) = gate_prev.get(&(lattice, n + stride))
                    && other != drawer
                {
                    self.gate.cell_border_pairs += 1;
                    let over_one = usize::from(depth.abs_diff(other_depth) > 1);
                    self.gate.cell_border_over_one += over_one;
                    if on_screen.contains(&(lattice, n))
                        && on_screen.contains(&(lattice, n + stride))
                    {
                        self.gate.cell_border_visible_pairs += 1;
                        self.gate.cell_border_visible_over_one += over_one;
                    }
                }
            }
        }
        self.cell_gate_prev = gate_prev;
        memory.retain(|(node, _), _| current.contains(node));
        seen.retain(|(node, _), _| current.contains(node));
        self.cell_memory = memory;
        self.cell_seen = seen;
        self.cell_counters.evaluations += 1;
        self.cell_nodes = nodes;
        self.cell_ctx = Some(ctx);
    }

    /// ADR 0055 ("Decision"): the active depths of a node are a contiguous run of resident depths.
    /// Between the coarsest and the finest active depth of a node, every depth that is resident,
    /// usable and inside the node's span is active (`cell_active_depths`), except the D5 depth of a node with a mask, which the pool appends
    /// apart from the run (`d5_active`: the smallest resident depth from `node_depth + 3`), so it
    /// is left out of the run and never reported itself. Usable is the pool's own (`is_usable`:
    /// resident and not uploaded this frame).
    fn check_active_runs(&mut self, number: usize, plan: &FramePlan, nodes: &[CellNode]) {
        let pool = self.driver.pool();
        let mut problems = Vec::new();
        for node in nodes {
            let base = node.lod.node_depth;
            let resident = |depth: u8| pool.is_usable(pool.key(node.id, depth));
            let d5 = (node.lod.mask.iter().any(|word| *word != 0))
                .then(|| {
                    (base + 3..=base + MAX_NODE_SPAN)
                        .find(|depth| pool.is_resident(pool.key(node.id, *depth)))
                })
                .flatten();
            let run: Vec<u8> = (base..=base + MAX_NODE_SPAN)
                .filter(|depth| node.lod.active >> (depth - base) & 1 == 1 && Some(*depth) != d5)
                .collect();
            let (Some(&first), Some(&last)) = (run.first(), run.last()) else {
                continue;
            };
            let missing: Vec<u8> = (first..=last)
                .filter(|depth| !run.contains(depth) && Some(*depth) != d5 && resident(*depth))
                .collect();
            if !missing.is_empty() {
                let entry = plan.entries.iter().find(|entry| entry.node == node.id);
                let range = entry.and_then(|entry| entry.range);
                let drawn = entry.map(CellCutEntry::required_levels);
                problems.push(format!(
                    "frame {number}: node {:?} has resident usable depths {missing:?} \
                     missing inside its active run {run:?} (D5 {d5:?}, range {range:?}, drawn {drawn:?})",
                    node.id
                ));
            }
        }
        for problem in problems {
            self.fail(problem);
        }
    }

    /// ADR 0055: the active set of a node is one contiguous run. Between the coarsest and the
    /// finest active depth of a node (its D5 depth exempt, as in `check_active_runs`), a depth
    /// inside the node's buildable span that is not usable (not resident, or uploaded this frame)
    /// is a gap: the per-cell rule would jump over it. Counted per node and frame in
    /// `Totals::active_gaps`; the first case is kept in `first_gap`. Runs on every frame, also
    /// the ones `check_cell_cut` skips as identical, because residency moves without the plan.
    fn check_active_gaps(&mut self, number: usize, plan: &FramePlan) {
        let pool = self.driver.pool();
        let grid = self.dataset.cell_grid();
        let hierarchy = self.dataset.metadata.hierarchy();
        let mut found = Vec::new();
        let mut spans = Vec::new();
        for entry in &plan.entries {
            let Some(node) = hierarchy.node(entry.node) else {
                continue;
            };
            let base = representative_depth(&grid, node.bounds()).saturating_sub(3);
            let d5 = entry
                .mask
                .iter()
                .any(|word| *word != 0)
                .then(|| {
                    (base + 3..=base + MAX_NODE_SPAN)
                        .find(|depth| pool.is_resident(pool.key(entry.node, *depth)))
                })
                .flatten();
            let run: Vec<u8> = plan
                .active
                .iter()
                .filter(|level| level.key.node == entry.node && Some(level.key.depth) != d5)
                .map(|level| level.key.depth)
                .collect();
            let (Some(&first), Some(&last)) = (run.iter().min(), run.iter().max()) else {
                continue;
            };
            if last - first < 2 {
                continue;
            }
            let Some(span) = self
                .buildable_cache
                .get(&entry.node)
                .copied()
                .or_else(|| buildable_span(self, entry.node))
            else {
                continue;
            };
            spans.push((entry.node, span));
            let missing = active_gaps_of(&run, d5, span, |depth| {
                pool.is_usable(pool.key(entry.node, depth))
            });
            if !missing.is_empty() {
                let state = |depth: u8| {
                    let key = pool.key(entry.node, depth);
                    (depth, pool.is_resident(key), pool.is_usable(key))
                };
                found.push(format!(
                    "frame {number}: node {:?} active {run:?} skips non-usable \
                     buildable depths {missing:?} (depth, resident, usable: {:?}; buildable \
                     {span:?}, D5 {d5:?}, range {:?}, drawn {:?}, wanted {:?})",
                    entry.node,
                    missing.iter().map(|d| state(*d)).collect::<Vec<_>>(),
                    entry.range,
                    entry.required_levels(),
                    entry.wanted
                ));
            }
        }
        self.buildable_cache.extend(spans);
        self.totals.active_gaps += found.len();
        if self.totals.first_gap.is_none() {
            self.totals.first_gap = found.into_iter().next();
        }
    }

    /// Cell mode: the frame of reference of the twin for the frame about to be checked.
    pub(super) fn set_cell_ctx(&mut self, view: &ViewProjection, stage_pixels: f64) {
        let grid = self.dataset.cell_grid();
        let viewport = view.viewport();
        let scale = view.visual_scale();
        self.cell_ctx = Some(CellCtx {
            lod: LodView::from_camera(
                &view.camera(),
                viewport.map(|v| v as f32),
                stage_pixels as f32,
                (self.driver.pool().min_side().unwrap_or(0.0) * scale) as f32,
            ),
            eye: view.camera().position(),
            scale,
            origin: self.dataset.coordinate_origin(),
            root_side: grid.root_side,
            viewport: viewport.map(f64::from),
            clip: view.camera().clip_distances(),
            near: lod_near_depth_scaled(&view.camera()),
        });
    }

    fn frame(&mut self, view: &ViewProjection) -> FrameInfo {
        self.target.eye = view.camera().position();
        let step = CellStep {
            view,
            target_pixels: self.target_px,
            config: self.config,
            can_load_blocks: true,
        };
        let number = self.totals.frames;
        let out = update_cells_staged(
            &mut self.dataset,
            &step,
            &mut self.stage,
            &mut self.dirty,
            &mut self.driver,
            &mut self.target,
            None,
            &mut self.blocks,
        )
        .unwrap_or_else(|error| panic!("frame {number}: {error}"));
        let stats = &out.update.stats;
        let plan = &out.update.report.plan;
        self.last_entries = plan
            .entries
            .iter()
            .map(|e| (e.node, e.wanted, e.drawn))
            .collect();
        self.last_levels = plan
            .entries
            .iter()
            .map(|e| {
                let depths = plan
                    .active
                    .iter()
                    .filter(|level| level.key.node == e.node)
                    .map(|level| level.key.depth)
                    .collect();
                (e.node, depths, e.range)
            })
            .collect();
        self.target.check_plan(plan);
        let digest = cell_cut_digest(&plan.entries);
        self.frame_capped = plan.coarsened_nodes.clone();
        self.frame_stage_px = out.stage_pixels;
        // The driver draws with the final target; the ladder rung only picks the nodes.
        self.set_cell_ctx(view, self.target_px);
        self.last_plan = Some(plan.clone());
        self.check_drawn_cut(number, plan, digest);
        if let Some(last) = self.last_stage
            && out.stage_pixels > last
        {
            self.totals.stage_drops += 1;
        }
        self.last_stage = Some(out.stage_pixels);
        if self.totals.stages.last() != Some(&out.stage_pixels) {
            self.totals.stages.push(out.stage_pixels);
        }
        let info = FrameInfo {
            apply: stats.apply,
            stage: out.stage_pixels,
            uploaded_pages: out.update.report.uploaded_pages,
            built: stats.built_nodes.len(),
            deferred: stats.build_deferred,
            nodes_deferred: stats.nodes_deferred,
            max_gap: stats.max_gap,
            dtf: stats.descendant_too_fine,
            digest,
            pending_blocks: stats.pending_block_ids.len(),
            cut_nodes: stats.cut_nodes,
            build_ms: stats.build_ms,
            cpu_cell_bytes: stats.cpu_cell_bytes,
            resident_pages: stats.counters.resident_pages,
            fallback_pages: stats.counters.fallback_pages,
            held_nodes: stats.counters.held_nodes,
            excluded_nodes: stats.counters.excluded_nodes,
            cpu_coarsened: stats.cpu_coarsened,
            gpu_coarsened: stats.gpu_coarsened,
            pool_overflow: stats.counters.pool_overflow,
            pool_coarsened: stats.counters.pool_coarsened,
        };
        {
            let c = &stats.counters;
            let t = &mut self.totals;
            t.hold_age_max = t.hold_age_max.max(c.hold_age_max);
            t.held_peak = t.held_peak.max(c.held_nodes);
            t.excluded_peak = t.excluded_peak.max(c.excluded_nodes);
            t.fallback_peak = t.fallback_peak.max(c.fallback_pages);
            t.resident_peak = t.resident_peak.max(c.resident_pages);
            t.hold_unsettled += u64::from(c.hold_unsettled);
            t.wait_frames += usize::from(c.held_nodes + c.excluded_nodes > 0);
            if c.hold_age_max > MAX_HOLD_FRAMES {
                self.fail(format!("frame {number}: hold_age_max {}", c.hold_age_max));
            }
        }
        let anchors = stats.pending_page_anchors.clone();
        for id in &stats.built_nodes {
            *self.totals.builds.entry(*id).or_default() += 1;
        }
        self.totals.forgotten += out.update.report.forgotten.len();
        self.totals.frames += 1;
        self.totals.uploads += info.uploaded_pages;
        self.totals.max_gap = self.totals.max_gap.max(stats.max_gap);
        self.totals.gaps_over_one += stats.gaps_over_one;
        self.totals.dtf += usize::from(stats.descendant_too_fine);
        if info.apply == CutApply::RetainedPrevious {
            self.retained_run += 1;
            self.totals.longest_retained = self.totals.longest_retained.max(self.retained_run);
        } else {
            self.retained_run = 0;
        }
        self.totals.drawn_digest = digest;
        for anchor in anchors {
            let page = (self.pages)(anchor).unwrap_or_else(|e| panic!("page {anchor:?}: {e}"));
            match self.dataset.graft_page(page) {
                Ok(()) => self.totals.grafted += 1,
                Err(LodDatasetError::PageBudget { .. }) => self.totals.graft_refused += 1,
                Err(error) => panic!("frame {number}: graft of {anchor:?}: {error}"),
            }
        }
        info
    }

    /// Frames until the ladder is at the target, nothing is pending and the pool applied the cut
    /// without uploads for `quiet` frames in a row. Returns the last frame.
    fn settle(&mut self, view: &ViewProjection, quiet: usize, max_frames: usize) -> FrameInfo {
        let clock = Instant::now();
        let mut calm = 0;
        let mut last = String::new();
        for _ in 0..max_frames {
            let info = self.frame(view);
            last = format!(
                "apply {:?} built {} deferred {} pending_blocks {} uploaded_pages {} cut_nodes {}",
                info.apply,
                info.built,
                info.deferred,
                info.pending_blocks,
                info.uploaded_pages,
                info.cut_nodes
            );
            let settled = !self.dirty
                && self.stage == Some(self.target_px)
                && info.apply == CutApply::Applied
                && info.uploaded_pages == 0
                && info.pending_blocks == 0
                && info.built == 0
                && info.deferred == 0
                && !self.driver.needs_frame();
            calm = if settled { calm + 1 } else { 0 };
            if calm >= quiet {
                return info;
            }
            assert!(
                clock.elapsed().as_secs() < 900,
                "did not settle within 900 s"
            );
        }
        panic!(
            "did not settle in {max_frames} frames: stage {:?} dirty {} last frame [{last}] totals {:?}",
            self.stage, self.dirty, self.totals
        );
    }

    fn report(&self, label: &str) {
        let t = &self.totals;
        println!(
            "{label}: frames {} | longest RetainedPrevious streak {} | max gap {} (gaps>1: {}) | \
             DescendantTooFine {} | uploads {} | grafted {} refused {} | stage drops {} | \
             digest {:016x} | pages used {} | zone coarsenings {} (staged, raw wanted: {}) | lost samples {} | hold age max {} (waiting frames {}, held peak {}, \
             excluded peak {}, fallback pages peak {}, unsettled {})",
            t.frames,
            t.longest_retained,
            t.max_gap,
            t.gaps_over_one,
            t.dtf,
            t.uploads,
            t.grafted,
            t.graft_refused,
            t.stage_drops,
            t.drawn_digest,
            self.dataset
                .pages
                .as_ref()
                .map_or(0, super::super::lod_pages::PageResidency::pages),
            self.zone_coarsenings,
            self.staged_coarsenings,
            self.lost_samples,
            t.hold_age_max,
            t.wait_frames,
            t.held_peak,
            t.excluded_peak,
            t.fallback_peak,
            t.hold_unsettled,
        );
        println!("{label}: ADR 0053 gate {:?}", self.gate);
    }
}

// ---------------------------------------------------------------------------------------------
// Synthetic paged octree (always on)
// ---------------------------------------------------------------------------------------------

/// `octree_nodes()` (1, 2..=9, 10..=17 under 2) plus eight children (18..=25) of node 9, the
/// octant opposite to node 2.
fn grown_nodes() -> Vec<SpatialNode> {
    let mut nodes = octree_nodes();
    let parent = nodes
        .iter()
        .find(|n| n.id() == NodeId::new(9))
        .unwrap()
        .bounds();
    for index in 0..8_u64 {
        let side = SIDE / 4.0;
        let min: [f64; 3] =
            std::array::from_fn(|axis| parent.min()[axis] + side * ((index >> axis) & 1) as f64);
        nodes.push(
            SpatialNode::new(
                NodeId::new(18 + index),
                Some(NodeId::new(9)),
                2,
                SpatialBounds::new(min, min.map(|v| v + side)).unwrap(),
                200,
                0.0,
                vec![BlockId::new(100 + 18 + index)],
            )
            .unwrap(),
        );
    }
    nodes
}

/// Root + octants resident; nodes 2 and 3 are anchors whose pages (children) arrive from
/// `PageInsert`s of 100 bytes each, and the budget holds one page at a time.
struct PagedSynthetic {
    dataset: LodDataset,
    blocks: HashMap<BlockId, PointBlock>,
    pages: HashMap<NodeId, (u32, Vec<SpatialNode>)>,
}

fn paged_synthetic(budget: usize, cache_bytes: usize) -> PagedSynthetic {
    let all = grown_nodes();
    let full = octree_dataset(all.clone(), HierarchySemantics::Replacement, true);
    let blocks: HashMap<BlockId, PointBlock> = all
        .iter()
        .map(|n| (n.blocks()[0], node_block(&full, n.id().get())))
        .collect();
    let resident: Vec<SpatialNode> = all.iter().filter(|n| n.id().get() <= 9).cloned().collect();
    let total = all.iter().map(SpatialNode::point_count).sum();
    let hierarchy = SpatialHierarchy::new(resident, Vec::new(), Some(total)).unwrap();
    let dataset = LodDataset::new([0.0; 3], hierarchy, cache_bytes)
        .unwrap()
        .with_global_frame(true)
        .with_pages(&[NodeId::new(2), NodeId::new(9)], Box::new(|_| {}), budget)
        .unwrap();
    let page = |number: u32, range: std::ops::Range<u64>| {
        let nodes = all
            .iter()
            .filter(|n| range.contains(&n.id().get()))
            .cloned()
            .collect();
        (number, nodes)
    };
    let pages = HashMap::from([
        (NodeId::new(2), page(1, 10..18)),
        (NodeId::new(9), page(2, 18..26)),
    ]);
    PagedSynthetic {
        dataset,
        blocks,
        pages,
    }
}

fn synthetic_harness(budget: usize, target_px: f64) -> Harness {
    synthetic_harness_with_cache(budget, target_px, 64 << 20)
}

/// [`synthetic_harness`] with a decoded-block cache of `cache_bytes`.
fn synthetic_harness_with_cache(budget: usize, target_px: f64, cache_bytes: usize) -> Harness {
    synthetic_harness_spaced(budget, target_px, cache_bytes, None)
}

/// [`synthetic_harness_with_cache`] whose dataset carries a stored reference spacing.
fn synthetic_harness_spaced(
    budget: usize,
    target_px: f64,
    cache_bytes: usize,
    reference_spacing: Option<f64>,
) -> Harness {
    let PagedSynthetic {
        mut dataset,
        blocks,
        pages,
    } = paged_synthetic(budget, cache_bytes);
    if let Some(spacing) = reference_spacing {
        dataset = dataset.with_reference_spacing(spacing);
    }
    Harness::new(
        dataset,
        target_px,
        Box::new(move |id| Ok(blocks.get(&id).cloned())),
        Box::new(move |anchor| {
            let (number, nodes) = pages.get(&anchor).ok_or("no such page")?;
            Ok(PageInsert {
                anchor,
                number: *number,
                nodes: nodes.clone(),
                unloaded: Vec::new(),
                bytes: 100,
            })
        }),
    )
}

#[test]
fn synthetic_ladder_streams_parents_first_without_holes() {
    let mut h = synthetic_harness(1 << 20, 2.0);
    let view = synthetic_views().0;
    for _ in 0..400 {
        h.frame(&view);
        if !h.dirty && h.stage == Some(2.0) && !h.driver.needs_frame() {
            break;
        }
    }
    h.report("synthetic streaming");
    // Coarse to fine: 16 -> 8 -> 4 -> 2, never back up.
    assert_eq!(h.totals.stages, vec![16.0, 8.0, 4.0, 2.0]);
    assert_eq!(h.totals.stage_drops, 0);
    assert!(h.totals.longest_retained <= MAX_RETAINED_STREAK);
    assert!(h.totals.max_gap <= MAX_CELL_CUT_GAP);
    assert_eq!(h.totals.dtf, 0);
    assert!(h.target.uploads_total > 0);
}

#[test]
fn the_oracle_reports_a_hidden_cell_whose_refinement_does_not_draw() {
    let mut h = synthetic_harness(1 << 20, 2.0);
    let view = synthetic_views().0;
    // Along the ladder some cut has a node hidden under a refinement.
    let (mut cut, hider) = (0..400)
        .find_map(|_| {
            h.frame(&view);
            let cut = CutView::new(&h.dataset, h.last_plan.as_ref()?);
            let hider = (0..cut.nodes.len())
                .find(|i| !cut.descendants[*i].is_empty() && cut.entries[*i].mask != [0; 8]);
            hider.map(|hider| (cut, hider))
        })
        .expect("a cut with a node hidden under a refinement");
    assert!(cut.holes().is_empty());
    // The ancestor wants an octant its mask hides under the refinement (a pool that kept the
    // octant while a child covers it), and the refinement stops drawing: the hidden cells are
    // uncovered.
    let hidden_octant = (0..8)
        .find(|o| {
            (0..512).any(|n| {
                let c = [n % 8, n / 8 % 8, n / 64];
                octant_of_cell(c) == *o && cell_hidden(&cut.entries[hider].mask, c)
            })
        })
        .expect("the mask hides a cell");
    cut.entries[hider].wanted[hidden_octant] = Some(0);
    for j in cut.descendants[hider].clone() {
        cut.active[j] = [None; 8];
    }
    assert!(
        cut.holes().iter().any(|hole| hole.contains("hidden true")),
        "a hidden cell under a refinement that draws nothing is a hole"
    );
}

#[test]
fn the_cell_oracle_reports_a_node_whose_active_depths_are_missing() {
    let mut h = synthetic_harness(1 << 20, 2.0);
    let view = synthetic_views().0;
    h.settle(&view, 3, 400);
    let plan = h.last_plan.clone().expect("a frame ran");
    let digest = cell_cut_digest(&plan.entries);
    // The state the harness checked passes when it is checked again.
    h.cell_signature = None;
    h.check_cell_cut(0, &plan, digest);
    assert!(h.cell_ctx.is_some(), "the check ran to its end");
    // A node that wants its octants, alone in its footprint, loses every active depth: the
    // samples inside it are drawn by no node.
    let lone = (0..h.cell_nodes.len())
        .find(|&j| {
            let a = &h.cell_nodes[j];
            plan.entries[j].wanted.iter().any(Option::is_some)
                && plan.active.iter().any(|level| level.key.node == a.id)
                && h.cell_nodes.iter().enumerate().all(|(k, b)| {
                    k == j
                        || (0..3)
                            .any(|axis| a.max[axis] <= b.min[axis] || b.max[axis] <= a.min[axis])
                })
        })
        .expect("a node of the cut that overlaps no other");
    let mut broken = plan.clone();
    let lost = h.cell_nodes[lone].id;
    broken.active.retain(|level| level.key.node != lost);
    h.cell_signature = None;
    let reported = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        h.check_cell_cut(0, &broken, digest);
    }))
    .expect_err("the oracle reports the missing active depths");
    let message = reported
        .downcast_ref::<String>()
        .cloned()
        .unwrap_or_default();
    assert!(message.contains("is drawn by no node"), "{message}");
}

/// One depth of a multi-depth node's active run goes missing while the node keeps the others:
/// the samples that depth drew end coarser than before (R3-Q21) or lose their drawer.
fn a_missing_depth_of_a_run_is_reported(pick: impl Fn(&[u8]) -> u8) {
    let mut h = synthetic_harness(1 << 20, 2.0);
    let view = synthetic_views().0;
    h.settle(&view, 3, 400);
    let plan = h.last_plan.clone().expect("a frame ran");
    let digest = cell_cut_digest(&plan.entries);
    h.cell_signature = None;
    h.check_cell_cut(0, &plan, digest);
    // The node with the longest active run, which has a depth strictly between its ends.
    let runs: Vec<(NodeId, Vec<u8>)> = plan
        .entries
        .iter()
        .map(|entry| {
            let mut depths: Vec<u8> = plan
                .active
                .iter()
                .filter(|level| level.key.node == entry.node)
                .map(|level| level.key.depth)
                .collect();
            depths.sort_unstable();
            (entry.node, depths)
        })
        .collect();
    let (node, depths) = runs
        .into_iter()
        .filter(|(_, depths)| depths.len() >= 3)
        .max_by_key(|(_, depths)| depths.len())
        .expect("a node of the cut with three active depths");
    let missing = pick(&depths);
    let mut broken = plan.clone();
    broken
        .active
        .retain(|level| !(level.key.node == node && level.key.depth == missing));
    assert!(broken.active.len() < plan.active.len());
    h.cell_signature = None;
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        h.check_cell_cut(0, &broken, digest);
    }))
    .expect_err("the oracle reports the missing depth");
}

/// A node whose active set is {11, 18} with 12..=17 buildable but not resident has a gap the
/// oracle must report; a usable middle depth, an unbuildable one and the D5 depth are not gaps.
#[test]
fn the_oracle_reports_non_resident_gaps_in_a_nodes_active_set() {
    let resident = [11_u8, 18];
    let usable = |depth: u8| resident.contains(&depth);
    assert_eq!(
        active_gaps_of(&[11, 18], None, (9, 19), usable),
        [12, 13, 14, 15, 16, 17]
    );
    assert_eq!(active_gaps_of(&[11, 18], None, (9, 19), |_| true), []);
    assert_eq!(active_gaps_of(&[11, 18], None, (11, 12), usable), [12]);
    assert_eq!(
        active_gaps_of(&[11, 12, 13], Some(14), (9, 19), usable),
        [] as [u8; 0]
    );
    assert_eq!(
        active_gaps_of(&[11, 17], Some(14), (9, 19), usable),
        [12, 13, 15, 16]
    );
    assert_eq!(active_gaps_of(&[], None, (9, 19), usable), []);
}

// Observed on the synthetic ladder (node 14, active depths 9..=19, depth 14 removed while 31 lattice
// samples drew it): the per-cell twin falls back on a neighbouring depth that is not coarser than
// `min(before, wanted)`, so only the contiguity check of `check_active_runs` reports it.
#[test]
fn the_cell_oracle_reports_a_middle_depth_missing_from_a_run() {
    a_missing_depth_of_a_run_is_reported(|depths| depths[depths.len() / 2]);
}

#[test]
fn the_cell_oracle_reports_the_finest_depth_missing_from_a_run() {
    a_missing_depth_of_a_run_is_reported(|depths| *depths.last().unwrap());
}

#[test]
fn synthetic_build_cap_defers_without_waiting_for_the_camera() {
    let view = synthetic_views().0;
    let mut free = synthetic_harness(1 << 20, 2.0);
    free.settle(&view, 3, 400);
    let expected = free.totals.drawn_digest;
    // Same dataset with every block already decoded but no cells built (a fresh driver and
    // target), so one update has several nodes to build and the cap defers the rest.
    let mut capped = free;
    capped.driver = driver_for(&capped.dataset, u64::MAX);
    capped.target = StrictTarget::default();
    capped.dataset.pool_reset();
    capped.dataset.driver_replaced();
    capped.camera_moved();
    // The new pool draws nothing yet: the old image is not a baseline for lost samples.
    capped.cell_seen.clear();
    capped.totals = Totals::default();
    // The ladder is already at the target rung: every selected node waits for the cap.
    capped.stage = Some(2.0);
    capped.config.max_node_builds = 1;
    let mut deferred_frames = 0;
    let mut builds = 0;
    // Frames run only while the ladder asks for them, as the viewer loop does.
    for _ in 0..800 {
        if !capped.dirty && !capped.driver.needs_frame() {
            break;
        }
        let info = capped.frame(&view);
        // `deferred` also counts the level backlog; only node deferral is compared with builds.
        deferred_frames += usize::from(info.nodes_deferred > 0);
        builds += info.built;
    }
    capped.report("synthetic capped builds");
    assert!(deferred_frames > 0, "the cap must defer some builds");
    assert!(builds > deferred_frames, "deferred nodes were built later");
    assert_eq!(
        capped.totals.drawn_digest, expected,
        "the capped run ends on the cut of the uncapped one with the camera still"
    );
}

/// A stands inside node 2 looking out of the cube and B inside node 9 looking out the other
/// way, so each sees only the anchor of its own page.
fn synthetic_views() -> (ViewProjection, ViewProjection) {
    let look = |eye: [f64; 3], target: [f64; 3]| {
        let mut camera = OrbitCamera::default();
        assert!(camera.fit(
            SceneBounds {
                min: [0.0; 3],
                max: [SIDE; 3],
            },
            [800, 600],
        ));
        assert!(camera.set_pose(eye, target));
        ViewProjection::new(camera, [800, 600], 1.0, VoxelBuildLimits::default())
    };
    (look([16.0; 3], [-20.0; 3]), look([48.0; 3], [84.0; 3]))
}

#[test]
fn synthetic_regraft_restores_the_cut_without_holes() {
    // One page of 100 bytes fits the 150-byte budget.
    let mut h = synthetic_harness(150, 2.0);
    let (a, b) = synthetic_views();
    h.settle(&a, 3, 600);
    let before = h.totals.drawn_digest;
    assert!(
        h.dataset
            .metadata
            .hierarchy()
            .node(NodeId::new(10))
            .is_some()
    );
    let used_a = h.dataset.pages.as_ref().unwrap().used_bytes();
    assert_eq!(used_a, 100, "only the page of anchor 2 is resident");

    h.camera_moved();
    h.settle(&b, 3, 900);
    assert!(
        h.dataset
            .metadata
            .hierarchy()
            .node(NodeId::new(10))
            .is_none(),
        "the page of A was evicted"
    );
    assert!(
        h.dataset
            .metadata
            .hierarchy()
            .node(NodeId::new(18))
            .is_some()
    );
    // B needs the page of anchor 9 and the budget holds one page: A's page left by eviction.
    assert!(h.dataset.pages.as_ref().unwrap().evictions() > 0);
    println!(
        "synthetic regraft: {} grafts refused",
        h.totals.graft_refused
    );

    h.camera_moved();
    h.settle(&a, 3, 900);
    h.report("synthetic regraft");
    assert!(
        h.dataset
            .metadata
            .hierarchy()
            .node(NodeId::new(10))
            .is_some()
    );
    assert_eq!(
        h.totals.drawn_digest, before,
        "the cut at A is the one from before"
    );
    assert_eq!(h.totals.dtf, 0);
}

#[test]
fn synthetic_page_of_a_drawn_node_is_refused() {
    let mut h = synthetic_harness(150, 2.0);
    let (a, _) = synthetic_views();
    h.settle(&a, 3, 600);
    let drawn = h.dataset.cells.pool_drawn().to_vec();
    assert!(
        drawn.contains(&NodeId::new(10)),
        "the pool draws a node of page 1"
    );
    let error = h
        .dataset
        .graft_page((h.pages)(NodeId::new(9)).unwrap())
        .unwrap_err();
    assert!(
        matches!(error, LodDatasetError::PageBudget { .. }),
        "{error}"
    );
    assert!(
        h.dataset
            .metadata
            .hierarchy()
            .node(NodeId::new(10))
            .is_some()
    );
    assert!(
        h.dataset
            .metadata
            .hierarchy()
            .node(NodeId::new(18))
            .is_none()
    );
}

/// A straight zoom towards the synthetic cube: at rest at each distance, along the view axis the
/// depth a node draws never increases with the distance to the eye (nearer cells are finer or
/// equal). Along the axis the viewing angle of a cell is constant, which off-axis samples lack:
/// the projected edge of a cube depends on the angle too, so a farther off-axis point may draw
/// finer.
#[test]
fn cell_mode_bands_follow_the_distance() {
    let target = [SIDE / 2.0; 3];
    let direction = [0.6_f64, 0.5, 0.62];
    let norm = direction.iter().map(|v| v * v).sum::<f64>().sqrt();
    let mut h = synthetic_harness(1 << 20, 2.0);
    let mut banded_nodes = 0;
    for distance in [300.0, 150.0, 80.0, 40.0, 20.0, 10.0] {
        let eye: [f64; 3] = std::array::from_fn(|a| target[a] + direction[a] / norm * distance);
        let mut camera = OrbitCamera::default();
        assert!(camera.fit(
            SceneBounds {
                min: [0.0; 3],
                max: [SIDE; 3],
            },
            [800, 600],
        ));
        assert!(camera.set_pose(eye, target));
        let view = ViewProjection::new(camera, [800, 600], 1.0, VoxelBuildLimits::default());
        h.camera_moved();
        h.settle(&view, 3, 2_000);
        let ctx = h.cell_ctx.as_ref().expect("a cell-mode frame ran");
        // Points of the view axis from the eye through the target and beyond, dataset frame.
        let mut per_node: HashMap<NodeId, Vec<u8>> = HashMap::new();
        // The node the axis enters first, nearest the eye.
        let mut nearest_node: Option<NodeId> = None;
        for step in 0..=4_000 {
            let along = distance * (f64::from(step) / 2_000.0);
            let p: [f64; 3] = std::array::from_fn(|a| eye[a] - direction[a] / norm * along);
            let rel: [f64; 3] =
                std::array::from_fn(|a| (p[a] - ctx.origin[a]) * ctx.scale - ctx.eye[a]);
            for node in &h.cell_nodes {
                if !(0..3).all(|a| p[a] >= node.min[a] && p[a] <= node.max[a]) {
                    continue;
                }
                if let Some(depth) = drawn_depth_at(&ctx.lod, &node.lod, rel, node.lo, node.side) {
                    nearest_node.get_or_insert(node.id);
                    per_node.entry(node.id).or_default().push(depth);
                }
            }
        }
        let nearest = nearest_node.expect("the view axis crosses the cut");
        let depths = &per_node[&nearest];
        let distinct: HashSet<u8> = depths.iter().copied().collect();
        // Far away a node is one band; at the closest distance the first node entered is a
        // single deep band (observed: 33); in between it must show at least two depths.
        assert!(
            distance > 50.0 || distance < 15.0 || distinct.len() >= 2,
            "distance {distance}: the nearest node {nearest:?} draws one depth {depths:?}"
        );
        for (node, depths) in per_node {
            // `depths` follows the distance to the eye, nearest first.
            assert!(
                depths.windows(2).all(|pair| pair[1] <= pair[0]),
                "distance {distance}: node {node:?} draws {depths:?} along the view axis"
            );
            let distinct: HashSet<u8> = depths.iter().copied().collect();
            banded_nodes += usize::from(distinct.len() > 1);
        }
    }
    assert!(banded_nodes > 0, "the zoom never produced a band");
}

// ---------------------------------------------------------------------------------------------
// Real data (belvedere v7.5)
// ---------------------------------------------------------------------------------------------

type RealReader = FormatReader<std::io::BufReader<std::fs::File>>;

struct Real {
    harness: Harness,
    viewport: [u32; 2],
    scale: f64,
    origin: [f64; 3],
    /// The unloaded anchors of the root page.
    anchor_ids: Vec<NodeId>,
}

fn real_path(name: &str) -> Option<std::path::PathBuf> {
    let path = render_data_dir().join(name);
    if path.exists() {
        Some(path)
    } else {
        assert!(
            std::env::var("R3_REQUIRE_REAL_DATA").as_deref() != Ok("1"),
            "R3_REQUIRE_REAL_DATA=1 but {} is missing",
            path.display()
        );
        println!(
            "SKIP: {} is missing (real-data test not run)",
            path.display()
        );
        None
    }
}

#[path = "lod_cells_bench_tests.rs"]
mod bench;

#[path = "lod_cells_sim_tests.rs"]
mod sim;

fn open_real(name: &str, target_px: f64, page_budget: usize) -> Option<Real> {
    open_real_with_cache(name, target_px, page_budget, 2 << 30)
}

/// [`open_real`] with a decoded-block cache of `cache_bytes` (the browser uses 256 MiB).
fn open_real_with_cache(
    name: &str,
    target_px: f64,
    page_budget: usize,
    cache_bytes: usize,
) -> Option<Real> {
    let path = real_path(name)?;
    let file = std::fs::File::open(path).unwrap();
    let reader: RealReader =
        FormatReader::open_lazy(std::io::BufReader::new(file), FormatReaderLimits::default())
            .unwrap();
    assert!(reader.format_version().uses_global_frame());
    let source = reader.page_source().expect("paged").clone();
    let scale = reader.format_metadata().visual_scale();
    let dataset = LodDataset::from_shared_metadata_and_visual_scale(
        reader.format_metadata().viewer_dataset().unwrap(),
        scale,
        cache_bytes,
    )
    .with_global_frame(true)
    .with_pages(
        source.root_unloaded(),
        Box::new({
            let source = source.clone();
            move |anchor| source.release_page(anchor)
        }),
        page_budget,
    )
    .unwrap();
    assert!(dataset.supports_cells());
    let origin = dataset.coordinate_origin();
    let anchor_ids = source.root_unloaded().to_vec();
    let reader = Rc::new(RefCell::new(reader));
    let for_blocks = Rc::clone(&reader);
    let for_pages = Rc::clone(&reader);
    let blocks: Blocks = Box::new(move |id| {
        crate::block_loading::load_render_block(
            &mut *for_blocks.borrow_mut(),
            id,
            Some(viewer_render_wgpu::ColorMode::Rgb),
            None,
            None,
        )
        .map(Some)
    });
    let pages: Pages = Box::new(move |anchor| {
        let graft = for_pages
            .borrow_mut()
            .read_page(anchor)
            .map_err(|e| e.to_string())?;
        Ok(PageInsert {
            anchor,
            number: graft.number,
            nodes: graft.nodes,
            unloaded: graft.unloaded,
            bytes: graft.encoded_len,
        })
    });
    Some(Real {
        harness: Harness::new(dataset, target_px, blocks, pages),
        viewport: [800, 600],
        scale,
        origin,
        anchor_ids,
    })
}

impl Real {
    /// Centre and side (dataset frame) of every anchor of the root page, nearest to `corner`
    /// (fractions of the root cube) first.
    fn anchors_near(&self, corner: [f64; 3]) -> Vec<([f64; 3], f64)> {
        let hierarchy = self.harness.dataset.metadata.hierarchy();
        let root = hierarchy.stats().root_bounds;
        let side = (0..3)
            .map(|a| root.max()[a] - root.min()[a])
            .fold(0.0, f64::max);
        let want: [f64; 3] = std::array::from_fn(|a| root.min()[a] + side * corner[a]);
        let mut anchors: Vec<([f64; 3], f64)> = self
            .anchor_ids
            .iter()
            .filter_map(|id| hierarchy.node(*id))
            .map(|node| {
                let b = node.bounds();
                (
                    std::array::from_fn(|a| f64::midpoint(b.min()[a], b.max()[a])),
                    b.max()[0] - b.min()[0],
                )
            })
            .collect();
        let distance = |c: &[f64; 3]| (0..3).map(|a| (c[a] - want[a]).powi(2)).sum::<f64>();
        anchors.sort_by(|x, y| distance(&x.0).total_cmp(&distance(&y.0)));
        anchors
    }

    /// A camera at `centre + offset` (dataset frame) looking at `centre`.
    fn view(&self, centre: [f64; 3], offset: [f64; 3]) -> ViewProjection {
        let local = |p: [f64; 3]| -> [f64; 3] {
            std::array::from_fn(|a| (p[a] - self.origin[a]) * self.scale)
        };
        let eye = local(std::array::from_fn(|a| centre[a] + offset[a]));
        let target = local(centre);
        let mut camera = OrbitCamera::default();
        assert!(camera.fit(
            SceneBounds {
                min: local(centre.map(|v| v - 1.0)),
                max: local(centre.map(|v| v + 1.0)),
            },
            self.viewport,
        ));
        assert!(camera.set_pose(eye, target));
        ViewProjection::new(
            camera,
            self.viewport,
            self.scale,
            VoxelBuildLimits::default(),
        )
    }
}

#[test]
fn real_streaming_ladder_belvedere() {
    let Some(mut real) = open_real("belvedere-v8.oct", 4.0, 256 << 20) else {
        return;
    };
    let anchors = real.anchors_near([0.5, 0.5, 0.5]);
    println!(
        "belvedere: {} root anchors, scale {}, first {:?}",
        anchors.len(),
        real.scale,
        anchors[0]
    );
    let (centre, side) = anchors[0];
    let view = real.view(centre, [0.0, -side * 0.2, side * 0.12]);
    let clock = Instant::now();
    let last = real.harness.settle(&view, 3, 20_000);
    real.harness.report("belvedere streaming");
    println!(
        "belvedere streaming: {:.1} s, final cut {} nodes, stage {}",
        clock.elapsed().as_secs_f64(),
        last.cut_nodes,
        last.stage
    );
    let t = &real.harness.totals;
    assert!(t.longest_retained <= MAX_RETAINED_STREAK);
    assert!(t.max_gap <= MAX_CELL_CUT_GAP);
    assert_eq!(t.dtf, 0);
    assert_eq!(t.stage_drops, 0);
    // Coarse to fine through every rung; a straight jump to 4 px would show up here.
    assert_eq!(t.stages, vec![16.0, 8.0, 4.0]);
    assert_eq!(real.harness.zone_coarsenings, 0);
    assert_eq!(real.harness.unexcused_drops, 0);
    assert_eq!(real.harness.lost_samples, 0);
}

/// Settles belvedere at 1 px with `min_side` (dataset units, set as production does: the dataset
/// for the cut, the pool for the depths), after a dolly toward the data. Returns the number of
/// drawn octants not drawn by the root and the smallest cell side among them (dataset units).
fn drawn_cubes_after_dolly(min_side: Option<f64>) -> (usize, f64) {
    let mut real = open_real("belvedere-v8.oct", 1.0, 256 << 20).expect("real data required");
    real.harness.dataset.set_min_side(min_side);
    real.harness.driver.set_min_side(min_side);
    let (centre, side) = real.anchors_near([0.5, 0.5, 0.5])[0];
    for k in [4.0, 0.02] {
        let view = real.view(centre, [0.0, -side * 0.2 * k, side * 0.12 * k]);
        real.harness.camera_moved();
        real.harness.settle(&view, 3, 20_000);
    }
    let h = &real.harness;
    let (root, root_side) = (
        h.dataset.metadata.hierarchy().root_id(),
        h.dataset.cell_grid().root_side,
    );
    let sides: Vec<f64> = (h.last_entries.iter())
        .filter(|(node, ..)| *node != root)
        .flat_map(|(_, _, drawn)| drawn.iter().flatten())
        .map(|&depth| root_side / 2.0_f64.powi(i32::from(depth)))
        .collect();
    (sides.len(), sides.into_iter().fold(f64::INFINITY, f64::min))
}

/// #103 on real data: with a large minimum and a small px target no cell is drawn below the
/// minimum (the cut ignored it), and without a minimum the same poses do draw smaller cells.
#[test]
#[ignore = "real data (minutes): R3_REQUIRE_REAL_DATA=1 cargo test -p viewer_wasm --profile fast --offline --lib -- --ignored real_belvedere_cells_respect_the_minimum_side"]
fn real_belvedere_cells_respect_the_minimum_side() {
    // Belvedere's cut nodes reach sides/8 of 16 m at this pose: 40 m makes the minimum bind.
    const MIN_SIDE: f64 = 40.0;
    let (samples, smallest) = drawn_cubes_after_dolly(Some(MIN_SIDE));
    println!("with min {MIN_SIDE}: {samples} drawn octants, smallest side {smallest}");
    assert!(samples > 0);
    assert!(
        smallest >= MIN_SIDE * (1.0 - 1e-3),
        "a cell of side {smallest} is drawn below the minimum {MIN_SIDE}"
    );
    let (samples, smallest) = drawn_cubes_after_dolly(None);
    println!("without min: {samples} drawn octants, smallest side {smallest}");
    assert!(
        smallest < MIN_SIDE,
        "the pose must draw below {MIN_SIDE} without a minimum"
    );
}

/// Two far-apart regions of belvedere: the views look at the nearest and the farthest root
/// anchor from the middle of the cube.
fn region_views(real: &Real) -> (ViewProjection, ViewProjection) {
    let anchors = real.anchors_near([0.5, 0.5, 0.5]);
    let look = |(centre, side): ([f64; 3], f64)| real.view(centre, [0.0, -side * 0.2, side * 0.12]);
    (look(anchors[0]), look(*anchors.last().unwrap()))
}

/// Resident page budget for the regraft tests: a little above what region A needs (A needs 47 398 bytes in 8 pages;
/// see the printout of `real_regraft_belvedere`), so region B must evict A's pages.
const REGRAFT_PAGE_BUDGET: usize = 60_000;

#[test]
fn real_regraft_belvedere() {
    let Some(mut real) = open_real("belvedere-v8.oct", 4.0, REGRAFT_PAGE_BUDGET) else {
        return;
    };
    let (a, b) = region_views(&real);
    real.harness.settle(&a, 3, 20_000);
    let before = real.harness.totals.drawn_digest;
    let drawn_a: Vec<NodeId> = real.harness.dataset.cells.pool_drawn().to_vec();
    let used_a = real.harness.dataset.pages.as_ref().unwrap().used_bytes();
    println!(
        "belvedere regraft: A uses {used_a} of {REGRAFT_PAGE_BUDGET} page bytes, draws {} nodes",
        drawn_a.len()
    );

    real.harness.camera_moved();
    real.harness.settle(&b, 3, 20_000);
    let hierarchy = real.harness.dataset.metadata.hierarchy();
    let evicted: Vec<NodeId> = drawn_a
        .iter()
        .copied()
        .filter(|id| hierarchy.node(*id).is_none())
        .collect();
    println!(
        "belvedere regraft: B settled, {} of A's {} drawn nodes were evicted with their pages \
         ({} page evictions)",
        evicted.len(),
        drawn_a.len(),
        real.harness.dataset.pages.as_ref().unwrap().evictions()
    );
    assert!(
        !evicted.is_empty(),
        "region B must evict a page that held nodes of A's cut"
    );

    real.harness.camera_moved();
    let frames_back = real.harness.totals.frames;
    real.harness.settle(&a, 3, 20_000);
    real.harness.report("belvedere regraft");
    println!(
        "belvedere regraft: {} frames back at A, digest before {before:016x} after {:016x}",
        real.harness.totals.frames - frames_back,
        real.harness.totals.drawn_digest
    );
    let hierarchy = real.harness.dataset.metadata.hierarchy();
    assert!(
        evicted.iter().all(|id| hierarchy.node(*id).is_some()),
        "the evicted page was grafted again with the same node ids"
    );
    assert_eq!(real.harness.totals.drawn_digest, before);
    let t = &real.harness.totals;
    assert!(t.longest_retained <= MAX_RETAINED_STREAK);
    assert!(t.max_gap <= MAX_CELL_CUT_GAP);
    assert_eq!(t.dtf, 0);
    assert_eq!(real.harness.zone_coarsenings, 0);
    assert_eq!(real.harness.unexcused_drops, 0);
    assert_eq!(real.harness.lost_samples, 0);
}

#[test]
fn real_pages_with_drawn_nodes_are_not_evicted() {
    let Some(mut real) = open_real("belvedere-v8.oct", 4.0, REGRAFT_PAGE_BUDGET) else {
        return;
    };
    let (a, b) = region_views(&real);
    real.harness.settle(&a, 3, 20_000);
    real.harness.camera_moved();
    // Travel to B; every frame also tries to graft the page of each root anchor still unloaded
    // (a host fetching greedily). The budget cannot hold them all: the grafts that would need
    // the pages of a drawn node are refused, and the pool never draws a node that left.
    let mut refused = 0;
    let mut retained_frames = 0;
    for frame in 0..60 {
        let info = real.harness.frame(&b);
        retained_frames += usize::from(info.apply == CutApply::RetainedPrevious);
        for anchor in real.anchor_ids.clone() {
            if !real
                .harness
                .dataset
                .metadata
                .hierarchy()
                .is_unloaded(anchor)
            {
                continue;
            }
            let page = (real.harness.pages)(anchor).unwrap();
            if let Err(error) = real.harness.dataset.graft_page(page) {
                assert!(
                    matches!(error, LodDatasetError::PageBudget { .. }),
                    "{error}"
                );
                refused += 1;
            }
            let hierarchy = real.harness.dataset.metadata.hierarchy();
            for drawn in real.harness.dataset.cells.pool_drawn() {
                assert!(
                    hierarchy.node(*drawn).is_some(),
                    "frame {frame}: the pool draws {drawn:?} but its page was evicted"
                );
            }
        }
        if !real.harness.dirty && !real.harness.driver.needs_frame() && frame > 30 {
            break;
        }
    }
    real.harness.report("belvedere pinned");
    println!("belvedere pinned: {refused} grafts refused, {retained_frames} frames retained");
    assert!(refused > 0, "the budget must refuse at least one graft");
    assert_eq!(real.harness.zone_coarsenings, 0);
    assert_eq!(real.harness.unexcused_drops, 0);
    assert_eq!(real.harness.lost_samples, 0);
}

// ---------------------------------------------------------------------------------------------
// Colour generations (R3-Q33)
// ---------------------------------------------------------------------------------------------

/// The synthetic dataset with blocks painted like an attribute mode paints them: while `paint`
/// is set, the red channel follows x and the others are fixed.
type PageCells = HashMap<(NodeId, u8, usize), u64>;

fn painting_harness(paint: Rc<std::cell::Cell<bool>>) -> Harness {
    let PagedSynthetic {
        dataset,
        blocks,
        pages,
    } = paged_synthetic(1 << 20, 64 << 20);
    Harness::new(
        dataset,
        2.0,
        Box::new(move |id| {
            Ok(blocks.get(&id).map(|block| {
                if !paint.get() {
                    return block.clone();
                }
                let points = block
                    .points()
                    .points()
                    .iter()
                    .map(|point| {
                        let red = (f64::from(point.position[0]) / SIDE * 255.0) as u8;
                        Point::new(point.position, ColorRgb::new(red, 20, 200))
                    })
                    .collect();
                PointBlock::new(block.id(), PointBatch::new(points), Vec::new())
            }))
        }),
        Box::new(move |anchor| {
            let (number, nodes) = pages.get(&anchor).ok_or("no such page")?;
            Ok(PageInsert {
                anchor,
                number: *number,
                nodes: nodes.clone(),
                unloaded: Vec::new(),
                bytes: 100,
            })
        }),
    )
}

/// Frames until the ladder is at the target and the driver has nothing left, calling `check`
/// after every frame. Returns the nodes of the cut at rest.
fn run_to_rest(
    h: &mut Harness,
    view: &ViewProjection,
    mut check: impl FnMut(&Harness),
) -> Vec<NodeId> {
    let mut calm = 0;
    for _ in 0..2_000 {
        h.frame(view);
        check(h);
        if !h.dirty && h.stage == Some(2.0) && !h.driver.needs_frame() {
            calm += 1;
            if calm == 3 {
                return h.dataset.cells.pool_drawn().to_vec();
            }
        } else {
            calm = 0;
        }
    }
    panic!("no rest: {:?}", h.totals.stages);
}

#[test]
fn switching_colour_rebuilds_every_drawn_node_once_without_holes_or_mixed_generations() {
    let paint = Rc::new(std::cell::Cell::new(false));
    let mut h = painting_harness(Rc::clone(&paint));
    let view = synthetic_views().0;
    let mut phases: Vec<(u64, PageCells)> = Vec::new();
    let snapshot = |h: &Harness| (h.target.color_digest(), h.target.active_cells());
    let mut refs: Vec<PageCells> = Vec::new();
    // rgb -> attribute -> rgb -> attribute, each switch with `paint` and the colour generation
    // changed together (the binding does both).
    for (phase, on) in [false, true, false, true].into_iter().enumerate() {
        if phase > 0 {
            paint.set(on);
            assert!(h.dataset.set_cell_paint(on.then_some((0, 0))));
            h.dirty = true;
        }
        let (old, new) = match phase {
            0 | 1 => (None, None),
            _ => (Some(refs[phase - 1].clone()), Some(refs[phase - 2].clone())),
        };
        let cut = run_to_rest(&mut h, &view, |h| {
            // Holes are checked by the harness on every frame. A node never shows cells of two
            // generations: its pages are all the old ones or all the new ones.
            let (Some(old), Some(new)) = (&old, &new) else {
                return;
            };
            let mut seen: HashMap<NodeId, (bool, bool)> = HashMap::new();
            for (key, hash) in h.target.active_cells() {
                let entry = seen.entry(key.0).or_default();
                let (is_old, is_new) = (old.get(&key) == Some(&hash), new.get(&key) == Some(&hash));
                if is_old != is_new {
                    entry.0 |= is_old;
                    entry.1 |= is_new;
                }
            }
            for (node, (from_old, from_new)) in seen {
                assert!(
                    !(from_old && from_new),
                    "{node:?} draws old and new colours together"
                );
            }
        });
        assert!(!cut.is_empty());
        for node in &cut {
            // Built once at the start and once per switch.
            assert_eq!(
                h.totals.builds.get(node).copied(),
                Some(phase + 1),
                "phase {phase}: {node:?}"
            );
        }
        let (digest, cells) = snapshot(&h);
        phases.push((digest, cells.clone()));
        refs.push(cells);
    }
    assert_ne!(
        phases[0].0, phases[1].0,
        "attribute colours differ from rgb"
    );
    assert_eq!(phases[0].0, phases[2].0, "back to rgb gives the rgb image");
    assert_eq!(phases[1].0, phases[3].0, "the attribute image repeats");
}

#[test]
fn a_late_node_of_an_older_colour_generation_is_dropped_by_the_driver() {
    let mut h = synthetic_harness(1 << 20, 2.0);
    let view = synthetic_views().0;
    let config = h.config;
    let plan = |h: &mut Harness| {
        let mut nodes = Vec::new();
        for _ in 0..600 {
            let update = h
                .dataset
                .plan_cell_update(&view, 2.0, config, None, &mut h.blocks)
                .unwrap();
            nodes.extend(update.new_nodes);
            for anchor in update.stats.pending_page_anchors {
                let page = (h.pages)(anchor).unwrap();
                let _ = h.dataset.graft_page(page);
            }
            if nodes.len() >= 3 {
                break;
            }
        }
        nodes
    };
    let old = plan(&mut h);
    assert!(old.len() >= 3);
    h.dataset.set_cell_paint(Some((0, 0)));
    let new = plan(&mut h);
    assert!(new.len() >= 3);
    assert!(
        new[0].0 > old[0].0,
        "a colour change gives a newer generation"
    );
    let mut driver = driver_for(&h.dataset, u64::MAX);
    add_cell_nodes(&mut driver, new.iter().cloned()).unwrap();
    let bytes = driver.cpu_bytes();
    // The nodes of the old generation arrive late, for nodes the driver holds and for nodes it
    // does not: nothing changes.
    add_cell_nodes(&mut driver, old.iter().cloned()).unwrap();
    assert_eq!(driver.cpu_bytes(), bytes);
    for (generation, node) in &old {
        assert_eq!(
            driver.has_node(node.id),
            new.iter().any(|(_, n)| n.id == node.id),
            "{:?}",
            node.id
        );
        assert!(*generation < new[0].0);
    }
}

/// The built-in sample loaded in a harness, with its coordinate origin and visual scale.
fn sample_harness() -> (Harness, [f64; 3], f64) {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/web-host/sample.oct");
    let file = std::fs::File::open(path).unwrap();
    let reader: RealReader =
        FormatReader::open_lazy(std::io::BufReader::new(file), FormatReaderLimits::default())
            .unwrap();
    let scale = reader.format_metadata().visual_scale();
    let mut dataset = LodDataset::from_shared_metadata_and_visual_scale(
        reader.format_metadata().viewer_dataset().unwrap(),
        scale,
        256 << 20,
    )
    .with_global_frame(reader.format_version().uses_global_frame());
    if let Some(source) = reader.page_source().cloned() {
        dataset = dataset
            .with_pages(
                source.root_unloaded(),
                Box::new({
                    let s = source.clone();
                    move |a| s.release_page(a)
                }),
                256 << 20,
            )
            .unwrap();
    }
    let origin = dataset.coordinate_origin();
    let reader = Rc::new(RefCell::new(reader));
    let for_blocks = Rc::clone(&reader);
    let for_pages = Rc::clone(&reader);
    let blocks: Blocks = Box::new(move |id| {
        crate::block_loading::load_render_block(
            &mut *for_blocks.borrow_mut(),
            id,
            Some(viewer_render_wgpu::ColorMode::Rgb),
            None,
            None,
        )
        .map(Some)
    });
    let pages: Pages = Box::new(move |anchor| {
        let graft = for_pages
            .borrow_mut()
            .read_page(anchor)
            .map_err(|e| e.to_string())?;
        Ok(PageInsert {
            anchor,
            number: graft.number,
            nodes: graft.nodes,
            unloaded: graft.unloaded,
            bytes: graft.encoded_len,
        })
    });
    (Harness::new(dataset, 4.0, blocks, pages), origin, scale)
}

/// The built-in sample at a close pose: the leaves are tall cubes whose octants cross the eye plane
/// while the data lies in front of it. No node may draw its finest depth (cells of ~0.003 px)
/// there: the probe measures the data, not the node box.
#[test]
fn the_close_pose_of_the_sample_never_draws_sub_pixel_cells_in_cell_mode() {
    const MAX_DEPTH: u8 = 14;
    let (mut h, origin, scale) = sample_harness();
    let viewport = [870, 750];
    // Measured at this pose: probing the data wants depth 11-13 in these leaves (cells near the
    // px target), while probing the node box wants the finest depth, 19 (cells of ~0.003 px).
    // 14 is one level of margin over the former and far from the latter.
    let local = |p: [f64; 3]| -> [f64; 3] { std::array::from_fn(|a| (p[a] - origin[a]) * scale) };
    let mk = |eye: [f64; 3], target: [f64; 3]| {
        let root = h.dataset.metadata.hierarchy().stats().root_bounds;
        let mut camera = OrbitCamera::default();
        assert!(camera.fit(
            SceneBounds {
                min: local(root.min()),
                max: local(root.max())
            },
            viewport
        ));
        assert!(camera.set_pose(local(eye), local(target)));
        ViewProjection::new(camera, viewport, scale, VoxelBuildLimits::default())
    };
    let target = [31.958_496_093_75, 31.958_496_093_75, 1.999_511_718_75];
    let v0 = mk(
        [
            31.958_496_093_75,
            75.781_277_364_260_28,
            77.902_795_408_251_59,
        ],
        target,
    );
    let v4 = mk(
        [
            31.958_496_093_75,
            38.759_519_607_345_27,
            6.118_357_964_164_779,
        ],
        target,
    );
    h.settle(&v0, 3, 5000);
    h.camera_moved();
    h.settle(&v4, 3, 5000);
    let mut seen = std::collections::BTreeSet::new();
    for (node, wanted, drawn) in &h.last_entries {
        if [13, 14, 17, 18].contains(&node.get()) {
            for o in 0..8 {
                let (Some(w), Some(d)) = (wanted[o], drawn[o]) else {
                    continue;
                };
                seen.insert(node.get());
                assert!(
                    w <= MAX_DEPTH && d <= MAX_DEPTH,
                    "node {} octant {o} wants {w} and draws {d}",
                    node.get()
                );
            }
        }
    }
    assert_eq!(
        seen.into_iter().collect::<Vec<_>>(),
        [13, 14, 17, 18],
        "every leaf must have an octant with a wanted and drawn depth, or the check is vacuous"
    );
}

/// The sample from a close pose whose leaves hold the eye (nodes 14 and 17): their octants that
/// straddle the eye plane want a far finer depth than the ones wholly in front. The node's active
/// depths must stay contiguous up to that wish, or the per-cell rule jumps from the coarse range
/// to a depth whose cells measure a fraction of a pixel (black holes).
#[test]
fn a_node_holding_the_eye_keeps_contiguous_active_depths_and_no_sub_pixel_cells() {
    let (mut h, origin, scale) = sample_harness();
    let viewport = [870, 750];
    let local = |p: [f64; 3]| -> [f64; 3] { std::array::from_fn(|a| (p[a] - origin[a]) * scale) };
    let mk = |eye: [f64; 3], target: [f64; 3]| {
        let root = h.dataset.metadata.hierarchy().stats().root_bounds;
        let mut camera = OrbitCamera::default();
        assert!(camera.fit(
            SceneBounds {
                min: local(root.min()),
                max: local(root.max())
            },
            viewport
        ));
        assert!(camera.set_pose(eye, target));
        ViewProjection::new(camera, viewport, scale, VoxelBuildLimits::default())
    };
    // Local-frame poses: the eye 9 units from the target, 1.4 above it.
    let target = [50.0, 50.0, 3.22];
    let far = mk([50.0, 90.0, 40.0], target);
    let p2 = mk([50.0, 59.11, 4.60], target);
    h.settle(&far, 3, 5000);
    h.camera_moved();
    h.settle(&p2, 3, 5000);
    let ctx = h.cell_ctx.as_ref().expect("cell mode context");
    let grid = h.dataset.cell_grid();
    let hierarchy = h.dataset.metadata.hierarchy();
    let mut seen = 0;
    let mut checked = 0;
    for (node, depths, range) in &h.last_levels {
        let mut depths = depths.clone();
        depths.sort_unstable();
        if ![14, 17].contains(&node.get()) {
            continue;
        }
        let (Some(&first), Some(&last)) = (depths.first(), depths.last()) else {
            continue;
        };
        seen += 1;
        let lo = range.map_or(first, |(lo, _)| lo.max(first));
        let missing: Vec<u8> = (lo..=last).filter(|d| !depths.contains(d)).collect();
        assert!(
            missing.is_empty(),
            "node {} active {depths:?} range {range:?}: {missing:?} missing",
            node.get()
        );
        // No cell of the node's lattice is drawn below half the pixel target, unless it is the
        // node's coarsest active depth (nothing coarser to fall back on).
        let bounds = hierarchy.node(*node).unwrap().bounds();
        let node_depth = representative_depth(&grid, bounds).saturating_sub(3);
        let rel_lo: [f64; 3] =
            std::array::from_fn(|a| (bounds.min()[a] - origin[a]) * scale - ctx.eye[a]);
        let lod = NodeLod {
            anchor: rel_lo.map(|v| v as f32),
            node_depth,
            active: depths.iter().fold(0_u64, |b, d| b | 1 << (d - node_depth)),
            mask: [0; 8],
        };
        let side = ctx.root_side / (1_u64 << node_depth) as f64 * ctx.scale;
        for n in 0..343_u16 {
            let at = [n % 7, n / 7 % 7, n / 49];
            let rel: [f64; 3] = std::array::from_fn(|a| {
                let t = 0.01 + 0.98 * (f64::from(at[a]) + 0.37) / 6.37;
                rel_lo[a] + (bounds.max()[a] - bounds.min()[a]) * scale * t
            });
            let Some(depth) = drawn_depth_at(&ctx.lod, &lod, rel, rel_lo, side) else {
                continue;
            };
            if depth == first {
                continue;
            }
            let cells = (1_u64 << (depth - node_depth)) as f64;
            let cell_side = side / cells;
            let cell: [f64; 3] = std::array::from_fn(|a| {
                ((rel[a] - rel_lo[a]) / cell_side)
                    .floor()
                    .clamp(0.0, cells - 1.0)
            });
            let cell_min: [f32; 3] =
                std::array::from_fn(|a| (rel_lo[a] + cell[a] * cell_side) as f32);
            let px = edge_px(&ctx.lod, cell_min, cell_side as f32);
            checked += 1;
            assert!(
                px >= 2.0,
                "node {} draws depth {depth} at {px} px (active {depths:?})",
                node.get()
            );
        }
    }
    assert_eq!(seen, 2, "nodes 14 and 17 are drawn");
    assert!(checked > 100, "too few lattice samples measured: {checked}");
}

/// Diagnostic (prints, asserts nothing): the built-in sample at pose P4 of the web demo (close
/// pose: from the initial fit, 8 wheel steps of -0.3 zoom each, then a right drag of 120 px
/// upward on an 870 x 750 canvas). For every face of every node of the cut it takes a grid of
/// point pairs straddling the face, asks the twin of the GPU rule (`drawn_depth_at`) which node
/// draws each side and at which depth, and prints, the nodes' active, resident,
/// wanted and drawn depths and ranges, a histogram of the level differences of the pairs where
/// both samples are on screen, and the worst pairs with their screen position.
/// Run: `cargo test --offline --profile fast -p viewer_wasm p4_border_levels -- --ignored --nocapture`.
#[test]
#[ignore = "diagnostic: prints the level difference across the P4 node borders"]
fn p4_border_levels() {
    let _ = p4_border_levels_under();
}

/// ADR 0055 plan, task 4: at P4 node 14 (beside node 8, which draws level 10 across their border)
/// can draw level 10 because its `lo` comes from `node_view_range`, and no visible border pair
/// differs by more than one level. `node_far_depth` gave node 14 the range 11..13.
#[test]
fn at_p4_node_14_can_draw_the_level_its_border_needs() {
    let (histogram, node_14) = p4_border_levels_under();
    let (lo, _) = node_14.expect("node 14 has a range at P4");
    assert!(lo <= 10, "node 14 range.lo {lo} > 10");
    assert!(
        histogram.keys().all(|diff| *diff <= 1),
        "border pairs by level difference: {histogram:?}"
    );
}

/// Returns the histogram of the visible border pairs by level difference, and the range of node 14.
fn p4_border_levels_under() -> (std::collections::BTreeMap<u8, usize>, Option<(u8, u8)>) {
    type Group = (usize, [f64; 2], [f64; 2]);
    const N: usize = 48;
    let (mut h, origin, scale) = sample_harness();
    let viewport = [870, 750];
    let local = |p: [f64; 3]| -> [f64; 3] { std::array::from_fn(|a| (p[a] - origin[a]) * scale) };
    // The host fits the camera to the data extent (`LodDataset::fit_camera`).
    let root = h.dataset.metadata.hierarchy().stats().data_bounds;
    let mut camera = OrbitCamera::default();
    assert!(camera.fit(
        SceneBounds {
            min: local(root.min()),
            max: local(root.max())
        },
        viewport
    ));
    let initial = ViewProjection::new(camera, viewport, scale, VoxelBuildLimits::default());
    // The web host sums the wheel deltas (deltaY * 0.001) and the pointer deltas (normalised by
    // the canvas size) of a frame and applies zoom, then orbit.
    let mut p4_camera = camera;
    assert!(p4_camera.zoom(-0.3 * 8.0));
    assert!(p4_camera.orbit(0.0, -120.0 / 750.0));
    let p4 = ViewProjection::new(p4_camera, viewport, scale, VoxelBuildLimits::default());
    println!(
        "== P4 eye {:?} target {:?} (dataset frame of the close-pose test: eye \
         [31.958, 38.760, 6.118] target [31.958, 31.958, 1.9995])",
        p4_camera.position(),
        p4_camera.target()
    );
    h.settle(&initial, 3, 5000);
    h.camera_moved();
    h.settle(&p4, 3, 5000);
    far_diagnosis(&h, &p4, &[14, 8]);

    let ctx = h.cell_ctx.as_ref().expect("cell mode context");
    let grid = h.dataset.cell_grid();
    let hierarchy = h.dataset.metadata.hierarchy();
    let nodes = &h.cell_nodes;
    let to_eye = |p: [f64; 3]| -> [f64; 3] {
        std::array::from_fn(|a| (p[a] - ctx.origin[a]) * ctx.scale - ctx.eye[a])
    };
    // The unique drawer of a point: (node index, depth).
    let drawer = |p: [f64; 3]| -> Option<(usize, u8)> {
        let rel = to_eye(p);
        let mut found = None;
        for (j, node) in nodes.iter().enumerate() {
            if !(0..3).all(|a| p[a] >= node.min[a] && p[a] <= node.max[a]) {
                continue;
            }
            if let Some(depth) = drawn_depth_at(&ctx.lod, &node.lod, rel, node.lo, node.side) {
                found = Some((j, depth));
            }
        }
        found
    };
    // Projected edge of the cell of node `j` at `depth` that holds `rel`.
    let cell_px = |j: usize, depth: u8, rel: [f64; 3]| -> f32 {
        let node = &nodes[j];
        let level = depth - node.lod.node_depth;
        let cells = (1_u64 << level) as f64;
        let side = node.side / cells;
        let min: [f32; 3] = std::array::from_fn(|a| {
            (node.lo[a]
                + ((rel[a] - node.lo[a]) / side)
                    .floor()
                    .clamp(0.0, cells - 1.0)
                    * side) as f32
        });
        edge_px(&ctx.lod, min, side as f32)
    };
    let screen = |rel: [f64; 3]| -> [f64; 2] {
        let dot = |v: [f32; 3]| (0..3).map(|a| rel[a] * f64::from(v[a])).sum::<f64>();
        let depth = dot(ctx.lod.forward);
        let focal = f64::from(ctx.lod.focal);
        [
            ctx.viewport[0] * 0.5 + dot(ctx.lod.right) * focal / depth,
            ctx.viewport[1] * 0.5 - dot(ctx.lod.up) * focal / depth,
        ]
    };

    println!("-- nodes of the cut ({})", nodes.len());
    for (j, node) in nodes.iter().enumerate() {
        let (_, depths, range) = &h.last_levels[j];
        let (_, wanted, drawn) = &h.last_entries[j];
        let mut depths = depths.clone();
        depths.sort_unstable();
        let resident: Vec<u8> = (node.lod.node_depth..=node.lod.node_depth + MAX_NODE_SPAN)
            .filter(|d| {
                h.driver
                    .pool()
                    .is_resident(h.driver.pool().key(node.id, *d))
            })
            .collect();
        let bounds = hierarchy.node(node.id).unwrap().bounds();
        println!(
            "node {:>3} depth {} bounds {:.1?}..{:.1?} active {depths:?} resident {resident:?} \
             range {range:?}\n          wanted {wanted:?}\n          drawn {drawn:?}",
            node.id.get(),
            representative_depth(&grid, bounds).saturating_sub(3),
            bounds.min(),
            bounds.max(),
        );
    }

    // Only the data matters on screen: the cells above and below the point cloud draw nothing.
    let data = h.dataset.metadata.hierarchy().stats().data_bounds;
    println!("data bounds {:?}..{:?}", data.min(), data.max());
    let mut bands: std::collections::BTreeMap<usize, std::collections::BTreeMap<u8, usize>> =
        std::collections::BTreeMap::default();
    // `P4_DUMP=<dir>` writes every visible pair as `x y z depth_a depth_b node_a node_b` lines.
    let mut dump = String::new();
    let mut histogram: std::collections::BTreeMap<u8, usize> =
        std::collections::BTreeMap::default();
    // (node a, depth a, node b, depth b) -> (pairs, min xy, max xy)
    let mut groups: std::collections::BTreeMap<(u64, u8, u64, u8), Group> =
        std::collections::BTreeMap::default();
    for node in nodes {
        for axis in 0..3 {
            let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
            for upper in [false, true] {
                for i in 0..N {
                    for j in 0..N {
                        let extent: [f64; 3] = std::array::from_fn(|a| node.max[a] - node.min[a]);
                        let eps = extent[axis] * 1e-4;
                        let mut inside = [0.0; 3];
                        inside[u] = node.min[u] + extent[u] * (i as f64 + 0.5) / N as f64;
                        inside[v] = node.min[v] + extent[v] * (j as f64 + 0.5) / N as f64;
                        let mut outside = inside;
                        let face = if upper {
                            node.max[axis]
                        } else {
                            node.min[axis]
                        };
                        let sign = if upper { 1.0 } else { -1.0 };
                        inside[axis] = face - sign * eps;
                        outside[axis] = face + sign * eps;
                        let (Some((ja, da)), Some((jb, db))) = (drawer(inside), drawer(outside))
                        else {
                            continue;
                        };
                        if ja == jb {
                            continue;
                        }
                        if !(0..3).all(|a| inside[a] >= data.min()[a] && inside[a] <= data.max()[a])
                        {
                            continue;
                        }
                        let (ra, rb) = (to_eye(inside), to_eye(outside));
                        if !(ctx.on_screen(ra) && ctx.on_screen(rb)) {
                            continue;
                        }
                        *histogram.entry(da.abs_diff(db)).or_default() += 1;
                        let at = screen(ra);
                        *bands
                            .entry((at[1] / 50.0).floor().max(0.0) as usize * 50)
                            .or_default()
                            .entry(da.abs_diff(db))
                            .or_default() += 1;
                        let _ = writeln!(
                            dump,
                            "{:.1} {:.1} {:.3} {da} {db} {} {} {:.2} {:.2}",
                            at[0],
                            at[1],
                            inside[2],
                            nodes[ja].id.get(),
                            nodes[jb].id.get(),
                            cell_px(ja, da, ra),
                            cell_px(jb, db, rb)
                        );
                        let entry = groups
                            .entry((nodes[ja].id.get(), da, nodes[jb].id.get(), db))
                            .or_insert((0, at, at));
                        entry.0 += 1;
                        for k in 0..2 {
                            entry.1[k] = entry.1[k].min(at[k]);
                            entry.2[k] = entry.2[k].max(at[k]);
                        }
                    }
                }
            }
        }
    }
    println!("-- visible border pairs by level difference: {histogram:?}");
    if let Ok(dir) = std::env::var("P4_DUMP") {
        std::fs::write(format!("{dir}/pairs.txt"), dump).unwrap();
    }
    println!("-- level differences per 50 px screen row band: {bands:?}");
    let mut worst: Vec<_> = groups.iter().collect();
    worst.sort_by_key(|((_, da, _, db), (n, ..))| {
        (std::cmp::Reverse(da.abs_diff(*db)), std::cmp::Reverse(*n))
    });
    for ((a, da, b, db), (n, lo, hi)) in worst.into_iter().take(25) {
        println!(
            "diff {} : node {a} @ depth {da} | node {b} @ depth {db} : {n} pairs, screen x {:.0}..{:.0} y {:.0}..{:.0}",
            da.abs_diff(*db),
            lo[0],
            hi[0],
            lo[1],
            hi[1]
        );
    }
    let node_14 = nodes
        .iter()
        .position(|node| node.id.get() == 14)
        .and_then(|j| h.last_levels[j].2);
    (histogram, node_14)
}

// ---------------------------------------------------------------------------------------------
// ADR 0055 plan, task 2: what the twin wants per node against the node range (diagnostics)
// ---------------------------------------------------------------------------------------------

const DEMO_VIEWPORT: [u32; 2] = [870, 750];
/// Frames of the wheel and drag deltas of the web demo: a wheel step is -0.3 of zoom, a pointer
/// drag is normalised by the canvas height.
const WHEEL_STEP: f64 = -0.3;

/// The camera of a pose of the web demo, from the initial fit: P1 = right drag 150 px upward then
/// 3 wheel steps; P2 = P1 + 6 wheel steps + right drag 80; P3 = P2 + 4 wheel steps; P4 = 8 wheel
/// steps + right drag 120; P5 = P2 + 6 half wheel steps. The drag direction (upward, as in P4) and
/// the order inside a pose are assumptions: zoom and orbit commute except at the pitch clamp.
fn demo_pose_camera(fit: OrbitCamera, pose: &str) -> OrbitCamera {
    let mut camera = fit;
    let drag = |camera: &mut OrbitCamera, px: f64| assert!(camera.orbit(0.0, -px / 750.0));
    let wheel = |camera: &mut OrbitCamera, steps: usize, k: f64| {
        (0..steps).for_each(|_| assert!(camera.zoom(WHEEL_STEP * k)));
    };
    match pose {
        "P1" => {
            drag(&mut camera, 150.0);
            wheel(&mut camera, 3, 1.0);
        }
        "P2" | "P3" | "P5" => {
            drag(&mut camera, 150.0);
            wheel(&mut camera, 9, 1.0);
            drag(&mut camera, 80.0);
            match pose {
                "P3" => wheel(&mut camera, 4, 1.0),
                "P5" => wheel(&mut camera, 6, 0.5),
                _ => {}
            }
        }
        "P4" => {
            wheel(&mut camera, 8, 1.0);
            drag(&mut camera, 120.0);
        }
        other => panic!("unknown pose {other}"),
    }
    camera
}

/// The sample settled at `pose` (from the initial fit, like `p4_border_levels`).
fn settled_sample_pose(pose: &str) -> (Harness, ViewProjection) {
    let (mut h, origin, scale) = sample_harness();
    let local = |p: [f64; 3]| -> [f64; 3] { std::array::from_fn(|a| (p[a] - origin[a]) * scale) };
    let data = h.dataset.metadata.hierarchy().stats().data_bounds;
    let mut fit = OrbitCamera::default();
    assert!(fit.fit(
        SceneBounds {
            min: local(data.min()),
            max: local(data.max())
        },
        DEMO_VIEWPORT
    ));
    let initial = ViewProjection::new(fit, DEMO_VIEWPORT, scale, VoxelBuildLimits::default());
    let view = ViewProjection::new(
        demo_pose_camera(fit, pose),
        DEMO_VIEWPORT,
        scale,
        VoxelBuildLimits::default(),
    );
    h.settle(&initial, 3, 5000);
    h.camera_moved();
    h.settle(&view, 3, 5000);
    (h, view)
}

/// `gpu_input_digest` of the settled cut at each demo pose, golden. It hashes what
/// the GPU receives per node (active-depth bits, mask, node depth), not the per-octant fields of
/// the entries, so it must not change while those fields are replaced by node-level ones. The
/// cut goldens of `cell_cut_tests.rs` are pure cuts without a pool, so they emit no active bits:
/// these poses stand in for them.
const GPU_INPUT_GOLDEN: &[(&str, u64)] = &[
    ("P1", 0x2f2f_4f1a_b372_490e),
    ("P2", 0xe0f8_8c8d_0cc1_68e4),
    ("P3", 0xdc13_ecd0_6a84_91cc),
    ("P4", 0xdfa4_469d_d7df_e145),
    ("P5", 0xe105_c88c_8ae1_50cc),
];

#[test]
fn the_gpu_input_at_the_sample_poses_is_pinned() {
    let mut actual = Vec::new();
    for pose in ["P1", "P2", "P3", "P4", "P5"] {
        let (h, _) = settled_sample_pose(pose);
        actual.push((pose.to_owned(), h.totals.gpu_digest_last));
    }
    let matches = actual.len() == GPU_INPUT_GOLDEN.len()
        && actual
            .iter()
            .zip(GPU_INPUT_GOLDEN)
            .all(|(a, g)| a.0 == g.0 && a.1 == g.1);
    if !matches {
        use std::fmt::Write as _;
        let table: String = actual
            .iter()
            .fold(String::new(), |mut table, (name, hash)| {
                let _ = writeln!(table, "    (\"{name}\", {hash:#018x}),");
                table
            });
        panic!("GPU input table changed; actual:\n{table}");
    }
}

fn unit(v: [f64; 3]) -> [f64; 3] {
    let n = v.iter().map(|c| c * c).sum::<f64>().sqrt();
    v.map(|c| c / n)
}

/// `lod_near_depth` of the camera, scaled units (`OrbitCamera::lod_near_depth` is crate-private:
/// this restates it from the public f64 array `[target, distance, yaw, pitch, radius, centre, ..]`).
fn lod_near_depth_scaled(camera: &OrbitCamera) -> f64 {
    let a = camera.to_f64_array();
    let position = camera.position();
    let forward = unit(std::array::from_fn(|i| a[i] - position[i]));
    let centre_depth: f64 = (0..3).map(|i| (a[7 + i] - position[i]) * forward[i]).sum();
    (centre_depth - a[6] * 1.2).max(a[6] * 0.001).max(0.001)
}

/// The cell `OctantProbe::at` places at `anchor` (relative frame: dataset minus origin) inside the
/// box `[bmin, bmax]` for the branch without `on_axis`: `(cell px, smallest child px, cell min)`.
fn probe_cell(
    view: &ViewProjection,
    root_side: f64,
    bmin: [f64; 3],
    bmax: [f64; 3],
    anchor: [f64; 3],
    depth: u8,
) -> (f64, f64, [f64; 3]) {
    let side = root_side / 2.0_f64.powi(i32::from(depth));
    let min: [f64; 3] = std::array::from_fn(|a| {
        let low = bmin[a];
        let high = (bmax[a] - side).max(low);
        anchor[a].clamp(bmin[a], bmax[a]).clamp(low, high)
    });
    let edge = |lo: [f64; 3], s: f64| {
        view.projected_voxel_edge_pixels(
            SpatialBounds::new(lo, lo.map(|v| v + s)).unwrap(),
            [0.0; 3],
        )
    };
    let half = side / 2.0;
    let smallest = (0..8_usize)
        .map(|c| {
            edge(
                std::array::from_fn(|a| min[a] + half * ((c >> a & 1) as f64)),
                half,
            )
        })
        .fold(f64::INFINITY, f64::min);
    (edge(min, side), smallest, min)
}

/// Deliverable A: why `node_far_depth` answers what it does for the nodes `ids`: the far corner,
/// then at that corner and at the centre of the 8|14 border on the ground, per depth, the px of
/// the cell and of its smallest child as the probe measures them (`OctantProbe::at_anchor`,
/// restated: the type is crate-private) and as the twin measures the lattice cell actually there.
fn far_diagnosis(h: &Harness, view: &ViewProjection, ids: &[u64]) {
    let ctx = h.cell_ctx.as_ref().expect("cell mode context");
    let grid = h.dataset.cell_grid();
    let hierarchy = h.dataset.metadata.hierarchy();
    let camera = view.camera();
    let scale = ctx.scale;
    let eye_rel = camera.position().map(|v| v / scale);
    let forward = unit(std::array::from_fn(|a| {
        camera.target()[a] - camera.position()[a]
    }));
    let data = hierarchy.stats().data_bounds;
    let target = f64::from(ctx.lod.target_px);
    println!(
        "-- far diagnosis: forward {forward:.4?} eye (relative, dataset units) {eye_rel:.3?} \
         origin {:?} target_px {target} min_side {} (scaled)",
        ctx.origin, ctx.lod.min_side
    );
    for id in ids {
        let Some(node) = h.cell_nodes.iter().find(|n| n.id.get() == *id) else {
            println!("node {id}: not in the cut");
            continue;
        };
        let j = h.cell_nodes.iter().position(|n| n.id == node.id).unwrap();
        let bounds = hierarchy.node(node.id).unwrap().bounds();
        let rep = representative_depth(&grid, bounds);
        let resident: Vec<u8> = (rep.saturating_sub(3)..=rep + 20)
            .filter(|d| {
                h.driver
                    .pool()
                    .is_resident(h.driver.pool().key(node.id, *d))
            })
            .collect();
        let is_leaf = hierarchy.is_leaf(node.id) && !hierarchy.is_unloaded(node.id);
        let rmin: [f64; 3] = std::array::from_fn(|a| bounds.min()[a] - ctx.origin[a]);
        let rmax: [f64; 3] = std::array::from_fn(|a| bounds.max()[a] - ctx.origin[a]);
        let corner: [f64; 3] =
            std::array::from_fn(|a| if forward[a] > 0.0 { rmax[a] } else { rmin[a] });
        let corner_depth: f64 = (0..3).map(|a| (corner[a] - eye_rel[a]) * forward[a]).sum();
        // The centre of the border between nodes 8 and 14 (y = 32) on the ground (z = 0).
        let border = [
            f64::midpoint(rmin[0], rmax[0]),
            32.0 - ctx.origin[1],
            0.0 - ctx.origin[2],
        ];
        println!(
            "node {id}: node_depth {} rep {rep} leaf {is_leaf} buildable.0 {rep} (derived: rep; the \
             pool does not expose it; buildable.1 not exposed, resident {resident:?}) range {:?} \
             wanted {:?}",
            node.lod.node_depth, h.last_levels[j].2, h.last_entries[j].1
        );
        let rel_of =
            |p: [f64; 3]| -> [f64; 3] { std::array::from_fn(|a| p[a] * scale - ctx.eye[a]) };
        let screen = |rel: [f64; 3]| {
            let dot = |v: [f32; 3]| (0..3).map(|a| rel[a] * f64::from(v[a])).sum::<f64>();
            let d = dot(ctx.lod.forward);
            [
                ctx.viewport[0] * 0.5 + dot(ctx.lod.right) * f64::from(ctx.lod.focal) / d,
                ctx.viewport[1] * 0.5 - dot(ctx.lod.up) * f64::from(ctx.lod.focal) / d,
            ]
        };
        for (name, anchor) in [
            ("far corner", corner),
            ("border centre on the ground", border),
        ] {
            let in_data = (0..3).all(|a| {
                let p = anchor[a] + ctx.origin[a];
                p >= data.min()[a] && p <= data.max()[a]
            });
            let rel = rel_of(anchor);
            println!(
                "  anchor {name}: relative {anchor:.3?} view depth {:.3} in data bounds {in_data} \
                 on screen {} screen px {:.0?}",
                (0..3)
                    .map(|a| rel[a] * f64::from(ctx.lod.forward[a]))
                    .sum::<f64>(),
                ctx.on_screen(rel),
                screen(rel)
            );
            // The lattice cell actually there: the point nudged inside the node box.
            let inside: [f64; 3] = std::array::from_fn(|a| {
                let eps = (rmax[a] - rmin[a]) * 1e-6;
                anchor[a].clamp(rmin[a] + eps, rmax[a] - eps)
            });
            let inside_rel = rel_of(inside);
            let mut far = None;
            for depth in rep..=rep + 8 {
                let (cell, child, _) = probe_cell(view, grid.root_side, rmin, rmax, anchor, depth);
                let meets = child < target;
                if meets && far.is_none() {
                    far = Some(depth);
                }
                let level = depth - node.lod.node_depth;
                let cells = (1_u64 << level) as f64;
                let side = node.side / cells;
                let cell_idx: [u32; 3] = std::array::from_fn(|a| {
                    ((inside_rel[a] - node.lo[a]) / side)
                        .floor()
                        .clamp(0.0, cells - 1.0) as u32
                });
                let lo = cell_lo(&node.lod, cell_idx, side as f32);
                println!(
                    "    depth {depth:>2}: probe cell {cell:7.2} px, smallest child {child:7.2} px \
                     (meets = child < target: {meets}) | twin lattice cell {:7.2} px, smallest \
                     child {:7.2} px, refines {}",
                    edge_px(&ctx.lod, lo, side as f32),
                    min_child_edge_px(&ctx.lod, lo, side as f32),
                    refines(&ctx.lod, lo, side as f32)
                );
            }
            println!("    first depth that meets (linear search of node_far_depth): {far:?}");
        }
        // Sweep along the node's border at y = 32 (its face towards the eye side) from the far
        // corner to the other end, on the ground: view depth, screen position and the smallest
        // child of the depths `rep + 4 ..= rep + 6` as the probe measures it.
        println!(
            "  sweep along x at y = 32, z = 0 (probe, depths {}..={}):",
            rep + 4,
            rep + 6
        );
        for step in 0..=8 {
            let x = rmin[0] + (rmax[0] - rmin[0]) * f64::from(step) / 8.0;
            let anchor = [x, 32.0 - ctx.origin[1], 0.0 - ctx.origin[2]];
            let rel = rel_of(anchor);
            let children: Vec<String> = (rep + 4..=rep + 6)
                .map(|d| {
                    format!(
                        "d{d} {:.2}",
                        probe_cell(view, grid.root_side, rmin, rmax, anchor, d).1
                    )
                })
                .collect();
            println!(
                "    x {x:6.2}: view depth {:.3} screen x {:6.0} on screen {:5} child px {children:?}",
                (0..3)
                    .map(|a| rel[a] * f64::from(ctx.lod.forward[a]))
                    .sum::<f64>(),
                screen(rel)[0],
                ctx.on_screen(rel)
            );
        }
        println!(
            "  far corner view depth {corner_depth:.3} (eye plane crossed: {})",
            corner_depth <= 0.0
        );
    }
}

// --- B/C/D: the twin-wanted depth range of every node of the cut -----------------------------

/// Screen lattice step in px of the samples (a drawn cell is at least ~4 px wide).
const LATTICE_PX: usize = 6;
/// Geometric step of the samples along each ray (a cell changes by one depth per doubling).
const DEPTH_RATIO: f64 = 1.1;
/// px tolerance for f32 (twin) against f64 (probes) in the containment property.
const PX_EPSILON: f32 = 0.05;

/// The depths the twin draws at `rel` (eye-relative, scaled) for each px target of `targets`, if
/// every depth from `first` to `last` (the node's buildable span) were resident: the first depth
/// whose cell holding the point does not refine (`refines`, min-side rule included), else `last`.
/// The second value says whether the cell at the first target's depth crosses the eye plane
/// (`edge_px` is `+inf`). One descent serves all targets: the smallest-child measure does not
/// depend on the target.
fn twin_depths(
    view: &LodView,
    node: &CellNode,
    rel: [f64; 3],
    (first, last): (u8, u8),
    targets: [f32; 3],
) -> ([u8; 3], bool) {
    let (mut out, mut done, mut crosses) = ([last; 3], [false; 3], false);
    for depth in first..=last {
        let level = depth - node.lod.node_depth;
        let cells = (1_u64 << level) as f64;
        let side = node.side / cells;
        let cell: [u32; 3] = std::array::from_fn(|a| {
            ((rel[a] - node.lo[a]) / side)
                .floor()
                .clamp(0.0, cells - 1.0) as u32
        });
        let lo = cell_lo(&node.lod, cell, side as f32);
        let min_side_ok = side as f32 * 0.5 >= view.min_side;
        let smallest = if min_side_ok {
            min_child_edge_px(view, lo, side as f32)
        } else {
            0.0
        };
        for i in 0..3 {
            if !(done[i] || min_side_ok && smallest >= targets[i]) {
                (out[i], done[i]) = (depth, true);
                if i == 0 {
                    crosses = edge_px(view, lo, side as f32).is_infinite();
                }
            }
        }
        if done.iter().all(|d| *d) {
            break;
        }
    }
    (out, crosses)
}

/// The buildable span of a node as the dataset builds it: `NodeSetup` (first depth = the
/// representative depth, or the spacing's for a leaf with one; last = 5 deeper for an internal
/// node, `MAX_CELL_DEPTH` for a leaf), cut where `node_cell_level` first refuses a depth as too
/// sparse (the dataset's own refusal, found by bisection, as `build_node_levels` does). The pool
/// does not expose `PoolNode::buildable`, so this restates it; `None` when a block is not cached.
fn buildable_span(h: &Harness, id: NodeId) -> Option<(u8, u8)> {
    let hierarchy = h.dataset.metadata.hierarchy();
    let node = hierarchy.node(id)?;
    let blocks: Vec<PointBlock> = node
        .blocks()
        .iter()
        .map(|b| h.dataset.cache.peek(*b).cloned())
        .collect::<Option<_>>()?;
    let origin = h.dataset.coordinate_origin();
    let world = h.dataset.cell_grid();
    let grid = CellGrid {
        root_min: std::array::from_fn(|a| world.root_min[a] - origin[a]),
        root_side: world.root_side,
    };
    let local = SpatialBounds::new(
        std::array::from_fn(|a| node.bounds().min()[a] - origin[a]),
        std::array::from_fn(|a| node.bounds().max()[a] - origin[a]),
    )
    .ok()?;
    let is_leaf = hierarchy.is_leaf(id) && !hierarchy.is_unloaded(id);
    let rep = representative_depth(&grid, local);
    let first = match node.spacing().filter(|_| is_leaf) {
        Some(spacing) => rep.min(representative_depth_for_spacing(&grid, spacing)),
        None => rep,
    };
    let mut last = if is_leaf {
        MAX_CELL_DEPTH
    } else {
        first.saturating_add(5)
    };
    let ok =
        |depth: u8| node_cell_level(&grid, local, node.spacing(), is_leaf, &blocks, depth).is_ok();
    if !ok(last) {
        let (mut good, mut bad) = (first, last);
        while bad - good > 1 {
            let middle = good + (bad - good) / 2;
            if ok(middle) {
                good = middle;
            } else {
                bad = middle;
            }
        }
        last = good;
    }
    Some((first, last))
}

/// Everything measured for one node of the applied cut.
#[derive(Clone, Debug, Default)]
struct NodeRow {
    id: u64,
    node_depth: u8,
    /// Buildable span (`buildable_span`).
    span: (u8, u8),
    points: u64,
    eye_inside: bool,
    samples: usize,
    excluded_near: usize,
    excluded_cross: usize,
    /// Twin minimum and maximum wanted depth at the nominal target.
    twin: Option<(u8, u8)>,
    /// Twin minimum at `target + eps` and maximum at `target - eps` (the containment property).
    twin_eps: Option<(u8, u8)>,
    /// Today's `range`.
    range: Option<(u8, u8)>,
    /// The fit truncated the node this frame (`FramePlan::coarsened_nodes`), and the largest
    /// depth any octant wanted (already clamped to the capped buildable range).
    capped: bool,
    wanted_max: Option<u8>,
    /// First sample (view depth, screen px) that reached the twin minimum and maximum.
    at_min: Option<(f64, [usize; 2])>,
    at_max: Option<(f64, [usize; 2])>,
    /// The shipped candidate range (`node_view_range`).
    candidate: Option<(u8, u8)>,
}

/// Slab of the ray `d * t` inside the box `[min, max]` (eye-relative): `(t_in, t_out)`.
fn ray_slab(d: [f64; 3], min: [f64; 3], max: [f64; 3]) -> Option<(f64, f64)> {
    let (mut lo, mut hi) = (1e-4_f64, f64::INFINITY);
    for a in 0..3 {
        if d[a].abs() < 1e-12 {
            if min[a] > 0.0 || max[a] < 0.0 {
                return None;
            }
            continue;
        }
        let (t1, t2) = (min[a] / d[a], max[a] / d[a]);
        lo = lo.max(t1.min(t2));
        hi = hi.min(t1.max(t2));
    }
    (lo <= hi).then_some((lo, hi))
}

/// Measures every node of the settled cell cut at `view` (see the section header for the rules).
fn twin_rows(h: &Harness, view: &ViewProjection, lattice_px: usize) -> Vec<NodeRow> {
    let ctx = h.cell_ctx.as_ref().expect("cell mode context");
    let hierarchy = h.dataset.metadata.hierarchy();
    let camera = view.camera();
    let scale = ctx.scale;
    let near = lod_near_depth_scaled(&camera);
    let data = hierarchy.stats().data_bounds;
    let eye_ds: [f64; 3] = std::array::from_fn(|a| ctx.eye[a] / scale + ctx.origin[a]);
    let [w, hgt] = ctx.viewport;
    let focal = f64::from(ctx.lod.focal);
    let f64v = |v: [f32; 3]| v.map(f64::from);
    let (forward, right, up) = (f64v(ctx.lod.forward), f64v(ctx.lod.right), f64v(ctx.lod.up));
    let targets = [
        ctx.lod.target_px,
        ctx.lod.target_px + PX_EPSILON,
        ctx.lod.target_px - PX_EPSILON,
    ];
    let mut rows = Vec::new();
    for (j, node) in h.cell_nodes.iter().enumerate() {
        let span = buildable_span(h, node.id).expect("blocks of the cut are cached");
        let mut row = NodeRow {
            id: node.id.get(),
            node_depth: node.lod.node_depth,
            span,
            points: hierarchy.node(node.id).unwrap().point_count(),
            eye_inside: (0..3).all(|a| eye_ds[a] >= node.min[a] && eye_ds[a] <= node.max[a]),
            range: h.last_levels[j].2,
            capped: h.frame_capped.contains(&node.id),
            wanted_max: h.last_entries[j].1.iter().flatten().copied().max(),
            ..NodeRow::default()
        };
        // node ∩ data, eye-relative scaled.
        let lo_ds: [f64; 3] = std::array::from_fn(|a| node.min[a].max(data.min()[a]));
        let hi_ds: [f64; 3] = std::array::from_fn(|a| node.max[a].min(data.max()[a]));
        if (0..3).any(|a| lo_ds[a] >= hi_ds[a]) {
            rows.push(row);
            continue;
        }
        let rmin: [f64; 3] =
            std::array::from_fn(|a| (lo_ds[a] - ctx.origin[a]) * scale - ctx.eye[a]);
        let rmax: [f64; 3] =
            std::array::from_fn(|a| (hi_ds[a] - ctx.origin[a]) * scale - ctx.eye[a]);
        let (mut tmin, mut tmax) = (u8::MAX, 0_u8);
        let (mut emin, mut emax) = (u8::MAX, 0_u8);
        let mut visit = |t: f64, d: [f64; 3], at: [usize; 2], row: &mut NodeRow| {
            if t < near {
                row.excluded_near += 1;
                return;
            }
            let rel: [f64; 3] = std::array::from_fn(|a| d[a] * t);
            if !ctx.on_screen(rel) {
                return;
            }
            let (found, crosses) = twin_depths(&ctx.lod, node, rel, span, targets);
            let depth = found[0];
            if crosses {
                row.excluded_cross += 1;
                return;
            }
            row.samples += 1;
            if depth < tmin {
                row.at_min = Some((t, at));
            }
            if depth > tmax || row.at_max.is_none() {
                row.at_max = Some((t, at));
            }
            tmin = tmin.min(depth);
            tmax = tmax.max(depth);
            emin = emin.min(found[1]);
            emax = emax.max(found[2]);
        };
        let columns: Vec<usize> = (0..w as usize)
            .step_by(lattice_px)
            .chain([w as usize - 1])
            .collect();
        let rows_px: Vec<usize> = (0..hgt as usize)
            .step_by(lattice_px)
            .chain([hgt as usize - 1])
            .collect();
        for &py in &rows_px {
            for &px in &columns {
                let (u, v) = (
                    (px as f64 - w / 2.0) / focal,
                    (hgt / 2.0 - py as f64) / focal,
                );
                let d: [f64; 3] = std::array::from_fn(|a| forward[a] + right[a] * u + up[a] * v);
                let Some((t_in, t_out)) = ray_slab(d, rmin, rmax) else {
                    continue;
                };
                let t_out = t_out.min(ctx.clip.1);
                let mut t = t_in;
                while t <= t_out {
                    visit(t, d, [px, py], &mut row);
                    t *= DEPTH_RATIO;
                }
                visit(t_out, d, [px, py], &mut row);
            }
        }
        if row.samples > 0 {
            row.twin = Some((tmin, tmax));
            row.twin_eps = Some((emin, emax));
        }
        // The shipped candidate range (`node_view_range`, ADR 0055 plan task 3) over node ∩ data.
        row.candidate = node_view_range(
            view,
            &h.dataset.cell_grid(),
            h.dataset.coordinate_origin(),
            SpatialBounds::new(lo_ds, hi_ds).ok(),
            span,
            f64::from(ctx.lod.target_px),
            h.driver.pool().min_side(),
        );
        rows.push(row);
    }
    rows
}

/// `ceil(min(points, 8^(depth - node_depth)) / 512)` pages, at least one: the formula of the
/// pool's `estimate_pages` (`cell_pool/residency.rs`, private), restated.
fn estimate_pages_like_the_pool(points: u64, depth: u8, node_depth: u8) -> usize {
    let gap = u32::from(depth.saturating_sub(node_depth));
    let cells = if gap >= 21 {
        u64::MAX
    } else {
        1_u64 << (3 * gap)
    };
    (points.min(cells).div_ceil(PAGE_SLOTS as u64)).max(1) as usize
}

fn pages_of(row: &NodeRow, depths: std::ops::RangeInclusive<u8>) -> usize {
    depths
        .map(|d| estimate_pages_like_the_pool(row.points, d, row.node_depth))
        .sum()
}

fn histogram(values: impl Iterator<Item = i32>) -> std::collections::BTreeMap<i32, usize> {
    let mut map = std::collections::BTreeMap::new();
    for v in values {
        *map.entry(v).or_insert(0) += 1;
    }
    map
}

fn print_twin_rows(label: &str, rows: &[NodeRow]) {
    println!("== {label}");
    println!(
        "node  nd buildable eye samples excl_near excl_cross | twin [min,max] | range [lo,hi] | \
         lo-twin_min twin_max-hi | candidate [lo_c,hi_c] twin_min-lo_c hi_c-twin_max"
    );
    for r in rows {
        let slack = |a: Option<(u8, u8)>, b: Option<(u8, u8)>| -> String {
            match (a, b) {
                (Some(t), Some(x)) => format!(
                    "{:>3} {:>3}",
                    i32::from(x.0) - i32::from(t.0),
                    i32::from(t.1) - i32::from(x.1)
                ),
                _ => "  -   -".into(),
            }
        };
        let cand = match (r.twin, r.candidate) {
            (Some(t), Some(c)) => format!(
                "{:>3} {:>3}",
                i32::from(t.0) - i32::from(c.0),
                i32::from(c.1) - i32::from(t.1)
            ),
            _ => "  -   -".into(),
        };
        println!(
            "{:>4} {:>3} {:>6} {:>3} {:>7} {:>9} {:>10} | {:?} | {:?} wmax {:?}{} | {} | {:?} {}",
            r.id,
            r.node_depth,
            format!("{}..{}", r.span.0, r.span.1),
            if r.eye_inside { "EYE" } else { "" },
            r.samples,
            r.excluded_near,
            r.excluded_cross,
            r.twin,
            r.range,
            r.wanted_max,
            if r.capped { " CAPPED" } else { "" },
            slack(r.twin, r.range),
            r.candidate,
            cand
        );
        if let (Some(a), Some(b)) = (r.at_min, r.at_max) {
            println!(
                "      twin min first reached at view depth {:.3} screen px {:?}; twin max at view depth {:.3} screen px {:?}",
                a.0, a.1, b.0, b.1
            );
        }
    }
}

fn summarise_twin_rows(label: &str, rows: &[NodeRow]) {
    for (scope, keep) in [("all nodes", false), ("nodes holding the eye", true)] {
        let set: Vec<&NodeRow> = rows
            .iter()
            .filter(|r| r.twin.is_some() && r.range.is_some() && (!keep || r.eye_inside))
            .collect();
        let t = |r: &&NodeRow| r.twin.unwrap();
        let g = |r: &&NodeRow| r.range.unwrap();
        let lo_defect = set.iter().filter(|r| g(r).0 > t(r).0).count();
        let hi_defect = set.iter().filter(|r| g(r).1 < t(r).1).count();
        println!(
            "-- {label} / {scope}: {} nodes with samples and a range; lo > twin_min: {lo_defect}; \
             hi < twin_max: {hi_defect}",
            set.len()
        );
        println!(
            "   lo - twin_min {:?} | twin_max - hi {:?}",
            histogram(set.iter().map(|r| i32::from(g(r).0) - i32::from(t(r).0))),
            histogram(set.iter().map(|r| i32::from(t(r).1) - i32::from(g(r).1)))
        );
        let cs: Vec<&&NodeRow> = set.iter().filter(|r| r.candidate.is_some()).collect();
        let c = |r: &&&NodeRow| r.candidate.unwrap();
        let contained = cs
            .iter()
            .filter(|r| c(r).0 <= t(r).0 && c(r).1 >= t(r).1)
            .count();
        let hi_within_one = cs
            .iter()
            .filter(|r| (0..=1).contains(&(i32::from(c(r).1) - i32::from(t(r).1))))
            .count();
        println!(
            "   candidate: contains the twin range in {contained}/{}; hi_c - twin_max in 0..=1 in \
             {hi_within_one}/{}",
            cs.len(),
            cs.len()
        );
        println!(
            "   twin_min - lo_c {:?} | hi_c - twin_max {:?}",
            histogram(cs.iter().map(|r| i32::from(t(r).0) - i32::from(c(r).0))),
            histogram(cs.iter().map(|r| i32::from(c(r).1) - i32::from(t(r).1)))
        );
        // Pages: the depths a range holds beyond the twin's span, and the totals.
        let (mut twin_pages, mut today_pages, mut cand_pages, mut extra_pages) = (0, 0, 0, 0);
        for r in &cs {
            let (tl, th) = t(r);
            let (cl, ch) = c(r);
            twin_pages += pages_of(r, tl..=th);
            cand_pages += pages_of(r, cl.min(tl)..=ch.max(th));
            let (gl, gh) = g(r);
            today_pages += pages_of(r, gl.min(tl)..=gh.max(th));
            if cl < tl {
                extra_pages += pages_of(r, cl..=tl - 1);
            }
            if ch > th {
                extra_pages += pages_of(r, th + 1..=ch);
            }
        }
        println!(
            "   pages (upper-bound estimate, nodes with a candidate): twin span {twin_pages}; \
             today's range (union with the twin span) {today_pages}; candidate (union) \
             {cand_pages}; extra of the candidate slack {extra_pages}"
        );
    }
}

/// Diagnostic (prints, asserts nothing): per node of the applied cut at the poses P1-P5 of the
/// web demo on the built-in sample, the depth range the twin of the GPU rule wants over
/// node ∩ data bounds ∩ view frustum if every buildable depth were resident, against today's
/// `range`, and against the shipped candidate `node_view_range` (`lo_c`, `hi_c`: on-axis cells at
/// the farthest and nearest view depth of the region, the nearest with the px target divided by
/// the worst off-axis stretch of the frustum).
///
/// Sampling: a screen lattice of `LATTICE_PX` px (`TWIN_LATTICE_PX` overrides it) with a ray per
/// node; along each ray a point every `DEPTH_RATIO` in view depth plus both ends of the slab.
/// Excluded: points nearer than `lod_near_depth` (`excl_near`), and points whose twin cell at the
/// wanted depth crosses the eye plane (`excl_cross`, `edge_px == +inf`).
/// Run: `cargo test --offline --profile fast -p viewer_wasm --lib twin_wanted_range_vs_node_range -- --ignored --nocapture`.
#[test]
#[ignore = "diagnostic: prints the twin-wanted depth range per node against the node range"]
fn twin_wanted_range_vs_node_range() {
    let lattice = std::env::var("TWIN_LATTICE_PX")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(LATTICE_PX);
    let poses = std::env::var("TWIN_POSES").unwrap_or_else(|_| "P1,P2,P3,P4,P5".into());
    for pose in poses.split(',') {
        let (h, view) = settled_sample_pose(pose);
        let camera = view.camera();
        println!(
            "######## {pose}: eye {:.3?} target {:.3?} lattice {lattice} px, depth ratio {DEPTH_RATIO}, \
             lod_near_depth {:.4} (scaled)",
            camera.position(),
            camera.target(),
            lod_near_depth_scaled(&camera)
        );
        let rows = twin_rows(&h, &view, lattice);
        print_twin_rows(pose, &rows);
        summarise_twin_rows(pose, &rows);
    }
}

/// Violations of the node range property (ADR 0055, D2) at the poses `poses`, over node ∩ data ∩
/// frustum, excluding points nearer than `lod_near_depth` and cells that cross the eye plane:
/// `lo` side (`lo <= twin min` with a `PX_EPSILON` tolerance on the px target for f32 against f64,
/// and at most one level above `lo`) when `lo_side`, `hi` side (`hi >= twin max`, at most one level
/// below it) when not.
fn range_violations(poses: &[&str], lo_side: bool) -> Vec<String> {
    let mut violations = Vec::new();
    for pose in poses {
        let (h, view) = settled_sample_pose(pose);
        for r in twin_rows(&h, &view, LATTICE_PX) {
            let (Some((emin, emax)), Some((lo, hi)), Some((tmin, tmax))) =
                (r.twin_eps, r.range, r.twin)
            else {
                continue;
            };
            if lo_side && lo > emin {
                violations.push(format!("{pose} node {}: lo {lo} > twin min {emin}", r.id));
            }
            if !lo_side && hi < emax {
                violations.push(format!("{pose} node {}: hi {hi} < twin max {emax}", r.id));
            }
            if lo_side && tmin > lo + 1 {
                violations.push(format!(
                    "{pose} node {}: lo {lo} is >1 below twin min {tmin}",
                    r.id
                ));
            }
            if !lo_side && hi > tmax + 1 {
                violations.push(format!(
                    "{pose} node {}: hi {hi} is >1 above twin max {tmax}",
                    r.id
                ));
            }
        }
    }
    violations
}

fn assert_no_violations(violations: &[String]) {
    assert!(
        violations.is_empty(),
        "{} violations:\n{}",
        violations.len(),
        violations.join("\n")
    );
}

/// `lo` of the node range contains what the twin of the GPU rule wants, within one level (ADR 0055
/// plan, task 4: `lo` comes from `node_view_range`).
#[test]
#[ignore = "slow (about 3 minutes of twin sampling); P4 alone runs in the next test"]
fn the_node_lo_is_not_finer_than_what_the_twin_wants_within_one_level() {
    assert_no_violations(&range_violations(&["P1", "P2", "P3", "P4", "P5"], true));
}

/// The `lo` property at P4, where `node_far_depth` used to put node 14 three levels too fine.
#[test]
fn the_node_lo_at_p4_is_not_finer_than_what_the_twin_wants_within_one_level() {
    assert_no_violations(&range_violations(&["P4"], true));
}

/// `hi` of the node range contains what the twin wants, within one level. Paused: near the eye the
/// faithful `hi` is the finest level in empty air, a budget question (plan task 5).
#[test]
#[ignore = "paused: `hi` is not changed until plan task 5 decides what bounds it near the eye"]
fn the_node_hi_contains_what_the_twin_wants_within_one_level() {
    assert_no_violations(&range_violations(&["P1", "P2", "P3", "P4", "P5"], false));
}

// ---- OQ-16: fine end of the node range bounded by the nearest occupied cell -----------------

/// What the occupied cells of the finest resident level of a node say (see `data_bound`).
#[derive(Clone, Debug, Default)]
struct DataBound {
    /// Finest resident level of the node at rest, and its cells inside the frustum.
    finest: Option<u8>,
    in_view: usize,
    /// Nearest view depth (scaled) of a nearest cell face.
    nearest_occupied: Option<f64>,
    /// Twin maximum at the centres of those cells, and at the point of each cell nearest the eye.
    data_max: Option<u8>,
    cell_max: Option<u8>,
    /// The candidate's `hi` with the nearest view depth replaced by `max(near, nearest_occupied)`.
    hi_d: Option<u8>,
    /// `hi_c` recomputed here (validates the restatement against `node_view_range`).
    hi_c_restated: Option<u8>,
    /// Pages of the node's levels that the pool holds now.
    resident_pages: usize,
}

/// `ViewSpan::on_axis` (`cell_cut.rs`, private) restated: the first buildable depth whose
/// smallest child, for a cell on the view axis with its nearest face at view depth `near`
/// (dataset units, eye-relative frame), projects below `target_px`, capped.
#[expect(
    clippy::too_many_arguments,
    reason = "the parameters are independent inputs of one computation; a parameter struct would only rename them"
)]
fn on_axis_depth(
    view: &ViewProjection,
    grid: &CellGrid,
    eye: [f64; 3],
    forward: [f64; 3],
    buildable: (u8, u8),
    cap: u8,
    near: f64,
    target_px: f64,
) -> u8 {
    let meets = |depth: u8| {
        let side = grid.root_side / 2.0_f64.powi(i32::from(depth));
        let extent: f64 = forward.iter().map(|v| v.abs()).sum::<f64>() * side;
        let min: [f64; 3] =
            std::array::from_fn(|a| eye[a] + forward[a] * (near + extent / 2.0) - side / 2.0);
        let half = side / 2.0;
        let smallest = (0..8_usize)
            .map(|child| {
                let lo: [f64; 3] =
                    std::array::from_fn(|a| min[a] + half * ((child >> a & 1) as f64));
                SpatialBounds::new(lo, lo.map(|v| v + half))
                    .map_or(f64::NAN, |c| view.projected_voxel_edge_pixels(c, [0.0; 3]))
            })
            .fold(
                f64::INFINITY,
                |acc, edge| if edge < acc { edge } else { acc },
            );
        smallest < target_px
    };
    (buildable.0..=buildable.1)
        .find(|d| meets(*d))
        .unwrap_or(buildable.1)
        .min(cap)
}

/// Occupied-cell measurement of node `j` of the cut (see the section header and the test below).
fn data_bound(
    h: &Harness,
    view: &ViewProjection,
    j: usize,
    row: &NodeRow,
    with_twin: bool,
) -> DataBound {
    let ctx = h.cell_ctx.as_ref().expect("cell mode context");
    let node = &h.cell_nodes[j];
    let (grid, scale) = (h.dataset.cell_grid(), ctx.scale);
    let camera = view.camera();
    let near = lod_near_depth_scaled(&camera);
    let key = |depth: u8| viewer_render_wgpu::LevelKey {
        session: 0,
        node: node.id,
        depth,
        color_gen: 0,
    };
    let mut out = DataBound::default();
    // Every level the pool holds data for, not only the drawn ones: a resident level that no
    // octant draws still occupies pages.
    let levels: Vec<(u8, &PackedLevel)> = (0..=MAX_CELL_DEPTH)
        .filter_map(|d| Some((d, h.driver.pool().level(key(d))?)))
        .collect();
    out.resident_pages = levels.iter().map(|(_, l)| l.pages.len()).sum();
    let Some(&(finest, level)) = levels.iter().max_by_key(|(d, _)| *d) else {
        return out;
    };
    out.finest = Some(finest);
    let f64v = |v: [f32; 3]| v.map(f64::from);
    let forward = f64v(ctx.lod.forward);
    let spread: f64 = forward.iter().map(|v| v.abs()).sum();
    let targets = [ctx.lod.target_px; 3];
    let side = grid.root_side / 2.0_f64.powi(i32::from(finest)) * scale;
    let mut nearest: Option<f64> = None;
    let (mut data_max, mut cell_max): (Option<u8>, Option<u8>) = (None, None);
    for (p, page) in level.pages.iter().enumerate() {
        for i in 0..page.cells as usize {
            let instance = level.instances[p * PAGE_SLOTS + i];
            if instance.is_empty() {
                continue;
            }
            let min: [f64; 3] = std::array::from_fn(|a| {
                let coordinate = page.origin[a] + u64::from(instance.rel()[a]);
                (grid.root_min[a] - ctx.origin[a]) * scale - ctx.eye[a] + coordinate as f64 * side
            });
            let centre: [f64; 3] = min.map(|v| v + side / 2.0);
            let depth: f64 = (0..3).map(|a| centre[a] * forward[a]).sum();
            if depth < near || !ctx.on_screen(centre) {
                continue;
            }
            if with_twin {
                let (found, crosses) = twin_depths(&ctx.lod, node, centre, row.span, targets);
                if crosses {
                    continue;
                }
                data_max = data_max.max(Some(found[0]));
            }
            out.in_view += 1;
            let face = depth - side / 2.0 * spread;
            nearest = Some(nearest.map_or(face, |n| n.min(face)));
            // The point of the cell nearest the eye (the eye is the origin of this frame).
            let q: [f64; 3] = std::array::from_fn(|a| 0.0_f64.clamp(min[a], min[a] + side));
            let q_depth: f64 = q.iter().zip(&forward).map(|(a, b)| a * b).sum();
            if with_twin && q_depth >= near && ctx.on_screen(q) {
                let (f, c) = twin_depths(&ctx.lod, node, q, row.span, targets);
                if !c {
                    cell_max = cell_max.max(Some(f[0]));
                }
            }
        }
    }
    out.nearest_occupied = nearest;
    out.data_max = data_max;
    out.cell_max = cell_max;
    // The candidate's inputs, as `twin_rows` and `ViewSpan` derive them.
    let data = h.dataset.metadata.hierarchy().stats().data_bounds;
    let (Some((lo_c, hi_c)), Some(nearest)) = (row.candidate, nearest) else {
        return out;
    };
    let eye_ds: [f64; 3] = camera.position().map(|v| v / scale);
    let target = camera.target();
    let fwd = unit(std::array::from_fn(|a| target[a] - camera.position()[a]));
    let min_side = h.driver.pool().min_side();
    let cap = min_side.map_or(row.span.1, |s| {
        representative_depth_for_spacing(&grid, s).clamp(row.span.0, row.span.1)
    });
    let [width, height] = view.viewport();
    let aspect = f64::from(width) / f64::from(height.max(1));
    let tan_corner = (45.0_f64.to_radians() * 0.5).tan() * (1.0 + aspect * aspect).sqrt();
    let stretch = (1.0 + tan_corner * tan_corner).sqrt();
    let target_px = f64::from(ctx.lod.target_px);
    let hi_at = |near_ds: f64| {
        on_axis_depth(
            view,
            &grid,
            eye_ds,
            fwd,
            row.span,
            cap,
            near_ds,
            target_px / stretch,
        )
        .max(lo_c)
    };
    out.hi_d = Some(hi_at(nearest.max(near) / scale));
    // Restate `hi_c`: nearest depth = max(near, distance to the region / stretch).
    let region_lo: [f64; 3] =
        std::array::from_fn(|a| node.min[a].max(data.min()[a]) - ctx.origin[a]);
    let region_hi: [f64; 3] =
        std::array::from_fn(|a| node.max[a].min(data.max()[a]) - ctx.origin[a]);
    let distance = (0..3)
        .map(|a| (eye_ds[a].clamp(region_lo[a], region_hi[a]) - eye_ds[a]).powi(2))
        .sum::<f64>()
        .sqrt();
    out.hi_c_restated = Some(hi_at((near / scale).max(distance / stretch)));
    debug_assert!(hi_c >= lo_c);
    out
}

// ---- Task 5a: cheaper sources of the nearest occupied view depth (no wiring) -----------------

/// Axis-aligned box in the eye-relative scaled frame of `CellCtx`.
type Box3 = ([f64; 3], [f64; 3]);

/// Columns of the source comparison: the cell-by-cell reference, then each source of the nearest
/// occupied depth (a: page boxes of the finest resident level; b: of the finest resident level not
/// finer than the reference `hi_d`; c: of the coarsest resident level; d: `octant_data_bounds`),
/// unclipped (`raw`) and clipped to the frustum (`clip`).
const SOURCES: [&str; 9] = [
    "ref", "a.raw", "a.clip", "b.raw", "b.clip", "c.raw", "c.clip", "d.raw", "d.clip",
];

fn dot3(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|i| a[i] * b[i]).sum()
}

/// The half-spaces `n . p <= c` (unit `n`) of `CellCtx::on_screen` plus the LOD near depth.
fn frustum_planes(ctx: &CellCtx, near: f64) -> Vec<([f64; 3], f64)> {
    let f = ctx.lod.forward.map(f64::from);
    let (r, u) = (ctx.lod.right.map(f64::from), ctx.lod.up.map(f64::from));
    let focal = f64::from(ctx.lod.focal);
    let mut planes = vec![(f.map(|v| -v), -ctx.clip.0.max(near)), (f, ctx.clip.1)];
    for (side, half) in [(r, ctx.viewport[0] * 0.5), (u, ctx.viewport[1] * 0.5)] {
        for sign in [1.0, -1.0] {
            // sign * side . p * focal <= depth * half
            let n: [f64; 3] = std::array::from_fn(|a| sign * focal * side[a] - half * f[a]);
            let len = dot3(n, n).sqrt();
            planes.push((n.map(|v| v / len), 0.0));
        }
    }
    planes
}

/// Smallest view depth (along `forward`) of the points of `bx` inside every half-space of
/// `planes`, or `None` when the intersection is empty. Exact up to the feasibility tolerance
/// (1e-9 relative), which only admits points marginally outside, so the depth can only come out
/// lower: the error is on the conservative side. Method: the minimum of a linear function over the
/// convex polytope `box ∩ frustum` sits at a vertex, so every triple of planes is intersected and
/// the feasible intersections are kept (12 planes, 220 triples; box fully inside and box fully
/// outside one plane are shortcuts).
fn min_depth_in_box(bx: &Box3, planes: &[([f64; 3], f64)], forward: [f64; 3]) -> Option<f64> {
    let tol = |p: [f64; 3]| 1e-9 * (1.0 + p.iter().fold(0.0_f64, |m, v| m.max(v.abs())));
    let corners: Vec<[f64; 3]> = (0..8_usize)
        .map(|k| std::array::from_fn(|a| if k >> a & 1 == 1 { bx.1[a] } else { bx.0[a] }))
        .collect();
    let feasible =
        |p: [f64; 3], set: &[([f64; 3], f64)]| set.iter().all(|(n, c)| dot3(*n, p) <= c + tol(p));
    if corners.iter().all(|p| feasible(*p, planes)) {
        return corners.iter().map(|p| dot3(*p, forward)).reduce(f64::min);
    }
    if planes
        .iter()
        .any(|(n, c)| corners.iter().all(|p| dot3(*n, *p) > c + tol(*p)))
    {
        return None;
    }
    let mut all = planes.to_vec();
    for a in 0..3 {
        let unit: [f64; 3] = std::array::from_fn(|i| f64::from(u8::from(i == a)));
        all.push((unit.map(|v| -v), -bx.0[a]));
        all.push((unit, bx.1[a]));
    }
    let cross = |u: [f64; 3], v: [f64; 3]| {
        [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ]
    };
    let mut best: Option<f64> = None;
    for i in 0..all.len() {
        for j in i + 1..all.len() {
            for k in j + 1..all.len() {
                let ((n1, c1), (n2, c2), (n3, c3)) = (all[i], all[j], all[k]);
                let det = dot3(n1, cross(n2, n3));
                if det.abs() < 1e-9 {
                    continue;
                }
                let (x, y, z) = (cross(n2, n3), cross(n3, n1), cross(n1, n2));
                let p: [f64; 3] =
                    std::array::from_fn(|a| (c1 * x[a] + c2 * y[a] + c3 * z[a]) / det);
                if feasible(p, &all) {
                    let depth = dot3(p, forward);
                    best = Some(best.map_or(depth, |b| b.min(depth)));
                }
            }
        }
    }
    best
}

/// `hi` of `node_view_range` as a function of the nearest view depth (scaled units), restating the
/// inputs `data_bound` derives (see there).
struct HiCalc<'a> {
    view: &'a ViewProjection,
    grid: CellGrid,
    eye_ds: [f64; 3],
    forward: [f64; 3],
    span: (u8, u8),
    cap: u8,
    target_px: f64,
    lo_c: u8,
    scale: f64,
    stretch: f64,
    near: f64,
}

impl<'a> HiCalc<'a> {
    fn new(h: &Harness, view: &'a ViewProjection, row: &NodeRow) -> Option<Self> {
        let ctx = h.cell_ctx.as_ref()?;
        let (lo_c, _) = row.candidate?;
        let grid = h.dataset.cell_grid();
        let camera = view.camera();
        let forward = unit(std::array::from_fn(|a| {
            camera.target()[a] - camera.position()[a]
        }));
        let cap = h.driver.pool().min_side().map_or(row.span.1, |s| {
            representative_depth_for_spacing(&grid, s).clamp(row.span.0, row.span.1)
        });
        let [width, height] = view.viewport();
        let aspect = f64::from(width) / f64::from(height.max(1));
        let tan_corner = (45.0_f64.to_radians() * 0.5).tan() * (1.0 + aspect * aspect).sqrt();
        Some(Self {
            view,
            grid,
            eye_ds: camera.position().map(|v| v / ctx.scale),
            forward,
            span: row.span,
            cap,
            target_px: f64::from(ctx.lod.target_px),
            lo_c,
            scale: ctx.scale,
            stretch: (1.0 + tan_corner * tan_corner).sqrt(),
            near: lod_near_depth_scaled(&camera),
        })
    }

    /// `hi` for a nearest view depth `nearest` (scaled units), not nearer than the LOD near depth.
    fn hi(&self, nearest: f64) -> u8 {
        on_axis_depth(
            self.view,
            &self.grid,
            self.eye_ds,
            self.forward,
            self.span,
            self.cap,
            nearest.max(self.near) / self.scale,
            self.target_px / self.stretch,
        )
        .max(self.lo_c)
    }
}

/// Every level the pool holds for node `id` (drawn or not), by depth.
fn resident_levels(h: &Harness, id: NodeId) -> Vec<(u8, &PackedLevel)> {
    (0..=MAX_CELL_DEPTH)
        .filter_map(|depth| {
            let key = viewer_render_wgpu::LevelKey {
                session: 0,
                node: id,
                depth,
                color_gen: 0,
            };
            Some((depth, h.driver.pool().level(key)?))
        })
        .collect()
}

/// Tight boxes of the pages of a level (eye-relative scaled frame).
fn page_boxes(ctx: &CellCtx, grid: &CellGrid, depth: u8, level: &PackedLevel) -> Vec<Box3> {
    let side = grid.root_side / 2.0_f64.powi(i32::from(depth)) * ctx.scale;
    level
        .pages
        .iter()
        .map(|page| {
            let min: [f64; 3] = std::array::from_fn(|a| {
                (grid.root_min[a] - ctx.origin[a]) * ctx.scale - ctx.eye[a]
                    + page.origin[a] as f64 * side
            });
            let max = std::array::from_fn(|a| min[a] + (f64::from(page.box_max[a]) + 1.0) * side);
            (min, max)
        })
        .collect()
}

/// `octant_data_bounds` (crate-private in `viewer_render_wgpu`) restated: the octants of the node
/// box intersected with the data extent, in the eye-relative scaled frame.
fn octant_boxes(h: &Harness, j: usize) -> Vec<Box3> {
    let ctx = h.cell_ctx.as_ref().expect("cell mode context");
    let node = &h.cell_nodes[j];
    let data = h.dataset.metadata.hierarchy().stats().data_bounds;
    let to_eye = |a: usize, v: f64| (v - ctx.origin[a]) * ctx.scale - ctx.eye[a];
    (0..8_usize)
        .filter_map(|octant| {
            let (mut min, mut max) = ([0.0; 3], [0.0; 3]);
            for a in 0..3 {
                let mid = f64::midpoint(node.min[a], node.max[a]);
                let (g_min, g_max) = if octant >> (2 - a) & 1 == 1 {
                    (mid, node.max[a])
                } else {
                    (node.min[a], mid)
                };
                let (lo, hi) = (g_min.max(data.min()[a]), g_max.min(data.max()[a]));
                if lo > hi {
                    return None;
                }
                (min[a], max[a]) = (to_eye(a, lo), to_eye(a, hi));
            }
            Some((min, max))
        })
        .collect()
}

/// `hi` of each source of `SOURCES` for node `j` (`None` where the candidate or the geometry in
/// view is missing). `max_resident` pretends the levels finer than it are not resident (the
/// stability probe); `ref` is `d.hi_d`, not recomputed.
fn source_his(
    h: &Harness,
    view: &ViewProjection,
    j: usize,
    row: &NodeRow,
    d: &DataBound,
    max_resident: Option<u8>,
) -> [Option<u8>; 9] {
    source_detail(h, view, j, row, d, max_resident).0
}

/// What `source_his` also reports for the diagnostics: the nearest view depth each source gave
/// (before the LOD near clamp) and, for `a.clip`, the page that gave it as (cells, box extent per
/// axis in scaled units).
type PageWitness = Option<(u32, [f64; 3])>;

fn source_detail(
    h: &Harness,
    view: &ViewProjection,
    j: usize,
    row: &NodeRow,
    d: &DataBound,
    max_resident: Option<u8>,
) -> ([Option<u8>; 9], [Option<f64>; 9], PageWitness) {
    let mut out = [None; 9];
    let mut nears = [None; 9];
    let mut witness = None;
    out[0] = d.hi_d;
    nears[0] = d.nearest_occupied;
    let (Some(calc), Some(ctx)) = (HiCalc::new(h, view, row), h.cell_ctx.as_ref()) else {
        return (out, nears, witness);
    };
    let levels: Vec<(u8, &PackedLevel)> = resident_levels(h, h.cell_nodes[j].id)
        .into_iter()
        .filter(|(depth, _)| max_resident.is_none_or(|m| *depth <= m))
        .collect();
    let grid = h.dataset.cell_grid();
    let boxes_of = |level: Option<&(u8, &PackedLevel)>| {
        level.map(|(depth, l)| page_boxes(ctx, &grid, *depth, l))
    };
    let cap_b = d.hi_d.unwrap_or(u8::MAX);
    let set_b = levels
        .iter()
        .filter(|(depth, _)| *depth <= cap_b)
        .max_by_key(|(depth, _)| *depth)
        .or_else(|| levels.iter().min_by_key(|(depth, _)| *depth));
    let chosen = [
        levels.iter().max_by_key(|(depth, _)| *depth),
        set_b,
        levels.iter().min_by_key(|(depth, _)| *depth),
    ];
    let sets = [
        boxes_of(chosen[0]),
        boxes_of(chosen[1]),
        boxes_of(chosen[2]),
        Some(octant_boxes(h, j)),
    ];
    let planes = frustum_planes(ctx, calc.near);
    let forward = ctx.lod.forward.map(f64::from);
    for (s, set) in sets.iter().enumerate() {
        let Some(set) = set.as_ref().filter(|b| !b.is_empty()) else {
            continue;
        };
        // raw: the distance to the box over the off-axis stretch, as `nearest_visible_depth` does
        // for the region; clip: the smallest view depth of the box inside the frustum (the
        // render crate's `nearest_occupied_page` for the pages of a level; the octant boxes of
        // the last source have no pages, so they keep the local clip).
        let raw = set
            .iter()
            .map(|(min, max)| {
                let distance = (0..3)
                    .map(|a| 0.0_f64.clamp(min[a], max[a]).powi(2))
                    .sum::<f64>()
                    .sqrt();
                distance / calc.stretch
            })
            .fold(f64::INFINITY, f64::min);
        let (clip, at) = match chosen.get(s).copied().flatten() {
            Some((depth, level)) => viewer_render_wgpu::nearest_occupied_page(
                view,
                &grid,
                h.dataset.coordinate_origin(),
                *depth,
                &level.pages,
            )
            .map_or((None, 0), |(depth, at)| (Some(depth * ctx.scale), at)),
            None => set
                .iter()
                .enumerate()
                .filter_map(|(i, bx)| Some((min_depth_in_box(bx, &planes, forward)?, i)))
                .fold((None, 0), |(best, at), (depth, i)| match best {
                    Some(b) if b <= depth => (best, at),
                    _ => (Some(depth), i),
                }),
        };
        if s == 0
            && clip.is_some()
            && let Some((_, level)) = levels.iter().max_by_key(|(depth, _)| *depth)
        {
            let bx = set[at];
            witness = Some((
                level.pages[at].cells,
                std::array::from_fn(|a| bx.1[a] - bx.0[a]),
            ));
        }
        out[1 + 2 * s] = Some(calc.hi(raw));
        out[2 + 2 * s] = clip.map(|c| calc.hi(c));
        (nears[1 + 2 * s], nears[2 + 2 * s]) = (Some(raw), clip);
    }
    (out, nears, witness)
}

/// Per-source tallies of the comparison with the reference and with `twin_data_max`.
#[derive(Default)]
struct SourceStats {
    violations: Vec<String>,
    /// `hi(source) - hi(ref)` and `hi(source) - twin_data_max`, over all nodes and the eye nodes.
    vs_ref: [Vec<i32>; 2],
    vs_tdm: [Vec<i32>; 2],
    at_hi_c: [usize; 2],
    nodes: [usize; 2],
}

impl SourceStats {
    #[expect(
        clippy::too_many_arguments,
        reason = "the parameters are independent inputs of one computation; a parameter struct would only rename them"
    )]
    fn add(
        &mut self,
        pose: &str,
        id: u64,
        eye: bool,
        hi: Option<u8>,
        reference: Option<u8>,
        tdm: u8,
        hi_c: u8,
    ) {
        let Some(hi) = hi else {
            self.violations
                .push(format!("{pose} node {id}: no hi (no geometry in view)"));
            return;
        };
        if hi < tdm {
            self.violations
                .push(format!("{pose} node {id}: hi {hi} < twin_data_max {tdm}"));
        }
        for group in std::iter::once(0).chain(eye.then_some(1)) {
            self.vs_tdm[group].push(i32::from(hi) - i32::from(tdm));
            if let Some(r) = reference {
                self.vs_ref[group].push(i32::from(hi) - i32::from(r));
            }
            self.at_hi_c[group] += usize::from(hi >= hi_c);
            self.nodes[group] += 1;
        }
    }

    fn print(&self, name: &str) {
        let hist = |v: &[i32]| format!("{:?}", histogram(v.iter().copied()));
        println!(
            "== SOURCE {name}: node-poses {} (eye {}); violations {}; at hi_c {} (eye {})\n   \
             hi-ref all {} | eye {}\n   hi-tdm all {} | eye {}",
            self.nodes[0],
            self.nodes[1],
            self.violations.len(),
            self.at_hi_c[0],
            self.at_hi_c[1],
            hist(&self.vs_ref[0]),
            hist(&self.vs_ref[1]),
            hist(&self.vs_tdm[0]),
            hist(&self.vs_tdm[1]),
        );
        for v in &self.violations {
            println!("   VIOLATION {v}");
        }
    }
}

/// Oscillation risk of the sources that depend on residency (a, b), for an eye node: the bound
/// with the levels resident today against the bound with the levels finer than the reference
/// `hi_d` filtered out, and the sequence `ceiling -> hi(source of the levels <= ceiling)` from
/// the reference `hi_d` (a cycle between two values means the bound and the residency chase each
/// other).
fn print_stability_probe(
    h: &Harness,
    view: &ViewProjection,
    j: usize,
    row: &NodeRow,
    d: &DataBound,
    pose: &str,
) {
    let show = |v: Option<u8>| v.map_or("-".to_string(), |x| x.to_string());
    let today = source_his(h, view, j, row, d, None);
    let Some(ref_hi) = d.hi_d else { return };
    let filtered = source_his(h, view, j, row, d, Some(ref_hi));
    println!(
        "PROBE {pose} node {} EYE finest resident {} ref hi_d {ref_hi}: today a.raw {} a.clip {} b.raw {} b.clip {} | levels <= ref hi_d only: a.raw {} a.clip {} b.raw {} b.clip {}",
        row.id,
        show(d.finest),
        show(today[1]),
        show(today[2]),
        show(today[3]),
        show(today[4]),
        show(filtered[1]),
        show(filtered[2]),
        show(filtered[3]),
        show(filtered[4]),
    );
    // Residency as if the bound were wired and only today's range were resident: levels up to
    // today's `hi` and up to today's `lo`.
    if let Some((lo, hi_now)) = row.range {
        for (label, cap) in [("<= today hi", hi_now), ("<= today lo", lo)] {
            let f = source_his(h, view, j, row, d, Some(cap));
            println!(
                "PROBE {pose} node {} levels {label} ({cap}): a.raw {} a.clip {} b.raw {} b.clip {}",
                row.id,
                show(f[1]),
                show(f[2]),
                show(f[3]),
                show(f[4])
            );
        }
    }
    for (slot, name) in [(1, "a.raw"), (2, "a.clip")] {
        let mut ceiling = ref_hi;
        let mut seq = vec![ceiling];
        for _ in 0..6 {
            let Some(next) = source_his(h, view, j, row, d, Some(ceiling))[slot] else {
                break;
            };
            seq.push(next);
            ceiling = next;
        }
        println!(
            "PROBE {pose} node {} {name} ceiling sequence {seq:?}",
            row.id
        );
    }
}

/// Diagnostic (prints, asserts nothing), OQ-16: bound the fine end of the node range by the data.
/// `hi_d` = the candidate's `hi` with the nearest view depth replaced by `max(lod_near_depth,
/// nearest face of an occupied cell of the node's finest resident level inside the frustum)`.
/// Compared per node with `twin_data_max` (the twin's maximum at the occupied cells: centres, and
/// the point of each cell nearest the eye), today's `hi`, `hi_c`, and in estimated pages
/// (`estimate_pages_like_the_pool`) of the run `lo..=X` for X in {today's hi, `twin_data_max`,
/// `hi_d`, `hi_c`}, with `lo` today's `lo`. Also prints the pages the pool holds for the node.
/// Run: `TWIN_POSES=P2,P4 cargo test --offline --profile fast -p viewer_wasm --lib data_bounded_hi -- --ignored --nocapture`.
#[test]
#[ignore = "diagnostic: prints a data-bounded fine end of the node range against the others"]
fn data_bounded_hi_vs_node_range() {
    let lattice = std::env::var("TWIN_LATTICE_PX")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(LATTICE_PX);
    let poses = std::env::var("TWIN_POSES").unwrap_or_else(|_| "P1,P2,P3,P4,P5".into());
    let (mut slack_d, mut d_vs_c, mut violations) = (Vec::new(), Vec::new(), Vec::new());
    let mut stats: Vec<SourceStats> = (0..SOURCES.len()).map(|_| SourceStats::default()).collect();
    for pose in poses.split(',') {
        let (h, view) = settled_sample_pose(pose);
        let rows = twin_rows(&h, &view, lattice);
        let pool = h.driver.pool().resident_pages();
        println!("######## {pose}: pool resident pages {pool}");
        println!(
            "node EYE fin cells | hi_now wmax twin_max data_max cell_max hi_c hi_d (hi_c rst) | \
             hi_d-dmax hi_d-cmax hi_c-hi_d | near_occ | pages now/dmax/hi_d/hi_c | resident"
        );
        let mut tot = [0_usize; 5];
        let mut eye_tot = [0_usize; 5];
        let mut src_pages = [[0_usize; 2]; 9];
        for (j, row) in rows.iter().enumerate() {
            let d = data_bound(&h, &view, j, row, true);
            let show = |v: Option<u8>| v.map_or("-".to_string(), |x| x.to_string());
            let (Some((lo, hi)), Some((_, hi_c)), Some(hi_d), Some(dmax)) =
                (row.range, row.candidate, d.hi_d, d.data_max)
            else {
                println!(
                    "{:>4} {:>3} {:>3} {:>5} | no data in view or no range/candidate | resident {}",
                    row.id,
                    if row.eye_inside { "EYE" } else { "" },
                    show(d.finest),
                    d.in_view,
                    d.resident_pages
                );
                continue;
            };
            let cmax = d.cell_max.unwrap_or(dmax);
            let run = |x: u8| pages_of(row, lo.min(x)..=x);
            let cols = [run(hi), run(dmax), run(hi_d), run(hi_c), d.resident_pages];
            for (t, c) in tot.iter_mut().zip(cols) {
                *t += c;
            }
            if row.eye_inside {
                for (t, c) in eye_tot.iter_mut().zip(cols) {
                    *t += c;
                }
            }
            let (sd, sc) = (
                i32::from(hi_d) - i32::from(dmax),
                i32::from(hi_d) - i32::from(cmax),
            );
            slack_d.push(sd.min(sc));
            d_vs_c.push(i32::from(hi_c) - i32::from(hi_d));
            if sd < 0 || sc < 0 {
                violations.push(format!(
                    "{pose} node {}: hi_d {hi_d} < data_max {dmax} / cell_max {cmax}",
                    row.id
                ));
            }
            println!(
                "{:>4} {:>3} {:>3} {:>5} | {hi:>3} {:>4} {} {dmax:>8} {cmax:>8} {hi_c:>4} {hi_d:>4} ({}) | \
                 {sd:>3} {sc:>3} {:>3} | {:.4} | {}/{}/{}/{} | {}",
                row.id,
                if row.eye_inside { "EYE" } else { "" },
                show(d.finest),
                d.in_view,
                show(row.wanted_max),
                show(row.twin.map(|t| t.1)),
                show(d.hi_c_restated),
                i32::from(hi_c) - i32::from(hi_d),
                d.nearest_occupied.unwrap_or(f64::NAN),
                cols[0],
                cols[1],
                cols[2],
                cols[3],
                cols[4],
            );
            // Sources of the nearest occupied depth (Task 5a).
            let tdm = dmax.max(cmax);
            let his = source_his(&h, &view, j, row, &d, None);
            let mut line = format!(
                "SRC {pose} node {:>3}{} lo {lo} hi_now {hi} hi_c {hi_c} tdm {tdm} |",
                row.id,
                if row.eye_inside { " EYE" } else { "" }
            );
            for (s, hi_s) in his.iter().enumerate() {
                let pages = hi_s.map_or(0, |x| pages_of(row, lo.min(x)..=x));
                let _ = write!(line, " {} {} ({pages}p)", SOURCES[s], show(*hi_s));
                src_pages[s][0] += pages;
                // The nodes next to the eye of the sample: the two that hold or touch it.
                let near_eye = row.eye_inside || row.id == 14 || row.id == 17;
                if near_eye {
                    src_pages[s][1] += pages;
                }
                stats[s].add(pose, row.id, near_eye, *hi_s, his[0], tdm, hi_c);
            }
            println!("{line}");
            if row.eye_inside || row.id == 14 || row.id == 17 {
                let (_, nears, witness) = source_detail(&h, &view, j, row, &d, None);
                let depths: Vec<u8> = resident_levels(&h, h.cell_nodes[j].id)
                    .iter()
                    .map(|(depth, _)| *depth)
                    .collect();
                println!(
                    "NEAR {pose} node {} resident depths {depths:?} nearest view depth (scaled) {:?}; a.clip page (cells, box extent) {witness:?}",
                    row.id,
                    nears.map(|n| n.map(|v| (v * 1000.0).round() / 1000.0))
                );
                print_stability_probe(&h, &view, j, row, &d, pose);
            }
        }
        println!("-- {pose} SRC pages (all, nodes 14/17/EYE) per source: {src_pages:?}");
        println!(
            "-- {pose} totals (pages now/data_max/hi_d/hi_c/resident): all {tot:?}; nodes holding the eye {eye_tot:?}; pool resident {pool}"
        );
    }
    println!(
        "== hi_d - max(data_max, cell_max): {:?}",
        histogram(slack_d.into_iter())
    );
    println!("== hi_c - hi_d: {:?}", histogram(d_vs_c.into_iter()));
    for (s, st) in stats.iter().enumerate() {
        st.print(SOURCES[s]);
    }
    println!(
        "== violations ({}):\n{}",
        violations.len(),
        violations.join("\n")
    );
}

/// OQ-16 on a real cut (`cells_bench` with `BENCH_HI_D=1`): per node of the settled cut, today's
/// `hi`, `hi_c`, `hi_d` (no twin sampling: only the nearest occupied face of the finest resident
/// level) and the estimated pages of the run `lo..=X`; totals against the pages the pool holds.
fn print_data_bound_totals(h: &Harness, view: &ViewProjection) {
    let ctx = h.cell_ctx.as_ref().expect("cell mode context");
    let hierarchy = h.dataset.metadata.hierarchy();
    let data = hierarchy.stats().data_bounds;
    let (mut tot, mut skipped, mut worst) = ([0_usize; 4], 0, (0_usize, 0_u64));
    let (mut src_pages, mut src_none) = ([0_usize; 9], [0_usize; 9]);
    // Resident levels finer than the node's `hi_d`, and their pages (task 7a: what the cap frees).
    let (mut over_nodes, mut over_levels, mut over_pages) = (0_usize, 0_usize, 0_usize);
    for (j, node) in h.cell_nodes.iter().enumerate() {
        let region = SpatialBounds::new(
            std::array::from_fn(|a| node.min[a].max(data.min()[a])),
            std::array::from_fn(|a| node.max[a].min(data.max()[a])),
        )
        .ok();
        let row = NodeRow {
            id: node.id.get(),
            node_depth: node.lod.node_depth,
            span: buildable_span(h, node.id).expect("blocks of the cut are cached"),
            points: hierarchy.node(node.id).unwrap().point_count(),
            range: h.last_levels[j].2,
            candidate: node_view_range(
                view,
                &h.dataset.cell_grid(),
                h.dataset.coordinate_origin(),
                region,
                buildable_span(h, node.id).unwrap(),
                f64::from(ctx.lod.target_px),
                h.driver.pool().min_side(),
            ),
            ..NodeRow::default()
        };
        let d = data_bound(h, view, j, &row, false);
        let (Some((lo, hi)), Some((_, hi_c)), Some(hi_d)) = (row.range, row.candidate, d.hi_d)
        else {
            skipped += 1;
            tot[3] += d.resident_pages;
            continue;
        };
        let finer: Vec<usize> = pages_by_depth(h, node.id)
            .into_iter()
            .filter(|(depth, _)| *depth > hi_d)
            .map(|(_, pages)| pages)
            .collect();
        over_nodes += usize::from(!finer.is_empty());
        over_levels += finer.len();
        over_pages += finer.iter().sum::<usize>();
        let run = |x: u8| pages_of(&row, lo.min(x)..=x);
        for (s, his) in source_his(h, view, j, &row, &d, None).iter().enumerate() {
            src_none[s] += usize::from(his.is_none());
            src_pages[s] += run(his.unwrap_or(lo));
        }
        for (t, c) in tot
            .iter_mut()
            .zip([run(hi), run(hi_d), run(hi_c), d.resident_pages])
        {
            *t += c;
        }
        if run(hi_c) > worst.0 {
            worst = (run(hi_c), row.id);
        }
    }
    println!(
        "BENCH_HI_D nodes {} (without range/candidate/data in view: {skipped}) pages est today's hi {} | hi_d {} | hi_c {} | resident (pool, all nodes of the cut) {} | pool counter {} | biggest hi_c run: node {} {} pages",
        h.cell_nodes.len(),
        tot[0],
        tot[1],
        tot[2],
        tot[3],
        h.driver.pool().resident_pages(),
        worst.1,
        worst.0
    );
    println!(
        "BENCH_OVER_BOUND nodes with a bound {} | nodes holding levels finer than hi_d {over_nodes} | levels finer than hi_d {over_levels} | pages of those levels {over_pages} | resident pages (pool) {}",
        h.cell_nodes.len() - skipped,
        h.driver.pool().resident_pages()
    );
    // Estimated pages of the runs `lo..=hi(source)` (a source without geometry in view counts the
    // run `lo..=lo`), per source, with the number of such nodes.
    let line: Vec<String> = SOURCES
        .iter()
        .enumerate()
        .map(|(s, name)| format!("{name} {} (none {})", src_pages[s], src_none[s]))
        .collect();
    println!("BENCH_SRC est pages: {}", line.join(" | "));
}

// ---------------------------------------------------------------------------------------------
// ADR 0055 plan, task 7a: what the eye node holds and how late detail arrives when zooming in
// (ignored diagnostics; the output formats are stable so a later run can be diffed against them)
// ---------------------------------------------------------------------------------------------

impl Harness {
    /// A frame followed by what the browser does with the driver's level requests: the set goes
    /// to the build side, which builds what it can on the next frame. `settle` does not relay
    /// them (the older diagnostics measure the levels the plan alone builds).
    fn frame_relayed(&mut self, view: &ViewProjection) -> FrameInfo {
        let info = self.frame(view);
        if let Some(requests) = self.driver.take_level_requests() {
            self.lag.requests = requests.len();
            self.dataset.apply_level_requests(&requests);
            self.dirty = true;
        }
        info
    }

    /// `settle` with the requests relayed; also waits for the requests the build side still holds.
    fn settle_relayed(&mut self, view: &ViewProjection, quiet: usize, max_frames: usize) {
        let mut calm = 0;
        for _ in 0..max_frames {
            let info = self.frame_relayed(view);
            let settled = !self.dirty
                && self.stage == Some(self.target_px)
                && info.apply == CutApply::Applied
                && info.uploaded_pages == 0
                && info.pending_blocks == 0
                && info.built == 0
                && info.deferred == 0
                && !self.driver.needs_frame()
                && !self.dataset.has_pending_level_requests(true);
            calm = if settled { calm + 1 } else { 0 };
            if calm >= quiet {
                return;
            }
        }
        panic!("did not settle in {max_frames} frames (requests relayed)");
    }
}

/// The sample settled at `pose` like `settled_sample_pose`, with the level requests relayed when
/// `relay` is set (what the browser does; the plain harness never answers them).
fn settled_sample_pose_with(pose: &str, relay: bool) -> (Harness, ViewProjection, ViewProjection) {
    let (mut h, origin, scale) = sample_harness();
    let local = |p: [f64; 3]| -> [f64; 3] { std::array::from_fn(|a| (p[a] - origin[a]) * scale) };
    let data = h.dataset.metadata.hierarchy().stats().data_bounds;
    let mut fit = OrbitCamera::default();
    assert!(fit.fit(
        SceneBounds {
            min: local(data.min()),
            max: local(data.max())
        },
        DEMO_VIEWPORT
    ));
    let initial = ViewProjection::new(fit, DEMO_VIEWPORT, scale, VoxelBuildLimits::default());
    let view = ViewProjection::new(
        demo_pose_camera(fit, pose),
        DEMO_VIEWPORT,
        scale,
        VoxelBuildLimits::default(),
    );
    if relay {
        h.settle_relayed(&initial, 3, 5000);
        h.camera_moved();
        h.settle_relayed(&view, 3, 5000);
    } else {
        h.settle(&initial, 3, 5000);
        h.camera_moved();
        h.settle(&view, 3, 5000);
    }
    (h, view, initial)
}

/// `hi_d` of node `j` of the settled cut, derived as `print_data_bound_totals` does (the pool's
/// own `CellPool::data_bound` is `cfg(test)` of the render crate, out of reach from here).
fn hi_d_of(h: &Harness, view: &ViewProjection, j: usize) -> Option<u8> {
    let ctx = h.cell_ctx.as_ref()?;
    let node = &h.cell_nodes[j];
    let data = h.dataset.metadata.hierarchy().stats().data_bounds;
    let region = SpatialBounds::new(
        std::array::from_fn(|a| node.min[a].max(data.min()[a])),
        std::array::from_fn(|a| node.max[a].min(data.max()[a])),
    )
    .ok();
    let span = buildable_span(h, node.id)?;
    let row = NodeRow {
        id: node.id.get(),
        node_depth: node.lod.node_depth,
        span,
        range: h.last_levels[j].2,
        candidate: node_view_range(
            view,
            &h.dataset.cell_grid(),
            h.dataset.coordinate_origin(),
            region,
            span,
            f64::from(ctx.lod.target_px),
            h.driver.pool().min_side(),
        ),
        ..NodeRow::default()
    };
    data_bound(h, view, j, &row, false).hi_d
}

/// Pages per resident depth of node `id`, ascending.
fn pages_by_depth(h: &Harness, id: NodeId) -> Vec<(u8, usize)> {
    resident_levels(h, id)
        .into_iter()
        .map(|(depth, level)| (depth, level.pages.len()))
        .collect()
}

/// One line per (pose, relay, node) of the eye node, its neighbour and the totals.
/// Run: `cargo test --profile fast -p viewer_wasm --offline --lib eye_node_residency_at_poses -- --ignored --nocapture`.
#[test]
#[ignore = "diagnostic: eye-node residency on the built-in sample at P1 to P5"]
fn eye_node_residency_at_poses() {
    for pose in ["P1", "P2", "P3", "P4", "P5"] {
        // Measured equal with and without relaying the requests (the plan itself builds the
        // levels it wishes), so only the browser-like variant runs.
        let (h, view, _) = settled_sample_pose_with(pose, true);
        print_eye_node_residency(&h, &view, pose, true);
    }
}

fn print_eye_node_residency(h: &Harness, view: &ViewProjection, pose: &str, relay: bool) {
    let tag = format!("EYE pose={pose} relay={}", u8::from(relay));
    let (mut over_levels, mut over_pages) = (0_usize, 0_usize);
    for (j, node) in h.cell_nodes.iter().enumerate() {
        let hi_d = hi_d_of(h, view, j);
        let by_depth = pages_by_depth(h, node.id);
        let resident: Vec<u8> = by_depth.iter().map(|(d, _)| *d).collect();
        let total: usize = by_depth.iter().map(|(_, p)| p).sum();
        let (levels_over, pages_over) = hi_d.map_or((0, 0), |bound| {
            by_depth
                .iter()
                .filter(|(d, _)| *d > bound)
                .fold((0, 0), |(n, p), (_, pages)| (n + 1, p + pages))
        });
        over_levels += levels_over;
        over_pages += pages_over;
        if ![14, 17].contains(&node.id.get()) {
            continue;
        }
        let (_, depths, range) = &h.last_levels[j];
        let mut active = depths.clone();
        active.sort_unstable();
        let wish = h.last_entries[j].1.iter().flatten().max().copied();
        println!(
            "{tag} node={} | resident={resident:?} active={active:?} range={range:?} \
             wish_max={wish:?} data_bound={hi_d:?} | pages_by_depth={by_depth:?} pages={total} | \
             levels_over_bound={levels_over} pages_over_bound={pages_over}",
            node.id.get()
        );
    }
    println!(
        "{tag} node=ALL | cut_nodes={} pool_pages={} | levels_over_bound={over_levels} \
         pages_over_bound={over_pages}",
        h.cell_nodes.len(),
        h.driver.pool().resident_pages()
    );
}

// ---- Zoom-in lag: drawn depth against what the same rule would draw with more levels ----------

/// Default samples per node of the lag measurement (about: see `select_samples`).
const LAG_SAMPLES_PER_NODE: usize = 64;

/// Per-frame record of the lag of the drawn depth at occupied samples (see `Harness::measure_lag`).
/// Lag `b` is against the finest BUILDABLE depth, lag `r` against the finest RESIDENT depth of
/// the node; positive means the frame draws coarser than the reference.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct LagFrame {
    view_gen: u64,
    /// Samples judged, samples off screen (outside the side planes of the frustum; the render clip
    /// distances and the LOD near clamp are not applied: the twin applies the clamp itself), and
    /// samples the node
    /// does not draw (hidden under a child or outside every active level).
    judged: u32,
    off_screen: u32,
    undrawn: u32,
    /// Samples lagging by at least 1, 2 and 3 levels, and the worst lag.
    b: [u32; 3],
    b_max: i32,
    r: [u32; 3],
    r_max: i32,
    /// Samples drawn finer than the buildable ideal.
    finer_b: u32,
    /// Samples drawn at a depth above the node's `data_bound` (when measured), and the largest
    /// excess.
    over_bound: u32,
    over_bound_max: i32,
    /// The judged samples and their lags split by the full view frustum of
    /// `ViewFrustum` (sides plus the near/far planes, see `in_clip`).
    in_clip: LagSplit,
    out_clip: LagSplit,
    /// Size of the last level-request set and the pool's resident pages.
    requests: u32,
    pages: u32,
}

/// Judged samples and lag counts (at least 1, 2 and 3 levels) of one part of the samples.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct LagSplit {
    judged: u32,
    b: [u32; 3],
    r: [u32; 3],
}

#[derive(Default)]
struct LagLog {
    /// Description of the in-clip sample with the largest lag against the resident reference
    /// (then the buildable one), over all frames.
    worst_in_clip: Option<((i32, i32), String)>,
    on: bool,
    /// About this many hash-picked samples per node (0: `LAG_SAMPLES_PER_NODE`).
    per_node: usize,
    /// Control: draw as if the `drop_top` finest active depths of every node were not active
    /// (the lag must then be positive where they matter; 0 measures normally).
    drop_top: u8,
    /// Also measure `data_bound` per node and frame (costs a walk of the finest level's cells).
    bound: bool,
    /// Only these node ids (`None`: every node of the cut).
    only: Option<Vec<u64>>,
    view_gen: u64,
    requests: usize,
    frames: Vec<LagFrame>,
    spans: HashMap<NodeId, Option<(u8, u8)>>,
    /// Sample positions per (node, finest resident depth): scaled frame minus origin.
    samples: HashMap<(NodeId, u8), Vec<[f64; 3]>>,
}

/// Occupied cells of `level` chosen by a position hash (so the choice does not move when a finer
/// level replaces it), about `LAG_SAMPLES_PER_NODE` of them; their centres in the scaled frame
/// minus the origin.
fn select_samples(
    grid: &CellGrid,
    ctx: &CellCtx,
    depth: u8,
    level: &PackedLevel,
    per_node: usize,
) -> Vec<[f64; 3]> {
    let slots: usize = level.pages.iter().map(|p| p.cells as usize).sum();
    let modulus = (slots / per_node.max(1)).max(1).next_power_of_two() as u64;
    let side = grid.root_side / 2.0_f64.powi(i32::from(depth)) * ctx.scale;
    let mut out = Vec::new();
    for (p, page) in level.pages.iter().enumerate() {
        for i in 0..page.cells as usize {
            let instance = level.instances[p * PAGE_SLOTS + i];
            if instance.is_empty() {
                continue;
            }
            let coordinate: [u64; 3] =
                std::array::from_fn(|a| page.origin[a] + u64::from(instance.rel()[a]));
            // Positions at a fixed resolution, whatever the depth.
            let mixed = coordinate.iter().fold(0_u64, |acc, c| {
                let fixed = if depth <= 24 {
                    c << (24 - depth)
                } else {
                    c >> (depth - 24)
                };
                (acc ^ fixed)
                    .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                    .rotate_left(29)
            });
            if (mixed >> 20) & (modulus - 1) != 0 {
                continue;
            }
            out.push(std::array::from_fn(|a| {
                (grid.root_min[a] - ctx.origin[a]) * ctx.scale + (coordinate[a] as f64 + 0.5) * side
            }));
        }
    }
    out
}

/// Whether an eye-relative point lies inside the side planes of the view frustum, ahead of the
/// eye, whatever the render clip distances (the sims' cameras clip the cloud to a thin slab; the
/// rule's decision does not depend on it).
fn in_frustum(ctx: &CellCtx, rel: [f64; 3]) -> bool {
    let dot = |v: [f32; 3]| (0..3).map(|a| rel[a] * f64::from(v[a])).sum::<f64>();
    let depth = dot(ctx.lod.forward);
    let focal = f64::from(ctx.lod.focal);
    depth > 0.0
        && dot(ctx.lod.right).abs() * focal <= depth * ctx.viewport[0] * 0.5
        && dot(ctx.lod.up).abs() * focal <= depth * ctx.viewport[1] * 0.5
}

/// `in_frustum` plus the render clip, as `ViewFrustum` (production, crate-private) tests a point:
/// view depth within `[max(lod_near_depth, clip near), clip far]` (restated, scaled units).
fn in_clip(ctx: &CellCtx, rel: [f64; 3]) -> bool {
    let depth: f64 = (0..3).map(|a| rel[a] * f64::from(ctx.lod.forward[a])).sum();
    in_frustum(ctx, rel) && depth >= ctx.near.max(ctx.clip.0) && depth <= ctx.clip.1
}

impl Harness {
    /// Appends the lag of the frame just drawn to `self.lag.frames`.
    fn record_lag(&mut self, view: &ViewProjection) {
        let mut log = std::mem::take(&mut self.lag);
        let frame = self.measure_lag(&mut log, view);
        log.frames.push(frame);
        self.lag = log;
    }

    /// For every node of the applied cut, the occupied cells of its finest resident level (the
    /// hash-picked ones, on screen) are the samples. The depth the node
    /// draws at the cell centre (`drawn_depth_at`, the twin of the GPU rule over the real active
    /// depths) is compared with what the same twin draws if every depth of the node's buildable
    /// span (`b`) or up to its finest resident depth (`r`) were active. Neither reference knows
    /// whether a finer cell exists at the sample: they are proxies, `r` the honest floor.
    fn measure_lag(&self, log: &mut LagLog, view: &ViewProjection) -> LagFrame {
        let ctx = self.cell_ctx.as_ref().expect("cell mode context");
        let grid = self.dataset.cell_grid();
        let mut f = LagFrame {
            view_gen: log.view_gen,
            requests: log.requests as u32,
            pages: self.driver.pool().resident_pages(),
            ..LagFrame::default()
        };
        for (j, node) in self.cell_nodes.iter().enumerate() {
            if log
                .only
                .as_ref()
                .is_some_and(|ids| !ids.contains(&node.id.get()))
            {
                continue;
            }
            let levels = resident_levels(self, node.id);
            let Some(&(finest, level)) = levels.iter().max_by_key(|(d, _)| *d) else {
                continue;
            };
            let Some(span) = *log
                .spans
                .entry(node.id)
                .or_insert_with(|| buildable_span(self, node.id))
            else {
                continue;
            };
            let per_node = if log.per_node == 0 {
                LAG_SAMPLES_PER_NODE
            } else {
                log.per_node
            };
            let bound = if log.bound {
                hi_d_of(self, view, j)
            } else {
                None
            };
            let samples = log
                .samples
                .entry((node.id, finest))
                .or_insert_with(|| select_samples(&grid, ctx, finest, level, per_node));
            let drawn_lod = if log.drop_top == 0 {
                node.lod
            } else {
                let mut active = node.lod.active;
                for _ in 0..log.drop_top {
                    active &= !(1_u64 << active.ilog2());
                }
                NodeLod { active, ..node.lod }
            };
            let ideal = |rel: [f64; 3], cap: u8| -> Option<u8> {
                let (lo, hi) = (span.0 - node.lod.node_depth, cap - node.lod.node_depth);
                let active = ((1_u64 << (hi + 1)) - 1) & !((1_u64 << lo) - 1);
                let lod = NodeLod { active, ..node.lod };
                drawn_depth_at(&ctx.lod, &lod, rel, node.lo, node.side)
            };
            for position in samples.iter() {
                let rel: [f64; 3] = std::array::from_fn(|a| position[a] - ctx.eye[a]);
                if !in_frustum(ctx, rel) {
                    f.off_screen += 1;
                    continue;
                }
                let Some(drawn) = drawn_depth_at(&ctx.lod, &drawn_lod, rel, node.lo, node.side)
                else {
                    f.undrawn += 1;
                    continue;
                };
                let (Some(ideal_b), Some(ideal_r)) =
                    (ideal(rel, span.1.max(finest)), ideal(rel, finest))
                else {
                    f.undrawn += 1;
                    continue;
                };
                f.judged += 1;
                let (lag_b, lag_r) = (
                    i32::from(ideal_b) - i32::from(drawn),
                    i32::from(ideal_r) - i32::from(drawn),
                );
                let clipped = in_clip(ctx, rel);
                let part = if clipped {
                    &mut f.in_clip
                } else {
                    &mut f.out_clip
                };
                part.judged += 1;
                for k in 0..3 {
                    part.b[k] += u32::from(lag_b > k as i32);
                    part.r[k] += u32::from(lag_r > k as i32);
                }
                if clipped
                    && lag_b > 0
                    && log
                        .worst_in_clip
                        .as_ref()
                        .is_none_or(|(key, _)| (lag_r, lag_b) > *key)
                {
                    let depth: f64 = (0..3).map(|a| rel[a] * f64::from(ctx.lod.forward[a])).sum();
                    let half = grid.root_side / 2.0_f64.powi(i32::from(finest)) * ctx.scale / 2.0;
                    let reach: f64 = (0..3).map(|a| f64::from(ctx.lod.forward[a]).abs()).sum();
                    let active: Vec<u8> = (0..64_u8)
                        .filter(|b| node.lod.active >> b & 1 == 1)
                        .map(|b| b + node.lod.node_depth)
                        .collect();
                    let resident: Vec<u8> = levels.iter().map(|(d, _)| *d).collect();
                    let text = format!(
                        "node={} node_depth={} lo={:?} side={:?}\n\
                         sample finest_depth={finest} rel_eye={rel:?} view_depth={depth:.6}\n\
                         clip near_eff={:.6} (lod_near={:.6} clip_near={:.6}) far={:.6}\n\
                         cell cube depth range=[{:.6}, {:.6}] (centre +- {:.6}); centre in clip={clipped}; \
                         whole cube inside depth clip={}; cube touches clip={}\n\
                         resident={resident:?} active={active:?} span={span:?} hi_d={:?}\n\
                         drawn={drawn} ideal_r={ideal_r} ideal_b={ideal_b} lag_r={lag_r} lag_b={lag_b}",
                        node.id.get(),
                        node.lod.node_depth,
                        node.lo,
                        node.side,
                        ctx.near.max(ctx.clip.0),
                        ctx.near,
                        ctx.clip.0,
                        ctx.clip.1,
                        depth - half * reach,
                        depth + half * reach,
                        half * reach,
                        depth - half * reach >= ctx.near.max(ctx.clip.0)
                            && depth + half * reach <= ctx.clip.1,
                        depth + half * reach >= ctx.near.max(ctx.clip.0)
                            && depth - half * reach <= ctx.clip.1,
                        hi_d_of(self, view, j),
                    );
                    log.worst_in_clip =
                        Some(((lag_r, lag_b), format!("frame_gen={} {text}", log.view_gen)));
                }
                for k in 0..3 {
                    f.b[k] += u32::from(lag_b > k as i32);
                    f.r[k] += u32::from(lag_r > k as i32);
                }
                f.b_max = f.b_max.max(lag_b);
                f.r_max = f.r_max.max(lag_r);
                f.finer_b += u32::from(lag_b < 0);
                if let Some(bound) = bound {
                    let over = i32::from(drawn) - i32::from(bound);
                    f.over_bound += u32::from(over > 0);
                    f.over_bound_max = f.over_bound_max.max(over);
                }
            }
        }
        f
    }
}

/// Frames after the first one of the last camera position until `key` last changes (0 if it did
/// not change after the last move).
fn frames_to_settle<K: PartialEq>(frames: &[LagFrame], key: impl Fn(&LagFrame) -> K) -> usize {
    let Some(last_gen) = frames.iter().map(|f| f.view_gen).max() else {
        return 0;
    };
    let first = frames.iter().position(|f| f.view_gen == last_gen).unwrap();
    (first + 1..frames.len())
        .rev()
        .find(|&i| key(&frames[i]) != key(&frames[i - 1]))
        .map_or(0, |i| i - first)
}

/// Per-frame lines (`LAG`) and the summary (`LAGSUM`) of a run; both stable and diff-friendly.
fn print_lag(label: &str, frames: &[LagFrame]) {
    for (i, f) in frames.iter().enumerate() {
        println!(
            "LAG {label} f={i} gen={} judged={} off={} undrawn={} b>=1,2,3={},{},{} bmax={} \
             r>=1,2,3={},{},{} rmax={} finer_b={} over_bound={} over_max={} req={} pages={}",
            f.view_gen,
            f.judged,
            f.off_screen,
            f.undrawn,
            f.b[0],
            f.b[1],
            f.b[2],
            f.b_max,
            f.r[0],
            f.r[1],
            f.r[2],
            f.r_max,
            f.finer_b,
            f.over_bound,
            f.over_bound_max,
            f.requests,
            f.pages
        );
    }
    let worst = |pick: &dyn Fn(&LagFrame) -> i32| {
        frames
            .iter()
            .enumerate()
            .map(|(i, f)| (pick(f), std::cmp::Reverse(i)))
            .max()
            .map_or((0, 0), |(lag, std::cmp::Reverse(i))| (lag, i))
    };
    let (wb, wr) = (worst(&|f| f.b_max), worst(&|f| f.r_max));
    let mut requests: Vec<u32> = frames.iter().map(|f| f.requests).collect();
    requests.sort_unstable();
    let at = |q: usize| requests.get(q).copied().unwrap_or(0);
    let sum = |pick: &dyn Fn(&LagFrame) -> u32| frames.iter().map(pick).sum::<u32>();
    println!(
        "LAGSUM {label} | frames={} judged_sum={} | settle_b={} settle_r={} | \
         sample_frames_b1={} b2={} b3={} | sample_frames_r1={} r2={} r3={} | \
         worst_b={} at f={} worst_r={} at f={} | over_bound_sample_frames={} over_max={} | \
         requests min={} med={} max={} | pages_end={}\n\
         LAGSPLIT {label} | in_clip: judged={} b1={} b2={} b3={} r1={} r2={} r3={} | \
         out_clip: judged={} b1={} b2={} b3={} r1={} r2={} r3={}",
        frames.len(),
        sum(&|f| f.judged),
        frames_to_settle(frames, |f| (f.judged, f.b, f.b_max)),
        frames_to_settle(frames, |f| (f.judged, f.r, f.r_max)),
        sum(&|f| f.b[0]),
        sum(&|f| f.b[1]),
        sum(&|f| f.b[2]),
        sum(&|f| f.r[0]),
        sum(&|f| f.r[1]),
        sum(&|f| f.r[2]),
        wb.0,
        wb.1,
        wr.0,
        wr.1,
        sum(&|f| f.over_bound),
        frames.iter().map(|f| f.over_bound_max).max().unwrap_or(0),
        at(0),
        at(requests.len() / 2),
        requests.last().copied().unwrap_or(0),
        frames.last().map_or(0, |f| f.pages),
        sum(&|f| f.in_clip.judged),
        sum(&|f| f.in_clip.b[0]),
        sum(&|f| f.in_clip.b[1]),
        sum(&|f| f.in_clip.b[2]),
        sum(&|f| f.in_clip.r[0]),
        sum(&|f| f.in_clip.r[1]),
        sum(&|f| f.in_clip.r[2]),
        sum(&|f| f.out_clip.judged),
        sum(&|f| f.out_clip.b[0]),
        sum(&|f| f.out_clip.b[1]),
        sum(&|f| f.out_clip.b[2]),
        sum(&|f| f.out_clip.r[0]),
        sum(&|f| f.out_clip.r[1]),
        sum(&|f| f.out_clip.r[2]),
    );
}

/// Wheel steps and frames between steps of the synthetic zoom-in, and frames held after it.
const ZOOM_STEPS: usize = 4;
const ZOOM_GAP_FRAMES: usize = 2;
const ZOOM_HOLD_FRAMES: usize = 40;

/// The synthetic zoom-in: the sample settled at P2, then `ZOOM_STEPS` wheel steps (the four that
/// lead from P2 to P3) with `ZOOM_GAP_FRAMES` frames after each, then still. Per frame, the lag
/// at the occupied samples of nodes 14 (holds the eye) and 17 (its neighbour).
/// Run: `cargo test --profile fast -p viewer_wasm --offline --lib synthetic_zoom_in_lag -- --ignored --nocapture`.
#[test]
#[ignore = "diagnostic: lag of the drawn depth while zooming in on the built-in sample"]
fn synthetic_zoom_in_lag() {
    // The last run is a control: the six finest active depths of each node are dropped from what
    // is drawn, so the lag must show up (if it stayed 0 the metric would measure nothing).
    for drop_top in [0, 6] {
        let (mut h, p2, _) = settled_sample_pose_with("P2", true);
        h.lag = LagLog {
            drop_top,
            on: true,
            bound: true,
            per_node: 1 << 20,
            only: Some(vec![14, 17]),
            ..LagLog::default()
        };
        let mut camera = p2.camera();
        let (viewport, scale) = (p2.viewport(), p2.visual_scale());
        let mut view = p2;
        // The state before the first step, as a frame of its own.
        h.frame_relayed(&view);
        h.record_lag(&view);
        for step in 1..=ZOOM_STEPS {
            assert!(camera.zoom(WHEEL_STEP));
            view = ViewProjection::new(camera, viewport, scale, VoxelBuildLimits::default());
            h.camera_moved();
            h.lag.view_gen = step as u64;
            for _ in 0..ZOOM_GAP_FRAMES {
                h.frame_relayed(&view);
                h.record_lag(&view);
            }
        }
        for _ in 0..ZOOM_HOLD_FRAMES {
            h.frame_relayed(&view);
            h.record_lag(&view);
        }
        let control = if drop_top > 0 { " control" } else { "" };
        print_lag(&format!("zoom{control}"), &h.lag.frames);
    }
}
