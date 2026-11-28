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
use viewer_format::{FormatReader, FormatReaderLimits};
use viewer_render_wgpu::cell_lod::{
    LodView, MAX_NODE_SPAN, NodeLod, cell_lo, drawn_depth_at, edge_px, min_child_edge_px, refines,
};
use viewer_render_wgpu::test_support::FakeGpu;
use viewer_render_wgpu::{
    CellCutEntry, CellDriver, CutApply, FramePlan, OrbitCamera, PAGE_SLOTS, PackedLevel,
    SceneBounds, ViewProjection, VoxelBuildLimits, cell_cut_digest, gpu_input_digest,
    node_view_range, representative_depth,
};
use viewer_render_wgpu::{
    CellGrid, MAX_CELL_DEPTH, node_cell_level, representative_depth_for_spacing,
};

/// Longest run of `RetainedPrevious` frames the pool may take while blocks stream in.
const MAX_RETAINED_STREAK: usize = 60;
/// Longest wait of a held or excluded node whose threshold level is requested (R3-Q20/Q21).
const MAX_HOLD_FRAMES: u32 = 120;

mod colour;
mod diagnostics;
mod real;
mod synthetic;

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
    target: FakeGpu,
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
            target: FakeGpu::default(),
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
// Helpers shared with the synthetic suite (`cells_stream_tests/synthetic.rs`)
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

// ---------------------------------------------------------------------------------------------
// Real data (belvedere v8)
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

mod bench;

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

// ---------------------------------------------------------------------------------------------
// Helpers shared with the colour suite (`cells_stream_tests/colour.rs`)
// ---------------------------------------------------------------------------------------------

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

/// The built-in sample loaded in a harness, with its coordinate origin and visual scale.
fn sample_harness() -> (Harness, [f64; 3], f64) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/web-host/sample-v8.oct");
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
    .with_global_frame(true);
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

// ---------------------------------------------------------------------------------------------
// Helpers shared with the diagnostics suite (`cells_stream_tests/diagnostics.rs`)
// ---------------------------------------------------------------------------------------------

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

/// `ceil(min(points, 8^(depth - node_depth)) / 512)` pages, at least one: the formula of the
/// pool's `estimate_pages` (`cells/pool/residency.rs`, private), restated.
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

/// `ViewSpan::on_axis` (`cells/cut.rs`, private) restated: the first buildable depth whose
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

/// Pages per resident depth of node `id`, ascending.
fn pages_by_depth(h: &Harness, id: NodeId) -> Vec<(u8, usize)> {
    resident_levels(h, id)
        .into_iter()
        .map(|(depth, level)| (depth, level.pages.len()))
        .collect()
}

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
