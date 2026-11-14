//! R3 Task 9b2a: the px ladder of the cells path, streamed block by block (R3-Q19), with a
//! strict recording `CellTarget` and a cell-level coverage oracle. The real-data tests need
//! `target/render-data/belvedere-v75.oct` and skip with a message without it, unless
//! `R3_REQUIRE_REAL_DATA=1` makes the missing file a failure.

use super::cells::MAX_CELL_CUT_GAP;
use super::cells_tests::{SIDE, driver_for, node_block, octree_dataset, octree_nodes};
use super::*;
use crate::lod_driver::{CellStep, update_cells_staged};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
    time::Instant,
};
use viewer_core::{BlockId, HierarchySemantics, SpatialBounds, SpatialHierarchy, SpatialNode};
use viewer_format::{FormatReader, FormatReaderLimits, OctreeReader};
use viewer_render_wgpu::{
    CellCutEntry, CellDriver, CellPoolError, CellTarget, CutApply, FramePlan, MASK_WORDS,
    OrbitCamera, PackedLevel, PageEntry, PageFrame, PageInfo, SceneBounds, ViewProjection,
    VoxelBuildLimits, cell_cut_digest,
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
        for (slot, page) in page_slots.iter().zip(&level.pages) {
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
    fn set_node_mask(&mut self, slot: u32, mask: &[u32; MASK_WORDS]) -> Result<(), CellPoolError> {
        if slot >= self.next_mask {
            return Err(CellPoolError::MaskSlotNotAllocated(slot));
        }
        self.masks.insert(slot, *mask);
        Ok(())
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
        self.table = pages.to_vec();
        self.page_frame = Some(*frame);
        Ok(())
    }
}

impl StrictTarget {
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

    /// Depth the cut wants at `p`: the finest wanted depth of the unhidden cells holding it.
    fn wanted_at(&self, p: [f64; 3]) -> Option<u8> {
        (0..self.nodes.len())
            .filter_map(|i| {
                let c = self.cell_at(i, p)?;
                if cell_hidden(&self.entries[i].mask, c) {
                    return None;
                }
                self.entries[i].wanted[octant_of_cell(c)]
            })
            .max()
    }

    /// Finest depth drawn at `p` (unhidden), if any.
    fn depth_at(&self, p: [f64; 3]) -> Option<u8> {
        (0..self.nodes.len())
            .filter_map(|i| {
                let c = self.cell_at(i, p)?;
                if cell_hidden(&self.entries[i].mask, c) {
                    return None;
                }
                self.active[i][octant_of_cell(c)]
            })
            .max()
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
    sample_points: Vec<[f64; 3]>,
    sample_depths: Vec<Option<u8>>,
    /// Depth last drawn per (node, octant) while the camera is still.
    octant_depths: HashMap<(NodeId, usize), u8>,
    /// Cross-node coarsenings: a sample point drawn at depth `before` is drawn at `after` with
    /// `after < min(before, wanted now)`.
    zone_coarsenings: usize,
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
    /// The bench counts holes, lost samples and long waits instead of failing the frame.
    soft_checks: bool,
    /// When set, a (node, octant) that gets coarser is counted here instead of failing the frame.
    count_octant_coarsenings: bool,
    octant_coarsenings: usize,
    /// Per-octant drops regardless of the wish (informative).
    octant_lowered: usize,
    last_cut_digest: Option<u64>,
    /// The cut of the last frame that changed it.
    last_cut: Option<CutView>,
    last_stage: Option<f64>,
    totals: Totals,
    check_zones: bool,
}

impl Harness {
    fn new(dataset: LodDataset, target_px: f64, blocks: Blocks, pages: Pages) -> Self {
        let driver = driver_for(&dataset, u64::MAX);
        let root = dataset.metadata.hierarchy().stats().root_bounds;
        // A fixed lattice of probe points over the root: the zone check compares the finest
        // drawn depth at each of them from frame to frame. It spans 0.01..0.99 of the root and
        // avoids the dyadic planes (node faces): a point exactly on a face belongs to two nodes under closed bounds, and
        // would count a neighbour's zone as its own.
        let mut sample_points = Vec::new();
        for x in 0..7 {
            for y in 0..7 {
                for z in 0..7 {
                    sample_points.push(std::array::from_fn(|axis| {
                        let t = 0.01 + 0.98 * ([x, y, z][axis] as f64 + 0.37) / 6.37;
                        root.min()[axis] + (root.max()[axis] - root.min()[axis]) * t
                    }));
                }
            }
        }
        let sample_depths = vec![None; sample_points.len()];
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
            sample_points,
            sample_depths,
            octant_depths: HashMap::new(),
            zone_coarsenings: 0,
            lost_samples: 0,
            planner_of_drawn_cut: None,
            hole_frames: 0,
            first_failure: None,
            soft_checks: false,
            count_octant_coarsenings: false,
            octant_coarsenings: 0,
            octant_lowered: 0,
            last_cut_digest: None,
            last_cut: None,
            last_stage: None,
            totals: Totals::default(),
            check_zones: true,
        }
    }

    /// The camera moved: the zone memory of the old view is dropped. The ladder restarts by
    /// itself only after a jump (R3-Q22), so `stage` is left alone.
    fn camera_moved(&mut self) {
        self.dirty = true;
        self.forget_zones();
        self.last_cut_digest = None;
        self.last_stage = None;
        self.retained_run = 0;
    }

    /// Drops the zone memory only: what was drawn before no longer bounds what is drawn now.
    fn forget_zones(&mut self) {
        self.sample_depths.iter_mut().for_each(|d| *d = None);
        self.octant_depths.clear();
    }

    /// A failed check: panics, or is counted when `soft_checks` is set.
    fn fail(&mut self, message: String) {
        assert!(self.soft_checks, "{message}");
        self.first_failure.get_or_insert(message);
    }

    /// The oracles on the cut a frame drew (holes, coarsenings, lost samples); runs when the
    /// drawn cut changed.
    fn check_drawn_cut(&mut self, number: usize, plan: &FramePlan, digest: u64) {
        if self.last_cut_digest != Some(digest) {
            let cut = CutView::new(&self.dataset, plan);
            let holes = cut.holes();
            if !holes.is_empty() {
                self.hole_frames += 1;
                self.fail(format!(
                    "frame {number}: {} holes, first: {:?}",
                    holes.len(),
                    &holes[..holes.len().min(3)]
                ));
            }
            if self.check_zones {
                // The pool stage of the brief: the depth drawn per (node, octant) never gets
                // coarser while the camera is still.
                for (i, node) in cut.nodes.iter().enumerate() {
                    for (octant, drawn) in cut.active[i].iter().enumerate() {
                        let Some(now) = *drawn else {
                            continue;
                        };
                        let before = self.octant_depths.insert((*node, octant), now);
                        if self.count_octant_coarsenings {
                            // Same rule as the cross-node metric: only below what the capped
                            // wish allows counts (a fit-forced lowering to the wish is legitimate).
                            let wanted = cut.entries[i].wanted[octant].unwrap_or(u8::MAX);
                            if before.is_some_and(|b| now < b) {
                                self.octant_lowered += 1;
                            }
                            if before.is_some_and(|b| now < b.min(wanted)) {
                                self.octant_coarsenings += 1;
                            }
                        } else {
                            assert!(
                                before.is_none_or(|before| now >= before),
                                "frame {number}: node {node:?} octant {octant} went from depth \
                                 {before:?} to {now}"
                            );
                        }
                    }
                }
                // Across nodes (an ancestor replaced by its child) a point only counts as coarsened
                // if it ends below what it drew before and below what the cut wants now (the
                // wish may have dropped: zoom-out). The real-data tests assert the count is 0;
                // the synthetic ones only print it. A lost sample is a point the previous cut
                // drew, the planner's cut still covers and the new cut draws with nothing.
                let hierarchy = self.dataset.metadata.hierarchy();
                let planned: &[NodeId] = match &self.planner_of_drawn_cut {
                    Some(ids) => ids,
                    None => self
                        .dataset
                        .visible_plan
                        .as_ref()
                        .map(|plan| plan.node_ids())
                        .unwrap_or_default(),
                };
                let planner: Vec<SpatialBounds> = planned
                    .iter()
                    .filter(|id| self.driver.has_node(**id))
                    .filter_map(|id| hierarchy.node(*id).map(SpatialNode::bounds))
                    .collect();
                let mut lost = 0;
                for (i, p) in self.sample_points.iter().enumerate() {
                    let now = cut.depth_at(*p);
                    if let (Some(before), Some(after)) = (self.sample_depths[i], now)
                        && after < before.min(cut.wanted_at(*p).unwrap_or(u8::MAX))
                    {
                        self.zone_coarsenings += 1;
                    }
                    let drawn_before = self
                        .last_cut
                        .as_ref()
                        .is_some_and(|c| c.depth_at(*p).is_some());
                    if drawn_before
                        && now.is_none()
                        && planner
                            .iter()
                            .any(|b| (0..3).all(|a| p[a] >= b.min()[a] && p[a] <= b.max()[a]))
                    {
                        lost += 1;
                    }
                    if now.is_some() {
                        self.sample_depths[i] = now;
                    }
                }
                if lost > 0 {
                    self.lost_samples += lost;
                    self.fail(format!("frame {number}: {lost} lost samples"));
                }
            }
            self.last_cut_digest = Some(digest);
            self.last_cut = Some(cut);
        }
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
        self.target.check_plan(plan);
        let digest = cell_cut_digest(&plan.entries);
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
             digest {:016x} | pages used {} | zone coarsenings {} | lost samples {} | hold age max {} (waiting frames {}, held peak {}, \
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
            self.dataset.pages.as_ref().map_or(0, |p| p.pages()),
            self.zone_coarsenings,
            self.lost_samples,
            t.hold_age_max,
            t.wait_frames,
            t.held_peak,
            t.excluded_peak,
            t.fallback_peak,
            t.hold_unsettled,
        );
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
    let PagedSynthetic {
        dataset,
        blocks,
        pages,
    } = paged_synthetic(budget, cache_bytes);
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
            let cut = h.last_cut.take()?;
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
    capped.last_cut = None;
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
        deferred_frames += usize::from(info.deferred > 0);
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
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/render-data")
        .join(name);
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

#[path = "lod_cells_bench.rs"]
mod bench;

#[path = "lod_cells_sim.rs"]
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
                    std::array::from_fn(|a| (b.min()[a] + b.max()[a]) / 2.0),
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
    let Some(mut real) = open_real("belvedere-v75.oct", 4.0, 256 << 20) else {
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
    assert_eq!(real.harness.lost_samples, 0);
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
    let Some(mut real) = open_real("belvedere-v75.oct", 4.0, REGRAFT_PAGE_BUDGET) else {
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
    assert_eq!(real.harness.lost_samples, 0);
}

#[test]
fn real_pages_with_drawn_nodes_are_not_evicted() {
    let Some(mut real) = open_real("belvedere-v75.oct", 4.0, REGRAFT_PAGE_BUDGET) else {
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
    assert_eq!(real.harness.lost_samples, 0);
}
