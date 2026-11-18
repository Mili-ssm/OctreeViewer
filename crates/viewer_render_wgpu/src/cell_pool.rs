//! CPU residency of the GPU cell pool (R3 Task 7a): pure and deterministic, no GPU.
//!
//! The pool owns a fixed number of 8 KiB pages. Each frame the caller gives it the cut, and gets a
//! [`FramePlan`]: the cut to draw, the levels to fetch, the page uploads and frees to execute, the
//! mask writes and the active levels. Data arrives later through [`CellPool::offer`].
//!
//! Frame order: release delayed frees -> local fallback ancestors (R3-Q19) -> wanted cut
//! (resident = stored) and fit (R3-Q11) -> real cut on the resident levels -> apply or retain
//! (R3-Q10, R3-Q19) -> cancel, evict, request -> uploads.
//!
//! Residency (R3-Q2): the frame that plans the last page upload of a level marks it resident at
//! its end, but the cut of that frame was computed before, so the level is first drawn (in
//! `active`) one frame later, when every page has been executed. The pages of an evicted level
//! return to the free list two frames after it last drew, so a slot is never rewritten while the
//! current or the previous frame can read it. Pages that never drew (a cancelled upload, or a
//! level evicted before it was drawn) are free for the uploads of the same frame. A level that
//! waits for delayed pages blocks the lower-priority levels from taking the free pages meanwhile
//! (a one-frame stall in exchange for priority order).
//!
//! The wanted cut (resident = stored) only steers what is requested: nodes that would make it fail
//! with `DescendantTooFine` get no requests. Applying is decided on the real cut, on the actual
//! residency (R3-Q19): it applies when `cell_cut` is Ok and every cut node without a resident level
//! is covered by an ancestor of the cut that draws (geometric ancestry, gap <= 3 like the masks);
//! otherwise the previous cut stays (`RetainedPrevious`). A real cut that fails with
//! `DescendantTooFine` stays retained and is reported in the counters.
//!
//! Local fallback (R3-Q19): a cut node without a resident level keeps in the real cut its nearest
//! ancestor (gap <= 3) from the last applied cut, until the node is resident, so what is missing in
//! one region never blocks the others. The pool remembers the [`PoolNode`] of the applied nodes
//! for that. The ancestor's mask hides only resident descendants, so it draws exactly where the
//! node is missing. The wanted cut and the fit never see the fallback nodes (they steer requests);
//! the real cut does, and its memo is keyed on the fallback set and the depths of those nodes.
//! The octants of covered nodes count as stages.
//!
//! Only refine (R3-Q21): a cut node `C` whose ancestor `A` drew its zone in the last applied cut
//! stays *held* while `C` would not draw, in every octant, a level at or above `min(depth A drew
//! over that octant, wanted depth of C, cap of C)`, judged on what `cell_cut` would put in `drawn`
//! (the closest resident level). Only a `C` with a covering ancestor can be held (`CoverRule`, the
//! rule of the fallback). A held `C` gets no resident level in the real cut, so it neither draws
//! nor masks `A`, and it counts as missing in the fallback, so `A` stays in the cut. The hold is
//! decided on the real cut and then fed back to it: `frame` runs the planning again (at most
//! `HOLD_PASSES` times) until the held set no longer changes; when it does not settle, the nodes
//! held in either set stay held and `hold_unsettled` counts them. It is per node because the
//! mask hides the whole footprint of `C`. The levels of `C` are requested and uploaded as usual.
//! Cell mode adds the D5 gate (E7): a `C` that would mask an ancestor of the real cut stays held
//! (same mechanism) while that ancestor has no resident depth >= its `rep`; that depth is
//! requested first and protected, and joins the ancestor's active set once its mask is set. While
//! the ancestor's fit cap (or source density) is below its `rep` there is no D5 depth to request,
//! so the child stays held and the ancestor draws the zone, as the whole cloud is coarser under
//! pressure (R3-Q11).
//! Requests and uploads go coarse to fine: the first
//! level of the nodes that draw nothing, then absolute level depth, then distance.
//!
//! Sizes: the fit uses, per level, the packed size once offered, else the exact size given by
//! [`CellPool::set_level_sizes`] (in the viewer every level of a node is built together, so its
//! sizes are known before any upload), else an estimate `ceil(min(points, 8^gap) / 512)` pages.
//! `pack_level` may split a page when its extent would exceed 2^16, so the estimate is not a strict
//! upper bound; an exact size replaces it, the fit corrects on the next frame, and the 1/8 reserve
//! absorbs the transient.
//!
//! CPU cost (R3 Task 7d): the depth each octant of a cut node asks for (the expensive part of
//! `cell_cut`, one projection per stored level) is cached per node, and it is computed for the
//! node's full stored list: for a capped list the answer is `min(depth, last stored depth)`. A
//! cached depth is refreshed when the node's data changes, or when the camera has drifted from the
//! pose it was computed at by more than [`DRIFT_FRACTION`] of the eye-to-node distance or
//! [`ROTATION_RAD`] (each scaled by a per-node factor in 0.5..=1, so refreshes do not all land on
//! one frame; nodes past half their limit are refreshed ahead of time, within a per-frame budget),
//! or as soon as the camera stops (a still pose is always exact). That bounds the change of a
//! projected edge to about 5 % (6 % by rotation), which only matters for octants within that
//! margin of the target. A drift refresh searches outward from the cached depth and assumes that
//! finer levels project smaller (`octant_wanted_depth_near`). The wanted cut and the fit are reused while the cut, the sizes, the view
//! parameters and every cached depth are unchanged; the real cut also needs the same residency.
//! In steady state a frame therefore calls `cell_cut` zero times; a change calls it once per
//! stage that has to be redone (wanted, fit steps, real).

use std::{cell::Cell, collections::BTreeSet, ops::Range, rc::Rc, sync::Arc};

use viewer_core::{NodeId, SpatialBounds};

pub use crate::cell_cut::LodMode;

use crate::camera::VERTICAL_FOV;
use crate::node_cells::{node_range, representative_depth_for_spacing};

use crate::{
    CellCutEntry, CellCutError, CellCutNode, CellGrid, PAGE_SLOTS, PackedLevel, ViewProjection,
    cell_cut::{
        FastMap, FastSet, MAX_MASK_LEVELS, PrevState, ProbeBox, STEP_WAIT_FRAMES, behind_octants,
        behind_wanted, cell_cut_ranged, cell_cut_with, cell_depth_range, drawn_levels,
        in_cell_range, node_far_depth, octant_wanted_depth_in, octant_wanted_depth_near,
        served_depth,
    },
    representative_depth,
};

/// Bytes of one pool page.
pub const POOL_PAGE_BYTES: u64 = (PAGE_SLOTS * 16) as u64;
/// 512 MiB (ADR 0040).
pub const DEFAULT_POOL_PAGES: u32 = 65_536;
/// Default upload budget per frame.
pub const DEFAULT_UPLOAD_BUDGET_BYTES: u64 = 16 << 20;

/// The fit starts coarsening above this share of the usable capacity and relaxes below `LOW`.
pub const HIGH_PERCENT: u64 = 90;
pub const LOW_PERCENT: u64 = 80;
/// Unused levels are evicted while the allocated pages exceed this share of the usable capacity.
const SOFT_PERCENT: u64 = 80;
/// Frames between the last draw of a level and the reuse of its pages.
const FREE_DELAY: u64 = 2;
/// A cached depth is refreshed when the eye has moved more than this share of the node's
/// effective view depth since it was computed: a projected edge is inversely proportional to the
/// depth, so it changes by about that share (about 5 % of the target at the decision boundary, 4 % plus second order).
const DRIFT_FRACTION: f64 = 0.04;
/// ... or when the view direction has turned by an angle `w` with `w * tan(half diagonal fov)`
/// above this share: a point at the corner of the frame changes depth by that much per radian,
/// the worst case. The half diagonal fov comes from the camera's vertical fov and the viewport.
const ROTATION_SHARE: f64 = 0.05;
/// Nodes that have used this share of their limit are refreshed ahead of time, `1 / EARLY_FRACTION`
/// of the cut per frame at most (at least 8), the closest to the limit first.
const EARLY_SHARE: f64 = 0.5;
const EARLY_FRACTION: usize = 8;

/// Pages of a pool of `capacity_pages` the fit may use; above them even the coarsest levels
/// overflow. The build side's fit to the pool (ADR 0040) uses the same measure.
pub fn usable_pages(capacity_pages: u32) -> u64 {
    u64::from(capacity_pages - capacity_pages / 8)
}

/// Identity of a resident level (R3 global constraints).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LevelKey {
    pub session: u32,
    pub node: NodeId,
    pub depth: u8,
    pub color_gen: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolConfig {
    pub capacity_pages: u32,
    pub upload_budget_bytes: u64,
    pub session: u32,
    pub color_gen: u32,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            capacity_pages: DEFAULT_POOL_PAGES,
            upload_budget_bytes: DEFAULT_UPLOAD_BUDGET_BYTES,
            session: 0,
            color_gen: 0,
        }
    }
}

/// What the pool needs to know about a node of the cut.
#[derive(Debug, Clone, PartialEq)]
pub struct PoolNode {
    /// Dataset frame, like the grid.
    pub bounds: SpatialBounds,
    /// Absolute depths of the stored levels, ascending.
    pub stored: Vec<u8>,
    /// Depths the node can build on demand, inclusive: `stored` lies inside it.
    pub buildable: (u8, u8),
    /// Points of the node, for the size estimate of levels that have not arrived.
    pub points: u64,
    /// Box each octant's LOD probe looks at (dataset frame): the octant box clipped to the frame
    /// data extent, computed once per node and independent of the stored levels; `None` for an
    /// octant outside the extent. `None` overall when no tighter extent is known, and the probes
    /// then use the geometric octant, which can reach the eye while the data is far.
    pub octant_data_bounds: Option<[Option<SpatialBounds>; 8]>,
}

impl PoolNode {
    pub(crate) fn probe_box(&self, octant: usize) -> ProbeBox {
        match self.octant_data_bounds {
            None => ProbeBox::Geometric,
            Some(bounds) => bounds[octant].map_or(ProbeBox::Empty, ProbeBox::Data),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CutApply {
    Applied,
    /// The previous complete cut is still the one drawn (R3-Q10, overflow, nothing to draw yet).
    RetainedPrevious,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageUpload {
    pub key: LevelKey,
    /// Pages of the offered level to write now.
    pub page_range: Range<usize>,
    /// Pool page slot of each page in `page_range`.
    pub slots: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaskUpdate {
    pub slot: u32,
    pub mask: [u64; 8],
    /// Bit `d - node_depth`: depth `d` is active in the node. Cell mode: the depths emitted as
    /// active levels; octant mode: the depths it draws that lie within 63 of `node_depth`.
    pub active: u64,
    /// Depth whose levels span the octants of the node.
    pub node_depth: u8,
    /// Cell index of the node's lower corner at `node_depth` (the `node_range` start per axis).
    pub node_lo: [u64; 3],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveLevel {
    pub key: LevelKey,
    /// Bit `o` = octant `o` draws this level (`0xff` in cell mode: the GPU chooses per cell).
    pub octant_bits: u8,
    pub mask_slot: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CellPoolEvent {
    /// The resident level left the pool; its worker-side data can go (Task 10).
    Evict(LevelKey),
    /// A requested or uploading level is no longer needed; drop its data or the request.
    Cancel(LevelKey),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PoolCounters {
    /// Octants drawing a level coarser than wanted (R3-Q3).
    pub stages: u32,
    /// Cut nodes with a non-masked octant that has no resident level to draw.
    pub nodes_without_level: u32,
    /// Nodes currently truncated by the fit (R3-Q11).
    pub pool_coarsened: u32,
    /// Frames, since the pool was created, that could not fit even at the coarsest levels.
    pub pool_overflow: u32,
    pub resident_pages: u32,
    /// `cell_cut` calls this frame.
    pub cut_evaluations: u32,
    /// The real cut failed with `DescendantTooFine` this frame (the previous cut is retained).
    pub descendant_too_fine: bool,
    /// Bytes planned for upload this frame.
    pub uploaded_bytes: u64,
    /// Cut nodes whose cached octant depths were recomputed this frame (camera drift or new data).
    pub depth_refreshes: u32,
    /// The wanted cut and the fit of the previous frame were reused.
    pub wanted_reused: bool,
    /// The real cut of the previous frame was reused (same wanted cut, same residency).
    pub real_reused: bool,
    /// Cut nodes held back by R3-Q21 (they count in `nodes_without_level` and their octants in
    /// `stages`).
    pub held_nodes: u32,
    /// Pages of the levels the applied cut draws for the fallback ancestors and for the ancestors
    /// of held nodes: the cost of "only refine", to compare with the fit's reserve.
    pub fallback_pages: u32,
    /// Held nodes the hold search did not settle in `HOLD_PASSES` passes: they stay held while an
    /// ancestor of the real cut covers them, so the image never draws coarser than before.
    pub hold_unsettled: u32,
    /// Cut nodes left out of the applied cut because nothing draws their zone yet (R3-Q20); their
    /// coarsest level is requested first. They count in `nodes_without_level`.
    pub excluded_nodes: u32,
    /// Nodes of the previous applied cut whose zone the new cut would stop drawing with nothing
    /// to cover it; the frame retains the previous cut.
    pub lost_nodes: u32,
    /// Longest run of consecutive frames a held or excluded node has been waiting (R3-Q20/Q21).
    pub hold_age_max: u32,
    /// Octants whose cut applied this frame drew a level more than one away from their previous
    /// one because their wait timed out or their previous level was gone (ADR 0053): the only
    /// jumps of more than one level the stepping rule allows.
    pub step_hops: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FramePlan {
    pub apply: CutApply,
    /// The cut to draw: the new one, or the retained one.
    pub entries: Vec<CellCutEntry>,
    pub fetch_requests: Vec<LevelKey>,
    /// Wanted depths that are buildable but not stored, coarse to fine: the owner builds them on
    /// demand. They are not fetched; the node keeps drawing its resident levels meanwhile.
    pub level_requests: Vec<LevelKey>,
    pub uploads: Vec<PageUpload>,
    /// Page slots to make inert; they are free for the uploads of this same frame.
    pub page_frees: Vec<u32>,
    pub mask_updates: Vec<MaskUpdate>,
    pub active: Vec<ActiveLevel>,
    pub events: Vec<CellPoolEvent>,
    pub counters: PoolCounters,
    /// The nodes the fit truncates this frame (`counters.pool_coarsened` of them), ascending.
    pub coarsened_nodes: Vec<NodeId>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Pose {
    eye: [f64; 3],
    forward: [f64; 3],
}

/// The wanted depth of each octant of a cut node, for its full stored list, at the pose it was
/// computed at.
struct NodeDepths {
    node: PoolNode,
    depths: [u8; 8],
    /// Cell mode: the node's far wanted depth and the octants in front of the eye
    /// (`node_far_depth`); `(0, 0)` in octant mode, which never reads them.
    far: (u8, u8),
    pose: Pose,
    /// Effective view depth of the node at that pose, in scaled units: the depth of its nearest
    /// point along the view direction, times the cosine of its off-axis angle (`depth^2 /
    /// distance`), so that moving the eye by a share of it changes a projected edge by about
    /// that share also away from the axis. 0 when the nearest point is not in front.
    depth: f64,
    /// Computed by the full search at exactly this pose (R3-Q18); a hinted search is not.
    exact: bool,
    /// Per-node factor in 0.5..=1 on the refresh thresholds.
    scale: f64,
}

/// What the cached depths depend on besides the node and the pose; a change drops them all.
#[derive(Debug, Clone, Copy, PartialEq)]
struct ViewKey {
    target_px: f64,
    viewport: [u32; 2],
    visual_scale: f64,
    grid: CellGrid,
    origin: [f64; 3],
    min_side: Option<f64>,
}

/// Result of the wanted cut and the fit, valid while its inputs are.
struct WantedMemo {
    cut: Vec<NodeId>,
    size_epoch: u64,
    generation: u64,
    needed: Rc<FastSet<LevelKey>>,
    /// Wanted levels that are buildable but not stored.
    requests: Rc<FastSet<LevelKey>>,
    /// Nodes left out of `needed` because they are too fine for an ancestor in the cut.
    offenders: BTreeSet<NodeId>,
    overflow: bool,
    /// Eye the fit ranked the nodes from, and the nearest eye-to-node view depth of the cut:
    /// while caps are active the memo is dropped when the eye moves past the drift limit.
    eye: [f64; 3],
    min_depth: f64,
    had_caps: bool,
    /// D5 levels (cell mode) the fit counted: they belong to ancestors outside `cut`, which the
    /// driver can forget while the cut stays the same, so a different set invalidates the memo.
    d5: FastSet<LevelKey>,
}

/// Result of the real cut, valid for one wanted generation and one residency epoch.
struct RealMemo {
    generation: u64,
    residency_epoch: u64,
    /// `None`: the cut failed with `DescendantTooFine`.
    entries: Option<Vec<CellCutEntry>>,
    planned_missing: u32,
    /// Unmasked octants of the covered nodes without a level: they draw a coarser ancestor.
    covered_octants: u32,
    /// Fallback ancestors and descendants the real cut was computed with.
    fallback: Vec<NodeId>,
    /// Cut nodes left out of the real cut (R3-Q20).
    excluded: FastSet<NodeId>,
    /// Nodes of the applied cut this real cut would stop drawing (R3-Q20).
    lost: u32,
    /// The real cut draws nothing while the applied one draws: a pose jump waiting for its first
    /// level keeps the image.
    blank: bool,
    /// Digest of the held set the real cut was computed with.
    hold_digest: u64,
    /// Digest of the drawn depths of the applied cut the hold decisions read; the memo is stale
    /// when it differs.
    applied_digest: u64,
    /// Capped nodes whose finer level from the applied cut stands in for the capped one.
    kept_finer: Vec<NodeId>,
    /// Held set the entries lead to (equal to the input at a fixed point).
    next_hold: FastSet<NodeId>,
    /// Nodes that are held in the input or in `next_hold` and have a covering ancestor: where the
    /// search settles when it does not converge.
    settle: FastSet<NodeId>,
    /// Ancestors that cover the nodes of `settle`.
    hold_ancestors: FastSet<NodeId>,
    /// `self.applied` already holds `entries`.
    applied: bool,
}

/// Maximum times `frame` plans again because the held set changed.
const HOLD_PASSES: usize = 3;

/// What `plan` leaves for the next pass of the same frame.
#[derive(Default)]
struct Carry {
    page_frees: Vec<u32>,
    evaluations: u32,
    wanted_reused: Option<bool>,
    unsettled: u32,
}

/// What stands in for the cut nodes that have no level (R3-Q19, R3-Q20).
#[derive(Clone, Default)]
struct Fallbacks {
    /// Applied ancestors with a resident level that draw where a cut node has none.
    ancestors: Vec<NodeId>,
    /// Applied descendants of an excluded node that were drawing: they keep drawing its zone.
    descendants: Vec<NodeId>,
    /// Cut nodes with no level and nothing that draws their zone: left out of the real cut.
    excluded: FastSet<NodeId>,
}

impl Fallbacks {
    /// The nodes added to the real cut, ancestors first.
    fn extra(&self) -> Vec<NodeId> {
        self.ancestors
            .iter()
            .chain(&self.descendants)
            .copied()
            .collect()
    }
}

/// How a pass treats a held set that differs from the one its cut leads to.
#[derive(Clone, Copy, PartialEq)]
enum Round {
    /// Plan again with the new set.
    Free,
    /// The last free pass: settle on the nodes that stay held and plan once more.
    Last,
    /// Apply the settled set as it is.
    Settled,
}

enum Pass {
    Done(Box<FramePlan>),
    /// The held set changed: plan again with it.
    Redo(FastSet<NodeId>),
}

/// Held nodes of a real cut and the applied ancestors that cover them.
#[derive(Default)]
struct HoldEval {
    hold: FastSet<NodeId>,
    settle: FastSet<NodeId>,
    ancestors: FastSet<NodeId>,
}

/// Order-free digest of a set of nodes.
fn set_digest(set: &FastSet<NodeId>) -> u64 {
    set.iter().fold(set.len() as u64, |acc, id| {
        acc.wrapping_add(id.get().wrapping_mul(0x9E37_79B9_7F4A_7C15).rotate_left(29))
    })
}

/// Where the cover of a node without a level comes from (R3-Q19); the fallback and the hold of
/// R3-Q21 share it, so that a held node always has an ancestor in the real cut.
struct CoverRule {
    in_cut: FastSet<NodeId>,
    /// Cells that hold a resident cut node more than `MAX_MASK_LEVELS` below them.
    too_deep: FastSet<(u8, [u64; 3])>,
    /// Cells of the cut nodes that draw (`with_level`).
    drawing: FastSet<(u8, [u64; 3])>,
    /// The applied cut was indexed for this grid, so its nodes can be ancestors.
    applied_ok: bool,
}

enum Cover {
    /// A cut node that draws lies over the node, at this cell: no fallback is needed.
    Cut {
        cell: (u8, [u64; 3]),
        gap: u8,
    },
    /// The nearest applied ancestor outside the cut that has a resident level.
    Fallback {
        ancestor: NodeId,
        gap: u8,
    },
    Nothing,
}

impl CoverRule {
    fn new(
        pool: &CellPool,
        cut: &[NodeId],
        with_level: &[NodeId],
        nodes: &FastMap<NodeId, PoolNode>,
        grid: &CellGrid,
    ) -> Self {
        let mut too_deep = FastSet::default();
        let mut drawing = FastSet::default();
        for id in with_level {
            let f = Footprint::new(grid, nodes[id].bounds);
            for k in MAX_MASK_LEVELS + 1..=f.rep.saturating_sub(3) {
                too_deep.insert(f.ancestor(k));
            }
            drawing.insert((f.rep, f.lo));
        }
        Self {
            in_cut: cut.iter().copied().collect(),
            too_deep,
            drawing,
            applied_ok: !pool.applied_index.nodes.is_empty()
                && pool.applied_index.grid == Some(*grid),
        }
    }

    fn cover(&self, pool: &CellPool, f: &Footprint) -> Cover {
        let mut nearest = None;
        for gap in f.gaps() {
            let cell = f.ancestor(gap);
            if self.drawing.contains(&cell) {
                return Cover::Cut { cell, gap };
            }
            if self.applied_ok
                && nearest.is_none()
                && let Some(&ancestor) = pool.applied_index.cells.get(&cell)
                && !self.in_cut.contains(&ancestor)
                && !self.too_deep.contains(&cell)
                && pool.applied_index.grid.as_ref().is_some_and(|grid| {
                    pool.has_resident_level(ancestor, &pool.applied_index.nodes[&ancestor], grid)
                })
            {
                nearest = Some(Cover::Fallback { ancestor, gap });
            }
        }
        nearest.unwrap_or(Cover::Nothing)
    }
}

/// Does `ancestor` (`gap` levels above `f`) draw, in the real cut `entries`, everywhere `f` lies?
/// A hidden octant (`wanted` is None) is drawn by the resident nodes that hide it.
fn covers_in(
    entries: &[CellCutEntry],
    position: &FastMap<NodeId, usize>,
    ancestor: NodeId,
    f: &Footprint,
    gap: u8,
) -> bool {
    let Some(&index) = position.get(&ancestor) else {
        return false;
    };
    let parent = &entries[index];
    f.octants_in(gap)
        .all(|o| parent.drawn[o].is_some() || parent.wanted[o].is_none())
}

/// FNV-1a digest of the drawn depths of a cut, in order.
fn drawn_digest(entries: &[CellCutEntry]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut feed = |byte: u8| hash = (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
    for entry in entries {
        entry
            .node
            .get()
            .to_le_bytes()
            .into_iter()
            .for_each(&mut feed);
        for drawn in entry.drawn {
            feed(drawn.map_or(0, |depth| depth + 1));
        }
    }
    hash
}

/// Per cell, the things (nodes or entry indices) under it and how many levels below.
type Below<T> = FastMap<(u8, [u64; 3]), Vec<(T, u8)>>;

/// The nodes of the applied cut, as the fallback of R3-Q19 needs them.
struct AppliedIndex {
    nodes: FastMap<NodeId, PoolNode>,
    /// `(representative depth, first cell)` -> node, like `cell_cut` finds ancestors.
    cells: FastMap<(u8, [u64; 3]), NodeId>,
    /// Per cell, the applied nodes under it (up to `MAX_MASK_LEVELS` levels below) and how many
    /// levels below: who drew a zone before a coarser node took it over.
    below: Below<NodeId>,
    grid: Option<CellGrid>,
    /// Bumped when the set of nodes (or their data) changes.
    epoch: u64,
}

/// Fallback ancestors for one cut, valid for one cut, residency, caps and applied set.
struct FallbackMemo {
    cut: Vec<NodeId>,
    caps: FastMap<NodeId, u8>,
    residency_epoch: u64,
    applied_epoch: u64,
    grid: CellGrid,
    hold_digest: u64,
    applied_digest: u64,
    found: Fallbacks,
}

/// Where a node sits in the grid, as `cell_cut` places it.
struct Footprint {
    rep: u8,
    lo: [u64; 3],
    hi: [u64; 3],
}

impl Footprint {
    fn new(grid: &CellGrid, bounds: SpatialBounds) -> Self {
        let rep = representative_depth(grid, bounds);
        let range: [(u64, u64); 3] =
            std::array::from_fn(|axis| node_range(grid, bounds, rep, axis));
        Self {
            rep,
            lo: range.map(|(lo, _)| lo),
            hi: range.map(|(_, hi)| hi),
        }
    }

    /// Ancestor gaps that can exist and be masked.
    fn gaps(&self) -> std::ops::RangeInclusive<u8> {
        1..=self.rep.saturating_sub(3).min(MAX_MASK_LEVELS)
    }

    /// Cell key of the ancestor `k` levels up.
    fn ancestor(&self, k: u8) -> (u8, [u64; 3]) {
        (self.rep - k, self.lo.map(|c| (c >> k) & !7))
    }

    /// Octants of the ancestor `k` levels up that this node's span touches.
    fn octants_in(&self, k: u8) -> impl Iterator<Item = usize> {
        self.octants_span(k, self.lo, self.hi)
    }

    /// Octants of the ancestor `k` levels up that the given `octant` of this node touches.
    fn octants_of(&self, k: u8, octant: usize) -> impl Iterator<Item = usize> {
        let (lo, hi) = self.octant_range(octant);
        self.octants_span(k, lo, hi)
    }

    /// Cells (inclusive range, at this node's depth) of the given `octant` of this node.
    fn octant_range(&self, octant: usize) -> ([u64; 3], [u64; 3]) {
        let (mut lo, mut hi) = (self.lo, self.hi);
        for axis in 0..3 {
            let size = self.hi[axis] - self.lo[axis] + 1;
            if size >= 2 {
                let middle = self.lo[axis] + size / 2;
                if octant >> (2 - axis) & 1 == 1 {
                    lo[axis] = middle;
                } else {
                    hi[axis] = middle - 1;
                }
            }
        }
        (lo, hi)
    }

    fn octants_span(
        &self,
        k: u8,
        lo: [u64; 3],
        hi: [u64; 3],
    ) -> impl Iterator<Item = usize> + use<> {
        let base = self.ancestor(k).1;
        let span: [(usize, usize); 3] = std::array::from_fn(|axis| {
            (
                ((lo[axis] >> k) - base[axis]).min(7) as usize,
                ((hi[axis] >> k) - base[axis]).min(7) as usize,
            )
        });
        (span[0].0 >> 2..=span[0].1 >> 2).flat_map(move |x| {
            (span[1].0 >> 2..=span[1].1 >> 2).flat_map(move |y| {
                (span[2].0 >> 2..=span[2].1 >> 2).map(move |z| x << 2 | y << 1 | z)
            })
        })
    }
}

enum LevelState {
    Requested,
    Uploading { slots: Vec<u32>, pages_done: usize },
    Resident { slots: Vec<u32> },
}

struct Level {
    state: LevelState,
    /// Shared with the owner of the data (`CellDriver`), never copied.
    data: Option<Arc<PackedLevel>>,
    last_used: u64,
    /// Last frame this level was in `active`; 0 when never.
    last_active: u64,
    /// Recoloured data of a resident level (same cells, other colours): it is written over the
    /// level's own page slots, so the level keeps drawing without a gap.
    refresh: Option<Arc<PackedLevel>>,
}

impl Level {
    fn pages(&self) -> Option<usize> {
        self.data.as_ref().map(|data| data.pages.len())
    }

    fn is_resident(&self) -> bool {
        matches!(self.state, LevelState::Resident { .. })
    }
}

/// Priority of a level to upload: nodes without anything to draw first, then by node distance.
#[derive(Clone, Copy)]
struct Need {
    first: bool,
    /// The adjacent level of a stepping octant (ADR 0053): right after `first`.
    step: bool,
    distance: f64,
}

/// A stepping octant's wait state: `((drawn depth, wanted depth is finer, adjacent depth
/// awaited), frames waited)`.
type StepWait = ((u8, bool, u8), u32);

pub struct CellPool {
    config: PoolConfig,
    frame: u64,
    free: BTreeSet<u32>,
    /// `(free_frame, slots)`: released at the start of that frame.
    pending: Vec<(u64, Vec<u32>)>,
    levels: FastMap<LevelKey, Level>,
    caps: FastMap<NodeId, u8>,
    /// Exact page counts given before the data arrives.
    sizes: FastMap<LevelKey, usize>,
    sizes_prune_at: usize,
    /// Bumped when the size the fit uses for some level changes.
    size_epoch: u64,
    /// Bumped when a level becomes resident or leaves residency.
    residency_epoch: u64,
    depths: FastMap<NodeId, NodeDepths>,
    last_pose: Option<Pose>,
    depth_key: Option<ViewKey>,
    wanted_memo: Option<WantedMemo>,
    wanted_generation: u64,
    real_memo: Option<RealMemo>,
    overflow: u32,
    applied: Vec<CellCutEntry>,
    applied_index: AppliedIndex,
    fallback_memo: Option<FallbackMemo>,
    /// R3-Q21: cut nodes held back in the last frame.
    hold: FastSet<NodeId>,
    /// Consecutive frames each held or excluded node has been waiting.
    wait_ages: FastMap<NodeId, u32>,
    /// Passes of the hold search per frame (`HOLD_PASSES`; the tests lower it).
    hold_passes: usize,
    /// Nodes whose finer level (above the fit cap) stopped drawing because the capped level
    /// could not get pages, with the cap at that moment (see `finer_drawn`).
    old_released: FastMap<NodeId, u8>,
    /// Drawn depths per octant of each node of the applied cut, and their digest.
    applied_drawn: FastMap<NodeId, [Option<u8>; 8]>,
    /// Cell mode: the finest depth active in each node of the applied cut. The GPU picks the
    /// drawing depth per cell among the active ones, so it can exceed the octant's `applied_drawn`
    /// depth wherever a finer active depth (another octant's, or D5) still refines.
    applied_finest: FastMap<NodeId, u8>,
    /// Per `(node, octant)` stepping towards its wanted depth: `((drawn depth, wanted depth is
    /// finer, adjacent depth awaited), frames waited)`. Every planned frame counts, applied or
    /// retained; the key ignores the exact wanted depth, so a wanted depth flipping within the
    /// same direction does not restart the wait, and a hop (a new drawn depth) does. At
    /// `STEP_WAIT_FRAMES` the octant hops to the resident level closest to its drawn depth
    /// (ADR 0053).
    step_waits: FastMap<(NodeId, usize), StepWait>,
    /// Cell mode: the D5 depth each masked node had active at the last applied frame. Retained
    /// frames keep it (the caps may drop below it, but the GPU record still holds the mask).
    applied_d5: FastMap<NodeId, u8>,
    applied_digest: u64,
    /// `RealMemo::hold_ancestors` of the applied cut.
    applied_cover: FastSet<NodeId>,
    carry: Carry,
    /// RF-05 continuity: no octant draws cells smaller than this side (dataset frame units);
    /// `None` is the literal mode.
    min_side: Option<f64>,
    /// Last frame that counted an overflow (a frame can plan more than once).
    overflow_frame: u64,
    /// `RealMemo::covered_octants` of the applied cut.
    applied_covered: u32,
    applied_levels: FastSet<LevelKey>,
    applied_pins: FastSet<LevelKey>,
    pinned: FastSet<LevelKey>,
    mask_slots: FastMap<NodeId, MaskSlot>,
    /// Levels whose upload completed in the current plan: a level is first drawn one frame after
    /// its last page is written, so cell mode does not activate them yet (the D5 depth included).
    fresh: FastSet<LevelKey>,
    lod_mode: LodMode,
    free_masks: BTreeSet<u32>,
    next_mask: u32,
    events: Vec<CellPoolEvent>,
}

/// What a node's mask slot last sent to the GPU.
struct MaskSlot {
    slot: u32,
    mask: [u64; 8],
    active: u64,
    node_depth: u8,
    /// A mode change: send the record again whatever it holds.
    stale: bool,
}

/// `ceil(min(points, 8^(depth - node_depth)) / 512)` pages, at least one.
fn estimate_pages(points: u64, depth: u8, node_depth: u8) -> usize {
    let gap = u32::from(depth.saturating_sub(node_depth));
    let cells = if gap >= 21 {
        u64::MAX
    } else {
        1_u64 << (3 * gap)
    };
    (points.min(cells).div_ceil(PAGE_SLOTS as u64)).max(1) as usize
}

fn distance_sq(eye: [f64; 3], bounds: SpatialBounds, origin: [f64; 3]) -> f64 {
    (0..3)
        .map(|a| {
            let d = eye[a].clamp(bounds.min()[a] - origin[a], bounds.max()[a] - origin[a]) - eye[a];
            d * d
        })
        .sum()
}

/// `NodeDepths::depth` of a node.
fn effective_depth(pose: Pose, bounds: SpatialBounds, origin: [f64; 3]) -> f64 {
    let offset: [f64; 3] = std::array::from_fn(|a| {
        pose.eye[a].clamp(bounds.min()[a] - origin[a], bounds.max()[a] - origin[a]) - pose.eye[a]
    });
    let depth: f64 = (0..3).map(|a| offset[a] * pose.forward[a]).sum();
    let distance = offset.iter().map(|v| v * v).sum::<f64>().sqrt();
    if depth <= 0.0 {
        0.0
    } else {
        depth * depth / distance
    }
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f64>().sqrt()
}

/// Everything one frame computes from the arguments of `frame`.
struct Ctx<'a> {
    grid: &'a CellGrid,
    origin: [f64; 3],
    depths: &'a FastMap<NodeId, NodeDepths>,
    evaluations: Cell<u32>,
}

impl Ctx<'_> {
    /// `octant_wanted_depth` of the octant for `data.buildable`, from the cache of the full
    /// range: the coarsest depth that meets the target, else the finest, so a range cut short by
    /// a cap gets the same depth capped at its finest.
    fn depth(&self, id: NodeId, octant: usize, data: &CellCutNode) -> u8 {
        self.depths[&id].depths[octant].min(data.buildable.1)
    }

    /// `node_far_depth` of the node (cell mode), capped like `depth`.
    fn far(&self, id: NodeId, data: &CellCutNode) -> (u8, u8) {
        let (far, front) = self.depths[&id].far;
        (far.min(data.buildable.1), front)
    }
}

impl CellPool {
    /// What `id` drew in the last applied cut and which of its octants gave up stepping.
    fn prev_state(&self, id: NodeId) -> PrevState {
        let mut state = PrevState {
            drawn: self.applied_drawn.get(&id).copied().unwrap_or([None; 8]),
            timed_out: [false; 8],
        };
        for (o, timed_out) in state.timed_out.iter_mut().enumerate() {
            *timed_out = self
                .step_waits
                .get(&(id, o))
                .is_some_and(|(_, frames)| *frames >= STEP_WAIT_FRAMES);
        }
        state
    }

    /// `prev_state`, except for a node that drew nothing in the applied cut: in cell mode its
    /// `prev` is the finest depth its drawing ancestor drew over its footprint (or, when it
    /// replaces drawn descendants, the coarsest depth they drew), clamped to its buildable range
    /// inside the span. The stepping rule then moves it one level from there, so it starts one
    /// level finer than its ancestor (one level coarser than its descendants) and goes on one
    /// level per frame (ADR 0053).
    fn prev_state_seeded(
        &self,
        id: NodeId,
        bounds: SpatialBounds,
        buildable: (u8, u8),
        grid: &CellGrid,
    ) -> PrevState {
        let mut state = self.prev_state(id);
        if self.lod_mode != LodMode::Cell || self.drew_in_applied(id) {
            return state;
        }
        let (lo, hi) = cell_depth_range(Self::node_depth(grid, bounds));
        let (lo, hi) = (lo.max(buildable.0), hi.min(buildable.1));
        if lo > hi {
            return state;
        }
        let seed = self.entering_seed(bounds, grid);
        for (slot, seed) in state.drawn.iter_mut().zip(seed) {
            *slot = seed.map(|depth| depth.clamp(lo, hi));
        }
        state
    }

    /// Whether `id` drew anything in the applied cut (a node held with nothing resident did not).
    fn drew_in_applied(&self, id: NodeId) -> bool {
        self.applied_drawn
            .get(&id)
            .is_some_and(|drawn| drawn.iter().any(Option::is_some))
    }

    /// Per octant, the depth a node entering the cut starts from (see `prev_state_seeded`).
    fn entering_seed(&self, bounds: SpatialBounds, grid: &CellGrid) -> [Option<u8>; 8] {
        let mut seed = [None; 8];
        let index = &self.applied_index;
        if index.grid.as_ref() != Some(grid) {
            return seed;
        }
        let f = Footprint::new(grid, bounds);
        for gap in f.gaps() {
            let Some(&ancestor) = index.cells.get(&f.ancestor(gap)) else {
                continue;
            };
            let Some(drawn) = self.applied_drawn.get(&ancestor) else {
                continue;
            };
            for (o, slot) in seed.iter_mut().enumerate() {
                *slot = f
                    .octants_of(gap, o)
                    .filter_map(|a| self.drawn_ceiling(ancestor, drawn[a]))
                    .max();
            }
            if seed.iter().any(Option::is_some) {
                return seed;
            }
        }
        for (below, gap) in index.below.get(&(f.rep, f.lo)).into_iter().flatten() {
            let Some(drawn) = self.applied_drawn.get(below) else {
                continue;
            };
            let df = Footprint::new(grid, index.nodes[below].bounds);
            for (o, depth) in drawn.iter().enumerate() {
                for a in df.octants_of(*gap, o) {
                    if let Some(depth) = *depth {
                        seed[a] = Some(seed[a].map_or(depth, |was: u8| was.min(depth)));
                    }
                }
            }
        }
        seed
    }

    /// Octants of `entries` (a cut about to be applied) that jump more than one level from what
    /// they drew because they timed out or their previous level is not resident any more.
    fn step_hops(&self, entries: &[CellCutEntry]) -> u32 {
        let mut hops = 0;
        for entry in entries {
            let prev = self.prev_state(entry.node);
            for o in 0..8 {
                if let (Some(was), Some(now)) = (prev.drawn[o], entry.drawn[o])
                    && was.abs_diff(now) > 1
                    && (prev.timed_out[o] || !self.is_resident(self.key(entry.node, was)))
                {
                    hops += 1;
                }
            }
        }
        hops
    }

    pub fn new(config: PoolConfig) -> Self {
        Self {
            config,
            frame: 0,
            free: (0..config.capacity_pages).collect(),
            pending: Vec::new(),
            levels: FastMap::default(),
            caps: FastMap::default(),
            sizes: FastMap::default(),
            sizes_prune_at: 1024,
            size_epoch: 0,
            residency_epoch: 0,
            depths: FastMap::default(),
            last_pose: None,
            depth_key: None,
            wanted_memo: None,
            wanted_generation: 0,
            real_memo: None,
            overflow: 0,
            applied: Vec::new(),
            applied_index: AppliedIndex {
                nodes: FastMap::default(),
                cells: FastMap::default(),
                below: FastMap::default(),
                grid: None,
                epoch: 0,
            },
            fallback_memo: None,
            hold: FastSet::default(),
            wait_ages: FastMap::default(),
            applied_drawn: FastMap::default(),
            applied_finest: FastMap::default(),
            step_waits: FastMap::default(),
            applied_d5: FastMap::default(),
            applied_digest: 0,
            applied_cover: FastSet::default(),
            hold_passes: HOLD_PASSES,
            old_released: FastMap::default(),
            carry: Carry::default(),
            min_side: None,
            overflow_frame: 0,
            applied_covered: 0,
            applied_levels: FastSet::default(),
            applied_pins: FastSet::default(),
            pinned: FastSet::default(),
            mask_slots: FastMap::default(),
            fresh: FastSet::default(),
            lod_mode: LodMode::Octant,
            free_masks: BTreeSet::new(),
            next_mask: 0,
            events: Vec::new(),
        }
    }

    /// Continuity (RF-05, ADR 0019): from now on each octant wants at most the finest stored
    /// level whose cell side is not below `min_side` (the coarsest stored level when every one is
    /// finer). The cap is part of the wanted depth, so a level coarser than the target only
    /// because of it is not a stage (R3-Q3) and the fit never counts it as coarsened. Cells are
    /// not rebuilt: the next frame only selects other levels. `None` or a non-positive or
    /// non-finite side restores the literal mode.
    pub fn set_min_side(&mut self, min_side: Option<f64>) {
        self.min_side = min_side.filter(|side| side.is_finite() && *side > 0.0);
    }

    /// Chooses how nodes draw their levels (`LodMode`). A change drops the memos of the cuts
    /// planned under the old mode and marks every mask slot stale, so the next plan sends a
    /// fresh `MaskUpdate` for each node; no level is rebuilt or re-uploaded.
    pub fn set_lod_mode(&mut self, mode: LodMode) {
        if self.lod_mode == mode {
            return;
        }
        self.lod_mode = mode;
        self.wanted_memo = None;
        self.real_memo = None;
        // The cached far depths are only computed in cell mode.
        self.depths.clear();
        for slot in self.mask_slots.values_mut() {
            slot.stale = true;
        }
    }

    pub fn lod_mode(&self) -> LodMode {
        self.lod_mode
    }

    /// Node depth of `bounds`: the depth whose levels span its octants.
    fn node_depth(grid: &CellGrid, bounds: SpatialBounds) -> u8 {
        representative_depth(grid, bounds).saturating_sub(3)
    }

    /// The stored depths of a node that can draw: all of them in octant mode, those inside the
    /// node's span in cell mode (E2). Cell-mode callers only: octant callers use `stored` itself.
    fn drawable_stored(&self, stored: &[u8], grid: &CellGrid, bounds: SpatialBounds) -> Vec<u8> {
        match self.lod_mode {
            LodMode::Octant => stored.to_vec(),
            LodMode::Cell => in_cell_range(stored, Self::node_depth(grid, bounds)),
        }
    }

    /// Cell mode: the depths of `entry` emitted as active levels: the drawn ones that are
    /// resident and inside the node's span (a retained cut may hold depths of another mode or
    /// span), the resident depths of its range that reach one of them without a residency gap,
    /// plus the node's D5 depth when its mask is set (see `d5_active`).
    fn cell_active_depths(
        &self,
        entry: &CellCutEntry,
        node_depth: u8,
        new_d5: Option<u8>,
    ) -> Vec<u8> {
        // The depths the GPU may pick from are the ones the octants draw plus the resident depths
        // of the node's range that reach a drawn one without a residency gap (ADR 0055,
        // proposed). A range depth beyond a gap is never active: the per-cell rule would jump
        // to it, drawing far cells below the pixel target (a finer one) or coarsening what is
        // on screen (a coarser one). `range_depths` requests the missing depths, so the set
        // grows as they arrive.
        let drawn = entry.required_levels();
        let usable = |depth: u8| {
            let key = self.key(entry.node, depth);
            self.is_resident(key) && !self.fresh.contains(&key)
        };
        let mut wanted = drawn.clone();
        if let (Some((lo, hi)), Some(first), Some(last)) =
            (entry.range, drawn.first(), drawn.last())
        {
            let (lo, hi) = (lo.min(*first), hi.max(*last));
            // A drawn depth that is not usable yet (fresh) anchors nothing: its neighbours would
            // form a set with a gap at it.
            for anchor in drawn.iter().filter(|depth| usable(**depth)) {
                wanted.extend((lo..*anchor).rev().take_while(|depth| usable(*depth)));
                wanted.extend((*anchor + 1..=hi).take_while(|depth| usable(*depth)));
            }
            wanted.sort_unstable();
            wanted.dedup();
        }
        let mut depths: Vec<u8> = in_cell_range(&wanted, node_depth)
            .into_iter()
            .filter(|depth| usable(*depth))
            .collect();
        if let Some(d5) = new_d5
            && !depths.contains(&d5)
        {
            depths.push(d5);
            depths.sort_unstable();
        }
        depths
    }

    /// D5 (E7): a node whose mask is set also has its D5 depth active, which covers the unmasked
    /// part of its coarse cells. `snapshot` reads the depth held at the last applied frame (it
    /// is still resident), which is what the GPU record holds on a retained frame: the caps
    /// may have dropped below it since. Otherwise it is computed from the current caps.
    fn d5_active(&self, entry: &CellCutEntry, snapshot: bool) -> Option<u8> {
        if entry.mask.iter().all(|word| *word == 0) {
            return None;
        }
        let d5 = if snapshot {
            self.applied_d5.get(&entry.node).copied()
        } else {
            let node = self.applied_index.nodes.get(&entry.node)?;
            let grid = self.applied_index.grid.as_ref()?;
            self.d5_resident(entry.node, node, grid)
        }?;
        (self.is_resident(self.key(entry.node, d5))
            && !self.fresh.contains(&self.key(entry.node, d5)))
        .then_some(d5)
    }

    /// The depth the applied cut may have drawn over an octant of `id` where it drew `drawn`: in
    /// cell mode up to the finest active depth of the node (R3-Q21 needs what was actually drawn,
    /// not the octant's nominal depth).
    fn drawn_ceiling(&self, id: NodeId, drawn: Option<u8>) -> Option<u8> {
        match self.lod_mode {
            LodMode::Octant => drawn,
            LodMode::Cell => {
                drawn.map(|depth| depth.max(self.applied_finest.get(&id).copied().unwrap_or(depth)))
            }
        }
    }

    /// Stored depths of `node` that cell mode can use (inside its span, under the fit cap).
    fn d5_stored(&self, id: NodeId, node: &PoolNode, grid: &CellGrid) -> Vec<u8> {
        self.drawable_stored(&capped(&node.stored, self.caps.get(&id)), grid, node.bounds)
    }

    /// The D5 depth of a node (E7): its smallest resident depth `>= rep` (`node_depth + 3`)
    /// inside the cell span.
    fn d5_resident(&self, id: NodeId, node: &PoolNode, grid: &CellGrid) -> Option<u8> {
        let rep = representative_depth(grid, node.bounds);
        self.d5_stored(id, node, grid)
            .into_iter()
            .find(|depth| *depth >= rep && self.is_resident(self.key(id, *depth)))
    }

    /// The depth to request as the D5 depth of a node: the smallest stored depth `>= rep` and,
    /// when none is stored, the first buildable one inside the span.
    fn d5_wanted(&self, id: NodeId, node: &PoolNode, grid: &CellGrid) -> Option<u8> {
        let rep = representative_depth(grid, node.bounds);
        self.d5_stored(id, node, grid)
            .into_iter()
            .find(|depth| *depth >= rep)
            .or_else(|| {
                let buildable = capped_range(node.buildable, self.caps.get(&id));
                let (lo, hi) = cell_depth_range(Self::node_depth(grid, node.bounds));
                let first = rep.max(lo).max(buildable.0);
                (first <= hi.min(buildable.1)).then_some(first)
            })
    }

    /// The coarsest buildable depth inside the node's span, which cell mode requests (and pins)
    /// when no stored level of the node can draw.
    fn coarsest_in_span(
        &self,
        buildable: (u8, u8),
        grid: &CellGrid,
        bounds: SpatialBounds,
    ) -> Option<u8> {
        let (lo, hi) = cell_depth_range(Self::node_depth(grid, bounds));
        let lo = lo.max(buildable.0);
        (lo <= hi.min(buildable.1)).then_some(lo)
    }

    pub fn min_side(&self) -> Option<f64> {
        self.min_side
    }

    pub fn key(&self, node: NodeId, depth: u8) -> LevelKey {
        LevelKey {
            session: self.config.session,
            node,
            depth,
            color_gen: self.config.color_gen,
        }
    }

    pub fn is_resident(&self, key: LevelKey) -> bool {
        self.levels.get(&key).is_some_and(Level::is_resident)
    }

    /// Packed data of a requested, uploading or resident level, for the GPU glue.
    pub fn level(&self, key: LevelKey) -> Option<&PackedLevel> {
        self.levels.get(&key)?.data.as_deref()
    }

    /// Mask slots that no node of the applied cut holds: their record is dead until a node
    /// takes the slot again.
    pub fn free_mask_slots(&self) -> impl Iterator<Item = u32> + '_ {
        self.free_masks.iter().copied()
    }

    pub fn resident_pages(&self) -> u32 {
        self.levels
            .values()
            .filter(|level| level.is_resident())
            .filter_map(Level::pages)
            .sum::<usize>() as u32
    }

    /// Delivers the data of a requested level. A level nobody wants any more, one that already has
    /// data, an empty one (no pages) or one larger than the pool is dropped (`false`). An `Arc`
    /// is shared, not copied.
    pub fn offer(&mut self, key: LevelKey, level: impl Into<Arc<PackedLevel>>) -> bool {
        let level = level.into();
        let capacity = self.config.capacity_pages as usize;
        match self.levels.get(&key) {
            Some(entry)
                if matches!(entry.state, LevelState::Requested)
                    && entry.data.is_none()
                    && !level.pages.is_empty()
                    && level.pages.len() <= capacity =>
            {
                let before = self.known_pages(key);
                let pages = level.pages.len();
                if let Some(entry) = self.levels.get_mut(&key) {
                    entry.data = Some(level);
                }
                if before != Some(pages) {
                    self.size_epoch += 1;
                }
                true
            }
            _ => false,
        }
    }

    /// Replaces the data of a level with a recoloured copy of the same cells (colour generation
    /// change, R3-Q33). Same page count is a precondition (`false` otherwise, nothing changes).
    /// A level the pool does not hold needs nothing: it is requested from the new data. A level
    /// not resident yet restarts its upload from the new data; a resident one keeps drawing the
    /// old colours until the next frame writes the new ones over its page slots (see
    /// `refresh_uploads`): the page table does not change, so no gap opens.
    pub fn refresh_level(&mut self, key: LevelKey, level: impl Into<Arc<PackedLevel>>) -> bool {
        let level = level.into();
        let Some(entry) = self.levels.get_mut(&key) else {
            return true;
        };
        if entry
            .pages()
            .is_some_and(|pages| pages != level.pages.len())
        {
            return false;
        }
        match &mut entry.state {
            LevelState::Requested => {
                if entry.data.is_some() {
                    entry.data = Some(level);
                }
            }
            LevelState::Uploading { pages_done, .. } => {
                *pages_done = 0;
                entry.data = Some(level);
            }
            LevelState::Resident { .. } => entry.refresh = Some(level),
        }
        true
    }

    /// Writes the pending recolours over the page slots of their resident levels, coarse to
    /// fine and one node whole at a time (a node never draws old and new colours together). The
    /// budget is checked before each node, so a node larger than what is left still goes in one
    /// frame. Returns the pages written.
    fn refresh_uploads(&mut self, budget: &mut usize, uploads: &mut Vec<PageUpload>) -> usize {
        let mut nodes: Vec<(u8, NodeId)> = Vec::new();
        for (key, level) in &self.levels {
            if level.refresh.is_some() {
                match nodes.iter_mut().find(|(_, id)| *id == key.node) {
                    Some((depth, _)) => *depth = (*depth).min(key.depth),
                    None => nodes.push((key.depth, key.node)),
                }
            }
        }
        nodes.sort();
        let mut written = 0;
        for (_, node) in nodes {
            if *budget == 0 {
                break;
            }
            let mut keys: Vec<LevelKey> = self
                .levels
                .iter()
                .filter(|(key, level)| key.node == node && level.refresh.is_some())
                .map(|(key, _)| *key)
                .collect();
            keys.sort();
            for key in keys {
                let level = self.levels.get_mut(&key).expect("level");
                let (Some(data), LevelState::Resident { slots }) =
                    (level.refresh.take(), &level.state)
                else {
                    continue;
                };
                let pages = data.pages.len();
                uploads.push(PageUpload {
                    key,
                    slots: slots.clone(),
                    page_range: 0..pages,
                });
                level.data = Some(data);
                *budget = budget.saturating_sub(pages);
                written += pages;
            }
        }
        written
    }

    /// Exact page counts of the levels of `node` as `(depth, pages)`, known before any upload. The
    /// fit uses them instead of the estimate until the data itself arrives; they must equal the
    /// size `pack_level` gives. Sizes of nodes that leave the cut and hold no level are dropped
    /// eventually.
    pub fn set_level_sizes(&mut self, node: NodeId, sizes: &[(u8, usize)]) {
        for (depth, pages) in sizes {
            let key = self.key(node, *depth);
            let before = self.known_pages(key);
            self.sizes.insert(key, (*pages).max(1));
            if self.known_pages(key) != before {
                self.size_epoch += 1;
            }
        }
    }

    /// The size of a level when known: its data, else its exact size.
    pub(crate) fn known_pages(&self, key: LevelKey) -> Option<usize> {
        self.levels
            .get(&key)
            .and_then(Level::pages)
            .or_else(|| self.sizes.get(&key).copied())
    }

    /// Drops a resident level that is neither pinned nor drawn by the applied (or retained) cut
    /// (device loss, tests, memory pressure from outside). Its pages return to the free list two
    /// frames after it last drew; a still-wanted level is requested again by the next frame.
    /// Other levels stay (`false`).
    pub fn evict(&mut self, key: LevelKey) -> bool {
        let drawn = self
            .applied
            .iter()
            .any(|entry| entry.node == key.node && entry.required_levels().contains(&key.depth));
        if drawn || self.pinned.contains(&key) || !self.is_resident(key) {
            return false;
        }
        self.evict_resident(key, self.frame);
        true
    }

    /// Empties the pool (device lost, or the renderer dropped its pool): no residency, no
    /// requests, no cut. The caller's GPU side is gone and starts fresh, so every page slot is free
    /// at once (no `page_frees`, no delayed release) and the mask slots restart from 0, like a new
    /// renderer hands them out; the next applied cut rewrites every mask. Returns the events of
    /// the levels that left: `Evict` for every resident or uploading level, `Cancel` for every
    /// pending request, after any event still queued. Page sizes given by `set_level_sizes` stay:
    /// they describe the data, not the residency. The next frames request again what the cut
    /// needs.
    #[must_use]
    pub fn reset(&mut self) -> Vec<CellPoolEvent> {
        let mut keys: Vec<LevelKey> = self.levels.keys().copied().collect();
        keys.sort();
        for key in keys {
            let Some(level) = self.levels.remove(&key) else {
                continue;
            };
            self.events.push(match level.state {
                LevelState::Requested => CellPoolEvent::Cancel(key),
                LevelState::Uploading { .. } | LevelState::Resident { .. } => {
                    CellPoolEvent::Evict(key)
                }
            });
        }
        self.free = (0..self.config.capacity_pages).collect();
        self.pending.clear();
        self.size_epoch += 1;
        self.residency_epoch += 1;
        self.caps.clear();
        self.wanted_memo = None;
        self.real_memo = None;
        self.fallback_memo = None;
        self.hold.clear();
        self.wait_ages.clear();
        self.applied_cover.clear();
        self.overflow_frame = 0;
        self.applied_drawn.clear();
        self.applied_finest.clear();
        self.step_waits.clear();
        self.applied_d5.clear();
        self.old_released.clear();
        self.applied_digest = 0;
        self.carry = Carry::default();
        self.applied.clear();
        self.applied_covered = 0;
        self.applied_levels.clear();
        self.applied_pins.clear();
        self.pinned.clear();
        self.applied_index.nodes.clear();
        self.applied_index.cells.clear();
        self.applied_index.below.clear();
        self.applied_index.epoch += 1;
        self.mask_slots.clear();
        self.free_masks.clear();
        self.next_mask = 0;
        std::mem::take(&mut self.events)
    }

    /// Evicts a resident level: its pages are free at `last_active + FREE_DELAY` (never before
    /// `now`), or at `now` when it never drew. A level still uploading is not evicted but
    /// cancelled by `plan`, and its pages are free that same frame (nothing drew them).
    fn evict_resident(&mut self, key: LevelKey, now: u64) {
        let before = self.known_pages(key);
        let Some(level) = self.levels.remove(&key) else {
            return;
        };
        if before != self.known_pages(key) {
            self.size_epoch += 1;
        }
        if let LevelState::Resident { slots } = level.state {
            self.residency_epoch += 1;
            let free_at = if level.last_active == 0 {
                now
            } else {
                (level.last_active + FREE_DELAY).max(now)
            };
            self.pending.push((free_at, slots));
        }
        self.events.push(CellPoolEvent::Evict(key));
    }

    fn release(&mut self, frame: u64, page_frees: &mut Vec<u32>) {
        let mut kept = Vec::new();
        for (at, slots) in std::mem::take(&mut self.pending) {
            if at <= frame {
                page_frees.extend(&slots);
                self.free.extend(slots);
            } else {
                kept.push((at, slots));
            }
        }
        self.pending = kept;
    }

    fn pending_pages(&self) -> usize {
        self.pending.iter().map(|(_, slots)| slots.len()).sum()
    }

    /// Pending pages that are released by `frame`.
    fn pending_pages_by(&self, frame: u64) -> usize {
        self.pending
            .iter()
            .filter(|(at, _)| *at <= frame)
            .map(|(_, slots)| slots.len())
            .sum()
    }

    fn allocated_pages(&self) -> usize {
        self.config.capacity_pages as usize - self.free.len() - self.pending_pages()
    }

    fn usable(&self) -> u64 {
        usable_pages(self.config.capacity_pages)
    }

    fn pages_of(&self, key: LevelKey, node: &PoolNode, grid: &CellGrid) -> usize {
        self.known_pages(key).unwrap_or_else(|| {
            let node_depth = representative_depth(grid, node.bounds).saturating_sub(3);
            estimate_pages(node.points, key.depth, node_depth)
        })
    }

    /// Wanted entries with resident = stored (minus the offenders and below the caps). A
    /// `DescendantTooFine` adds its descendant to `offenders` and runs again.
    fn wanted_cut(
        &self,
        cut: &[NodeId],
        nodes: &FastMap<NodeId, PoolNode>,
        caps: &FastMap<NodeId, u8>,
        offenders: &mut BTreeSet<NodeId>,
        ctx: &Ctx,
    ) -> Result<Vec<CellCutEntry>, CellCutError> {
        loop {
            let lookup = |id: NodeId| {
                let node = nodes.get(&id)?;
                let stored = capped(&node.stored, caps.get(&id));
                let resident = if offenders.contains(&id) {
                    Vec::new()
                } else {
                    stored.clone()
                };
                Some(CellCutNode {
                    bounds: node.bounds,
                    buildable: capped_range(node.buildable, caps.get(&id)),
                    stored,
                    resident,
                })
            };
            ctx.evaluations.set(ctx.evaluations.get() + 1);
            // Cell mode: the entries carry each node's depth range (`range_depths`).
            match cell_cut_ranged(
                cut,
                &lookup,
                ctx.grid,
                self.lod_mode,
                &|id, o, data| ctx.depth(id, o, data),
                Some(&|id, data| ctx.far(id, data)),
                None,
            ) {
                Err(CellCutError::DescendantTooFine { descendant, .. })
                    if !offenders.contains(&descendant) =>
                {
                    offenders.insert(descendant);
                }
                other => return other,
            }
        }
    }

    /// Cell mode: the depths of `node`'s range (`entry.range`) that the pool wants besides the
    /// octant ones: `(stored, missing)`. `stored` are the stored depths inside the range (fetched
    /// and protected); `missing` are the buildable depths not stored, requested on demand but
    /// never deeper than the spacing depth, so the single-point tail is not built for the range.
    /// The spacing depth is the finest stored depth of the node (the chain of the node ends at
    /// the first fully separated level, or where it is too sparse to page), or its
    /// representative depth when nothing is stored. Octant mode has no range: both are empty.
    fn range_depths(
        &self,
        entry: &CellCutEntry,
        node: &PoolNode,
        caps: &FastMap<NodeId, u8>,
        grid: &CellGrid,
    ) -> (Vec<u8>, Vec<u8>) {
        let Some((lo, hi)) = entry.range else {
            return (Vec::new(), Vec::new());
        };
        let cap = caps.get(&entry.node);
        let stored = self.drawable_stored(&capped(&node.stored, cap), grid, node.bounds);
        let spacing = stored
            .last()
            .copied()
            .unwrap_or_else(|| representative_depth(grid, node.bounds));
        let buildable = capped_range(node.buildable, cap);
        let in_range: Vec<u8> = stored
            .iter()
            .copied()
            .filter(|depth| (lo..=hi).contains(depth))
            .collect();
        let missing = in_cell_range(
            &(lo.max(buildable.0)..=hi.min(buildable.1).min(spacing)).collect::<Vec<u8>>(),
            Self::node_depth(grid, node.bounds),
        )
        .into_iter()
        .filter(|depth| !stored.contains(depth))
        .collect::<Vec<u8>>();
        // Contiguous span (ADR 0053): every buildable depth between the coarsest and the finest
        // active one (drawn or resident) is wanted too, below the spacing depth as well.
        // `buildable` already holds the 0.1 mm minimum side and the sparse-page refusals.
        let node_depth = Self::node_depth(grid, node.bounds);
        let (span_lo, span_hi) = cell_depth_range(node_depth);
        // Active depths are the drawn ones and the resident ones inside the range (the pinned
        // coarsest level outside it is not active).
        let active: Vec<u8> = (lo.max(span_lo)..=hi.min(span_hi))
            .filter(|depth| self.is_resident(self.key(entry.node, *depth)))
            .chain(entry.required_levels())
            .collect();
        let mut missing = missing;
        if let (Some(first), Some(last)) = (active.iter().min(), active.iter().max()) {
            for depth in (*first).max(buildable.0)..=(*last).min(buildable.1) {
                if (span_lo..=span_hi).contains(&depth)
                    && !stored.contains(&depth)
                    && !missing.contains(&depth)
                {
                    missing.push(depth);
                }
            }
            missing.sort_unstable();
        }
        (in_range, missing)
    }

    /// Distinct levels the entries need: the wanted ones plus the pinned coarsest of every node.
    fn needed_levels(
        &self,
        entries: &[CellCutEntry],
        nodes: &FastMap<NodeId, PoolNode>,
        caps: &FastMap<NodeId, u8>,
        grid: &CellGrid,
    ) -> FastSet<LevelKey> {
        let mut set = FastSet::default();
        for entry in entries {
            let node = &nodes[&entry.node];
            // Cell mode only sees the stored levels inside the node's span.
            let stored = capped(&node.stored, caps.get(&entry.node));
            let stored = self.drawable_stored(&stored, grid, node.bounds);
            let coarsest = match self.lod_mode {
                LodMode::Octant => coarsest(&node.stored, caps.get(&entry.node)),
                LodMode::Cell => stored.first().copied(),
            };
            if let Some(coarsest) = coarsest {
                set.insert(self.key(entry.node, coarsest));
            }
            // A wanted depth is fetched as the stored one that serves it; when it is not stored
            // itself it is also requested on demand (`requested_levels`).
            if !stored.is_empty() {
                for depth in entry.wanted.iter().flatten() {
                    set.insert(self.key(entry.node, served_depth(&stored, *depth)));
                }
            }
            for depth in self.range_depths(entry, node, caps, grid).0 {
                set.insert(self.key(entry.node, depth));
            }
            for depth in self.seed_depths(entry, node, caps, grid) {
                if stored.contains(&depth) {
                    set.insert(self.key(entry.node, depth));
                }
            }
        }
        set
    }

    /// Cell mode: the depths a node entering the cut starts from (`prev_state_seeded`) and the
    /// one its first step reaches, towards what each octant wants. They are fetched or built with
    /// the wanted depths, so the node can draw them instead of the resident level closest to its
    /// wanted depth.
    fn seed_depths(
        &self,
        entry: &CellCutEntry,
        node: &PoolNode,
        caps: &FastMap<NodeId, u8>,
        grid: &CellGrid,
    ) -> Vec<u8> {
        if self.lod_mode != LodMode::Cell || self.drew_in_applied(entry.node) {
            return Vec::new();
        }
        let buildable = capped_range(node.buildable, caps.get(&entry.node));
        let prev = self.prev_state_seeded(entry.node, node.bounds, buildable, grid);
        let mut depths = Vec::new();
        for (seed, wanted) in prev.drawn.iter().zip(entry.wanted) {
            if let (Some(seed), Some(wanted)) = (*seed, wanted) {
                let step = match wanted.cmp(&seed) {
                    std::cmp::Ordering::Greater => seed + 1,
                    std::cmp::Ordering::Less => seed - 1,
                    std::cmp::Ordering::Equal => seed,
                };
                depths.extend([seed, step]);
            }
        }
        depths.retain(|depth| (buildable.0..=buildable.1).contains(depth));
        depths.sort_unstable();
        depths.dedup();
        depths
    }

    /// Cell mode: the range depths of the entries that are buildable but not stored (see
    /// `range_depths`).
    fn range_requests(
        &self,
        entries: &[CellCutEntry],
        nodes: &FastMap<NodeId, PoolNode>,
        caps: &FastMap<NodeId, u8>,
        grid: &CellGrid,
    ) -> FastSet<LevelKey> {
        let mut set = FastSet::default();
        for entry in entries {
            for depth in self.range_depths(entry, &nodes[&entry.node], caps, grid).1 {
                set.insert(self.key(entry.node, depth));
            }
        }
        set
    }

    /// The wanted depths that are buildable but not stored: the levels to request on demand.
    fn requested_levels(
        &self,
        entries: &[CellCutEntry],
        nodes: &FastMap<NodeId, PoolNode>,
        caps: &FastMap<NodeId, u8>,
        grid: &CellGrid,
    ) -> FastSet<LevelKey> {
        let mut set = self.range_requests(entries, nodes, caps, grid);
        for entry in entries {
            let node = &nodes[&entry.node];
            let stored = capped(&node.stored, caps.get(&entry.node));
            let stored = self.drawable_stored(&stored, grid, node.bounds);
            let buildable = capped_range(node.buildable, caps.get(&entry.node));
            // Cell mode: no stored level can draw, so the coarsest buildable one inside the
            // span is requested first (it is also what the node pins).
            if self.lod_mode == LodMode::Cell
                && stored.is_empty()
                && let Some(depth) = self.coarsest_in_span(buildable, grid, node.bounds)
            {
                set.insert(self.key(entry.node, depth));
            }
            for depth in self.seed_depths(entry, node, caps, grid) {
                if !stored.contains(&depth) {
                    set.insert(self.key(entry.node, depth));
                }
            }
            for depth in entry.wanted.iter().flatten() {
                if !stored.contains(depth) {
                    debug_assert!(
                        (buildable.0..=buildable.1).contains(depth),
                        "a wanted depth is stored or buildable"
                    );
                    set.insert(self.key(entry.node, *depth));
                }
            }
        }
        set
    }

    /// The levels the entries need and their pages.
    fn needed_and_pages(
        &self,
        entries: &[CellCutEntry],
        nodes: &FastMap<NodeId, PoolNode>,
        caps: &FastMap<NodeId, u8>,
        grid: &CellGrid,
        d5: &FastSet<LevelKey>,
    ) -> (FastSet<LevelKey>, u64) {
        let mut needed = self.needed_levels(entries, nodes, caps, grid);
        // Cell mode: the D5 levels of the gated ancestors take pages too, so the fit counts them.
        needed.extend(d5.iter().copied());
        // Cell mode: the range depths requested on demand take pages once built, so the fit
        // counts them at their estimate (they are not in `needed`: nothing to fetch yet).
        let requested = self.range_requests(entries, nodes, caps, grid);
        let total = needed
            .iter()
            .chain(requested.iter().filter(|key| !needed.contains(key)))
            .map(|key| self.pages_of(*key, &nodes[&key.node], grid) as u64)
            .sum();
        (needed, total)
    }

    /// R3-Q11. Returns the levels the wanted entries need after the fit (the stored ones that
    /// serve them), the wanted depths that are not stored (to request), and whether even the coarsest levels overflow the usable
    /// capacity.
    #[allow(clippy::too_many_arguments)]
    fn fit(
        &mut self,
        cut: &[NodeId],
        nodes: &FastMap<NodeId, PoolNode>,
        offenders: &mut BTreeSet<NodeId>,
        eye: [f64; 3],
        ctx: &Ctx,
        d5: &FastSet<LevelKey>,
    ) -> Result<(FastSet<LevelKey>, FastSet<LevelKey>, bool), CellCutError> {
        let usable = self.usable();
        let (high, low) = (usable * HIGH_PERCENT / 100, usable * LOW_PERCENT / 100);
        let mut caps = std::mem::take(&mut self.caps);
        caps.retain(|id, _| nodes.contains_key(id));
        let mut entries = self.wanted_cut(cut, nodes, &caps, offenders, ctx)?;
        let (mut needed, mut total) = self.needed_and_pages(&entries, nodes, &caps, ctx.grid, d5);
        // Lifting the caps needs the uncapped cut to fit under `low`. This runs only when the
        // wanted cut is recomputed (cut, sizes or depths changed), never on a reused frame; sizes
        // are not monotone in depth, so the capped total says nothing about the uncapped one.
        if !caps.is_empty() {
            let none = FastMap::default();
            let open = self.wanted_cut(cut, nodes, &none, offenders, ctx)?;
            let (open_needed, open_total) =
                self.needed_and_pages(&open, nodes, &none, ctx.grid, d5);
            if open_total <= low {
                caps.clear();
                entries = open;
                (needed, total) = (open_needed, open_total);
            }
        }
        if total > high {
            // Farthest first, ordered once; coarsened in blocks of an eighth of the cut.
            let mut order: Vec<NodeId> = cut.to_vec();
            order.sort_by(|a, b| {
                let (da, db) = (
                    distance_sq(eye, nodes[a].bounds, ctx.origin),
                    distance_sq(eye, nodes[b].bounds, ctx.origin),
                );
                db.total_cmp(&da).then(a.cmp(b))
            });
            // `entries` follow the order of `cut`.
            let position: FastMap<NodeId, usize> =
                cut.iter().enumerate().map(|(i, id)| (*id, i)).collect();
            let block = (order.len() / 8).max(1);
            let mut cursor = 0;
            while total > high {
                let mut taken = 0;
                let mut scanned = 0;
                while taken < block && scanned < order.len() {
                    let id = order[cursor % order.len()];
                    cursor += 1;
                    scanned += 1;
                    let entry = &entries[position[&id]];
                    let Some(top) = entry.wanted.iter().flatten().max().copied() else {
                        continue;
                    };
                    let stored = capped(&nodes[&id].stored, caps.get(&id));
                    let stored = self.drawable_stored(&stored, ctx.grid, nodes[&id].bounds);
                    if stored.is_empty() {
                        continue;
                    }
                    let top = served_depth(&stored, top);
                    // E7: a cap never goes below the node's D5 depth (it is protected and its
                    // pages are already counted): below it the gate of its descendants waits for
                    // a depth that is no longer requested (`d5_wanted` reads the capped levels).
                    let floor = d5
                        .iter()
                        .filter(|key| key.node == id)
                        .map(|key| key.depth)
                        .max();
                    // The finest level below `top` that saves pages: smaller, or already needed
                    // (dropping `top` then frees it outright). Estimates can tie, and a tie must
                    // not stop the fit: the cut would overflow with nothing coarsened.
                    let size =
                        |depth: u8| self.pages_of(self.key(id, depth), &nodes[&id], ctx.grid);
                    if let Some(below) = stored
                        .iter()
                        .copied()
                        .filter(|d| {
                            floor.is_none_or(|floor| *d >= floor)
                                && *d < top
                                && (size(*d) < size(top) || needed.contains(&self.key(id, *d)))
                        })
                        .max()
                    {
                        caps.insert(id, below);
                        taken += 1;
                    }
                }
                if taken == 0 {
                    break;
                }
                entries = self.wanted_cut(cut, nodes, &caps, offenders, ctx)?;
                (needed, total) = self.needed_and_pages(&entries, nodes, &caps, ctx.grid, d5);
            }
        }
        self.old_released
            .retain(|id, cap| caps.get(id) == Some(cap));
        let requests = self.requested_levels(&entries, nodes, &caps, ctx.grid);
        self.caps = caps;
        Ok((needed, requests, total > usable))
    }

    /// R3-Q21 under a fit cap: a capped node (`node`: its levels below `cap`) that lacks, in an
    /// octant whose wanted level is requested, a resident level at or above that wanted depth
    /// keeps drawing the levels above the cap it drew in the last applied cut (while resident),
    /// instead of dropping to a coarser resident level until the capped level arrives. It is per
    /// node (the lookup cannot express octants): the kept levels replace the node's resident
    /// list, so every octant of the node draws a finer or equal level than before. The
    /// kept levels are protected as part of the applied cut; once the capped levels are resident
    /// they stop drawing and become evictable. `old_released` ends the stand-in when the capped
    /// level cannot get pages.
    fn finer_drawn(
        &self,
        id: NodeId,
        cap: u8,
        node: &CellCutNode,
        needed: &FastSet<LevelKey>,
        ctx: &Ctx,
    ) -> Option<Vec<u8>> {
        if self.old_released.get(&id) == Some(&cap) {
            return None;
        }
        let drawn = self.applied_drawn.get(&id)?;
        let mut old: Vec<u8> = drawn
            .iter()
            .flatten()
            .copied()
            .filter(|depth| *depth > cap && self.is_resident(self.key(id, *depth)))
            .collect();
        old.sort_unstable();
        old.dedup();
        if old.is_empty() {
            return None;
        }
        // Cell mode reasons about the depths it can use: stored levels and wanted depth are
        // restricted to the node's span, as `cell_cut_with` does.
        let (stored, span) = match self.lod_mode {
            LodMode::Octant => (node.stored.clone(), None),
            LodMode::Cell => {
                let (lo, hi) = cell_depth_range(Self::node_depth(ctx.grid, node.bounds));
                let (lo, hi) = (lo.max(node.buildable.0), hi.min(node.buildable.1));
                (
                    in_cell_range(&node.stored, Self::node_depth(ctx.grid, node.bounds)),
                    (lo <= hi).then_some((lo, hi)),
                )
            }
        };
        if stored.is_empty() {
            return None;
        }
        let lacks = (0..8).any(|octant| {
            let depth = ctx.depth(id, octant, node);
            let depth = span.map_or(depth, |(lo, hi)| depth.clamp(lo, hi));
            let wanted = served_depth(&stored, depth);
            needed.contains(&self.key(id, wanted)) && !node.resident.iter().any(|d| *d >= wanted)
        });
        lacks.then_some(old)
    }

    /// The levels of a node were dropped (a recolour): a fit cap that points at a depth no longer
    /// stored moves to the finest stored depth below it, or goes if there is none.
    pub(crate) fn reconcile_cap(&mut self, id: NodeId, stored: &[u8]) {
        let Some(cap) = self.caps.get(&id).copied() else {
            return;
        };
        if stored.contains(&cap) {
            return;
        }
        // Keep the cap: removing it would let finer levels back in and undo the fit. With no
        // stored depth below it, it clamps to the coarsest one (the node draws that and no finer).
        let below = stored.iter().copied().filter(|depth| *depth < cap).max();
        if let Some(depth) = below.or_else(|| stored.iter().copied().min()) {
            self.caps.insert(id, depth);
        }
        self.size_epoch += 1;
    }

    /// Does any stored level of the node, below its fit cap, have pages resident? In cell mode
    /// only the levels inside the node's span count (E2); octant mode scans `stored` as is.
    fn has_resident_level(&self, id: NodeId, node: &PoolNode, grid: &CellGrid) -> bool {
        let cap = self.caps.get(&id);
        let resident = |depth: &u8| {
            cap.is_none_or(|cap| *depth <= *cap) && self.is_resident(self.key(id, *depth))
        };
        match self.lod_mode {
            LodMode::Octant => node.stored.iter().any(resident),
            LodMode::Cell => self
                .drawable_stored(&node.stored, grid, node.bounds)
                .iter()
                .any(resident),
        }
    }

    /// R3-Q19: for each cut node without a resident level that no cut ancestor with a level
    /// covers, its nearest ancestor (gap <= 3) from the applied cut that has a resident level and
    /// lies at most `MAX_MASK_LEVELS` above every resident cut node under it (a deeper one would
    /// make the real cut fail with `DescendantTooFine`). Levels beyond the fit cap do not count.
    /// R3-Q20: a node with no such ancestor is excluded from the real cut, and the applied
    /// descendants of it that were drawing (gap <= 3) stand in for it until it is resident.
    /// The caps are those of the previous frame's fit; a change retains one frame at most.
    fn fallbacks(
        &mut self,
        cut: &[NodeId],
        nodes: &FastMap<NodeId, PoolNode>,
        grid: &CellGrid,
        hold: &FastSet<NodeId>,
    ) -> Fallbacks {
        let hold_digest = set_digest(hold);
        if let Some(memo) = &self.fallback_memo
            && memo.cut == cut
            && memo.caps == self.caps
            && memo.residency_epoch == self.residency_epoch
            && memo.applied_epoch == self.applied_index.epoch
            && memo.applied_digest == self.applied_digest
            && memo.grid == *grid
            && memo.hold_digest == hold_digest
        {
            return memo.found.clone();
        }
        let found = self.compute_fallbacks(cut, nodes, grid, hold);
        self.fallback_memo = Some(FallbackMemo {
            cut: cut.to_vec(),
            caps: self.caps.clone(),
            residency_epoch: self.residency_epoch,
            applied_epoch: self.applied_index.epoch,
            grid: *grid,
            hold_digest,
            applied_digest: self.applied_digest,
            found: found.clone(),
        });
        found
    }

    fn compute_fallbacks(
        &self,
        cut: &[NodeId],
        nodes: &FastMap<NodeId, PoolNode>,
        grid: &CellGrid,
        hold: &FastSet<NodeId>,
    ) -> Fallbacks {
        let (with_level, missing): (Vec<NodeId>, Vec<NodeId>) = cut.iter().partition(|id| {
            !hold.contains(*id) && self.has_resident_level(**id, &nodes[*id], grid)
        });
        let mut out = Fallbacks::default();
        if missing.is_empty() {
            return out;
        }
        let rule = CoverRule::new(self, cut, &with_level, nodes, grid);
        let mut seen: FastSet<NodeId> = FastSet::default();
        // Held nodes (R3-Q21) with nothing above them but applied descendants that drew there.
        let mut standing: Vec<NodeId> = Vec::new();
        for id in missing {
            let f = Footprint::new(grid, nodes[&id].bounds);
            match rule.cover(self, &f) {
                Cover::Fallback { ancestor, .. } => {
                    if seen.insert(ancestor) {
                        out.ancestors.push(ancestor);
                    }
                }
                Cover::Nothing => {
                    if rule.applied_ok
                        && hold.contains(&id)
                        && self.applied_index.below.contains_key(&(f.rep, f.lo))
                    {
                        standing.push(id);
                    } else {
                        out.excluded.insert(id);
                    }
                }
                Cover::Cut { .. } => {}
            }
        }
        if (!out.excluded.is_empty() || !standing.is_empty()) && rule.applied_ok {
            let cells: FastSet<(u8, [u64; 3])> = out
                .excluded
                .iter()
                .chain(&standing)
                .map(|id| {
                    let f = Footprint::new(grid, nodes[id].bounds);
                    (f.rep, f.lo)
                })
                .collect();
            // Cells that already draw in the real cut: a descendant under one of them, at any
            // distance, would draw the same zone twice or make the mask impossible
            // (`DescendantTooFine`), so it does not stand in.
            let mut shadow = rule.drawing.clone();
            for ancestor in &out.ancestors {
                let f = Footprint::new(grid, self.applied_index.nodes[ancestor].bounds);
                shadow.insert((f.rep, f.lo));
            }
            let mut found: Vec<NodeId> = self
                .applied
                .iter()
                .filter(|entry| {
                    let id = entry.node;
                    !rule.in_cut.contains(&id)
                        && !seen.contains(&id)
                        && entry.drawn.iter().any(Option::is_some)
                        && self.has_resident_level(id, &self.applied_index.nodes[&id], grid)
                        && {
                            let f = Footprint::new(grid, self.applied_index.nodes[&id].bounds);
                            f.gaps().any(|k| cells.contains(&f.ancestor(k)))
                                && !(1..=f.rep.saturating_sub(3))
                                    .any(|k| shadow.contains(&f.ancestor(k)))
                        }
                })
                .map(|entry| entry.node)
                .collect();
            found.sort();
            out.descendants = found;
            for id in standing {
                let f = Footprint::new(grid, nodes[&id].bounds);
                let stands = self
                    .applied_index
                    .below
                    .get(&(f.rep, f.lo))
                    .is_some_and(|list| list.iter().any(|(d, _)| out.descendants.contains(d)));
                if !stands {
                    out.excluded.insert(id);
                }
            }
        }
        out
    }

    /// Remembers the nodes of the applied cut for the fallback; the epoch moves only when the set
    /// or its data changes.
    fn index_applied(&mut self, nodes: &FastMap<NodeId, PoolNode>, grid: &CellGrid) {
        let index = &mut self.applied_index;
        let same = index.grid == Some(*grid)
            && index.nodes.len() == self.applied.len()
            && self
                .applied
                .iter()
                .all(|entry| index.nodes.get(&entry.node) == nodes.get(&entry.node));
        if same {
            return;
        }
        index.epoch += 1;
        index.grid = Some(*grid);
        index.nodes.clear();
        index.cells.clear();
        index.below.clear();
        for entry in &self.applied {
            let data = &nodes[&entry.node];
            let f = Footprint::new(grid, data.bounds);
            for k in f.gaps() {
                index
                    .below
                    .entry(f.ancestor(k))
                    .or_default()
                    .push((entry.node, k));
            }
            index.cells.entry((f.rep, f.lo)).or_insert(entry.node);
            index.nodes.insert(entry.node, data.clone());
        }
    }

    /// R3-Q21: the cut nodes of the real cut `entries` (computed with `hold` as the held set) that
    /// would draw, in some octant, a level below `min(depth what drew there before, wanted
    /// depth)`, judged as `cell_cut` would draw them with their resident levels. What drew there
    /// before is the applied ancestor (`CoverRule`, the one of the fallback) and, on a zoom out,
    /// the applied descendants of the node. A node with a covering ancestor or covering
    /// descendants can be held; one that is already held keeps the hold only while they are in the
    /// real cut. With descendants the wanted depths are those of the node alone: its octants that
    /// they mask in `entries` would otherwise look finished.
    fn eval_hold(
        &self,
        entries: &[CellCutEntry],
        cut: &[NodeId],
        nodes: &FastMap<NodeId, PoolNode>,
        grid: &CellGrid,
        hold: &FastSet<NodeId>,
        wanted_depth: &dyn Fn(NodeId, usize, &CellCutNode) -> u8,
    ) -> HoldEval {
        let mut out = self.eval_refine_hold(entries, cut, nodes, grid, hold, wanted_depth);
        self.add_d5_gate(&mut out, entries, cut, nodes, grid);
        out
    }

    /// E7, cell mode: a cut node that would mask an ancestor of the real cut stays held while
    /// that ancestor draws but has no resident D5 depth, so a partially masked coarse cell never
    /// leaves a hole. It looks at what the node would draw (its resident levels), not at the
    /// mask, so the held set does not depend on itself.
    fn add_d5_gate(
        &self,
        out: &mut HoldEval,
        entries: &[CellCutEntry],
        cut: &[NodeId],
        nodes: &FastMap<NodeId, PoolNode>,
        grid: &CellGrid,
    ) {
        if self.lod_mode != LodMode::Cell {
            return;
        }
        let mut cells: FastMap<(u8, [u64; 3]), NodeId> = FastMap::default();
        for entry in entries {
            let f = Footprint::new(grid, nodes[&entry.node].bounds);
            cells.entry((f.rep, f.lo)).or_insert(entry.node);
        }
        for entry in &entries[..cut.len()] {
            let id = entry.node;
            let data = &nodes[&id];
            if !self.has_resident_level(id, data, grid) {
                continue;
            }
            let f = Footprint::new(grid, data.bounds);
            for gap in f.gaps() {
                let Some(&ancestor) = cells.get(&f.ancestor(gap)) else {
                    continue;
                };
                let a = &nodes[&ancestor];
                if self.has_resident_level(ancestor, a, grid)
                    && self.d5_resident(ancestor, a, grid).is_none()
                {
                    out.hold.insert(id);
                    out.settle.insert(id);
                    out.ancestors.insert(ancestor);
                }
            }
        }
    }

    /// E7: the D5 depth (`d5_wanted`) of each node of `cut` or `ancestors` that has a node of
    /// `cut` under it (gap <= `MAX_MASK_LEVELS`, the span a mask can express).
    fn d5_keys(
        &self,
        cut: &[NodeId],
        ancestors: &[NodeId],
        nodes: &FastMap<NodeId, PoolNode>,
        grid: &CellGrid,
    ) -> FastSet<LevelKey> {
        let mut cells: FastMap<(u8, [u64; 3]), NodeId> = FastMap::default();
        for id in cut.iter().chain(ancestors) {
            let f = Footprint::new(grid, nodes[id].bounds);
            cells.entry((f.rep, f.lo)).or_insert(*id);
        }
        let mut parents: FastSet<NodeId> = FastSet::default();
        for id in cut {
            let f = Footprint::new(grid, nodes[id].bounds);
            parents.extend(f.gaps().filter_map(|gap| cells.get(&f.ancestor(gap))));
        }
        parents
            .into_iter()
            .filter_map(|id| {
                let depth = self.d5_wanted(id, &nodes[&id], grid)?;
                Some(self.key(id, depth))
            })
            .collect()
    }

    /// The R3-Q21 hold ("only refine"), see the module docs.
    fn eval_refine_hold(
        &self,
        entries: &[CellCutEntry],
        cut: &[NodeId],
        nodes: &FastMap<NodeId, PoolNode>,
        grid: &CellGrid,
        hold: &FastSet<NodeId>,
        wanted_depth: &dyn Fn(NodeId, usize, &CellCutNode) -> u8,
    ) -> HoldEval {
        let mut out = HoldEval::default();
        if self.applied_drawn.is_empty() {
            return out;
        }
        let position: FastMap<NodeId, usize> = entries
            .iter()
            .enumerate()
            .map(|(index, entry)| (entry.node, index))
            .collect();
        let in_cut: FastSet<NodeId> = cut.iter().copied().collect();
        let mut rule: Option<CoverRule> = None;
        for entry in &entries[..cut.len()] {
            let id = entry.node;
            let data = &nodes[&id];
            let stored = capped(&data.stored, self.caps.get(&id));
            let resident: Vec<u8> = self
                .drawable_stored(&stored, grid, data.bounds)
                .into_iter()
                .filter(|depth| self.is_resident(self.key(id, *depth)))
                .collect();
            if resident.is_empty() {
                continue;
            }
            let f = Footprint::new(grid, data.bounds);
            // Applied descendants of the node that drew something (a zoom out).
            let below: Vec<(NodeId, u8)> = self
                .applied_index
                .below
                .get(&(f.rep, f.lo))
                .map(|list| {
                    list.iter()
                        .copied()
                        .filter(|(d, _)| {
                            !in_cut.contains(d)
                                && self
                                    .applied_drawn
                                    .get(d)
                                    .is_some_and(|drawn| drawn.iter().any(Option::is_some))
                        })
                        .collect()
                })
                .unwrap_or_default();
            let has_above = f
                .gaps()
                .any(|k| self.applied_index.cells.contains_key(&f.ancestor(k)));
            if !has_above && below.is_empty() {
                continue;
            }
            let above = if has_above {
                let rule = rule.get_or_insert_with(|| {
                    let with_level: Vec<NodeId> = cut
                        .iter()
                        .copied()
                        .filter(|id| {
                            !hold.contains(id) && self.has_resident_level(*id, &nodes[id], grid)
                        })
                        .collect();
                    CoverRule::new(self, cut, &with_level, nodes, grid)
                });
                match rule.cover(self, &f) {
                    Cover::Fallback { ancestor, gap } => Some((ancestor, gap)),
                    Cover::Cut { cell, gap } => self
                        .applied_index
                        .cells
                        .get(&cell)
                        .map(|&ancestor| (ancestor, gap)),
                    Cover::Nothing => None,
                }
            } else {
                None
            };
            if above.is_none() && below.is_empty() {
                continue;
            }
            let held = hold.contains(&id);
            if held
                && !above.is_some_and(|(a, gap)| covers_in(entries, &position, a, &f, gap))
                && !below.iter().any(|(d, _)| position.contains_key(d))
            {
                continue;
            }
            // Per octant of the node: the deepest level that drew over it before.
            let mut limit: [Option<u8>; 8] = [None; 8];
            if let Some((ancestor, gap)) = above
                && let Some(drawn) = self.applied_drawn.get(&ancestor)
            {
                for (o, slot) in limit.iter_mut().enumerate() {
                    *slot = f
                        .octants_of(gap, o)
                        .filter_map(|a| self.drawn_ceiling(ancestor, drawn[a]))
                        .max();
                }
            }
            // The covering node may not have drawn over this zone before (it is new in the cut
            // or drew elsewhere): what drew there was the nearest applied ancestor above it.
            if let Some((_, gap)) = above {
                for k in f.gaps().filter(|k| *k > gap) {
                    let Some((far, drawn)) = self
                        .applied_index
                        .cells
                        .get(&f.ancestor(k))
                        .and_then(|a| Some((*a, self.applied_drawn.get(a)?)))
                    else {
                        continue;
                    };
                    for (o, slot) in limit.iter_mut().enumerate() {
                        if slot.is_none() {
                            *slot = f
                                .octants_of(k, o)
                                .filter_map(|a| self.drawn_ceiling(far, drawn[a]))
                                .max();
                        }
                    }
                }
            }
            for (d, k) in &below {
                let df = Footprint::new(grid, self.applied_index.nodes[d].bounds);
                for (o, drawn) in self.applied_drawn[d].iter().enumerate() {
                    if let Some(depth) = self.drawn_ceiling(*d, *drawn) {
                        for a in df.octants_of(*k, o) {
                            limit[a] = limit[a].max(Some(depth));
                        }
                    }
                }
            }
            let wanted = if below.is_empty() {
                entry.wanted
            } else {
                let solo = cell_cut_with(
                    &[id],
                    &|_| {
                        Some(CellCutNode {
                            bounds: data.bounds,
                            buildable: capped_range(data.buildable, self.caps.get(&id)),
                            stored: stored.clone(),
                            resident: resident.clone(),
                        })
                    },
                    grid,
                    self.lod_mode,
                    wanted_depth,
                );
                solo.ok()
                    .and_then(|solo| solo.first().map(|e| e.wanted))
                    .unwrap_or(entry.wanted)
            };
            let (here, _, _) = drawn_levels(
                &wanted,
                &stored,
                capped_range(data.buildable, self.caps.get(&id)),
                &resident,
                &self.prev_state_seeded(
                    id,
                    data.bounds,
                    capped_range(data.buildable, self.caps.get(&id)),
                    grid,
                ),
                f.rep.saturating_sub(MAX_MASK_LEVELS),
                self.lod_mode,
            );
            let short = (0..8).any(|o| {
                // The raw wanted depth, not the stored one (R3-Q21): a child storing only a
                // level coarser than what its parent draws does not take over until that level
                // is built on demand.
                wanted[o]
                    .is_some_and(|w| limit[o].is_some_and(|l| here[o].is_none_or(|d| d < l.min(w))))
            });
            if short {
                out.hold.insert(id);
            }
            if short || held {
                out.settle.insert(id);
                out.ancestors.extend(above.map(|(a, _)| a));
                out.ancestors.extend(below.iter().map(|(d, _)| *d));
            }
        }
        out
    }

    /// R3-Q20: the nodes of the applied cut that drew an octant which the real cut `entries` no
    /// longer draws and nothing covers: an ancestor of the real cut that draws there, or
    /// descendants of it that do. A node whose zone is no longer under any node of the planner's
    /// `cut` has left it and never counts. Built from the cells of the applied index, like the
    /// fallback; a descendant counts as covering an octant it touches.
    fn lost_zones(
        &self,
        entries: &[CellCutEntry],
        cut: &[NodeId],
        nodes: &FastMap<NodeId, PoolNode>,
        grid: &CellGrid,
    ) -> u32 {
        if self.applied.is_empty() || self.applied_index.grid != Some(*grid) {
            return 0;
        }
        let position: FastMap<NodeId, usize> = entries
            .iter()
            .enumerate()
            .map(|(index, entry)| (entry.node, index))
            .collect();
        let drew = |entry: &CellCutEntry, o: usize| entry.drawn[o].is_some();
        // Octants a new entry of the same node does not draw any more.
        let mut pending: Vec<(&CellCutEntry, [bool; 8])> = Vec::new();
        for old in &self.applied {
            let short: [bool; 8] = std::array::from_fn(|o| {
                drew(old, o)
                    && position.get(&old.node).is_none_or(|&index| {
                        let now = &entries[index];
                        now.drawn[o].is_none() && now.wanted[o].is_some()
                    })
            });
            if short.iter().any(|s| *s) {
                pending.push((old, short));
            }
        }
        if pending.is_empty() {
            return 0;
        }
        let feet = |id: &NodeId| -> Footprint {
            let bounds = self
                .applied_index
                .nodes
                .get(id)
                .map_or_else(|| nodes[id].bounds, |node| node.bounds);
            Footprint::new(grid, bounds)
        };
        // Cells with a cut node on them or over them, and per cell the octants a cut node below
        // it touches.
        let cut_set: FastSet<NodeId> = cut.iter().copied().collect();
        let mut cut_cells: FastSet<(u8, [u64; 3])> = FastSet::default();
        let mut over_cut: FastMap<(u8, [u64; 3]), [bool; 8]> = FastMap::default();
        let mut cut_under: Below<NodeId> = FastMap::default();
        for id in cut {
            let f = Footprint::new(grid, nodes[id].bounds);
            cut_cells.insert((f.rep, f.lo));
            for k in 1..=f.rep.saturating_sub(3) {
                cut_under.entry(f.ancestor(k)).or_default().push((*id, k));
                let touched = over_cut.entry(f.ancestor(k)).or_insert([false; 8]);
                for a in f.octants_in(k) {
                    touched[a] = true;
                }
            }
        }
        let mut by_cell: FastMap<(u8, [u64; 3]), usize> = FastMap::default();
        // Per wanted cell, the entries under it that draw and how many levels below.
        let mut under: Below<usize> = FastMap::default();
        for (index, entry) in entries.iter().enumerate() {
            let f = Footprint::new(grid, nodes[&entry.node].bounds);
            by_cell.entry((f.rep, f.lo)).or_insert(index);
        }
        let wanted_cells: FastSet<(u8, [u64; 3])> = pending
            .iter()
            .map(|(old, _)| {
                let f = feet(&old.node);
                (f.rep, f.lo)
            })
            .collect();
        for (index, entry) in entries.iter().enumerate() {
            if !entry.drawn.iter().any(Option::is_some) {
                continue;
            }
            let f = Footprint::new(grid, nodes[&entry.node].bounds);
            for k in 1..=f.rep.saturating_sub(3) {
                let cell = f.ancestor(k);
                if wanted_cells.contains(&cell) {
                    under.entry(cell).or_default().push((index, k));
                }
            }
        }
        let mut lost = 0;
        for (old, short) in pending {
            let f = feet(&old.node);
            let cell = (f.rep, f.lo);
            let whole = cut_set.contains(&old.node)
                || (1..=f.rep.saturating_sub(3)).any(|k| cut_cells.contains(&f.ancestor(k)));
            let in_cut = |o: usize| whole || over_cut.get(&cell).is_some_and(|t| t[o]);
            let covered = |o: usize| {
                under.get(&cell).is_some_and(|list| {
                    let zone = Zone {
                        f: &f,
                        mask: &old.mask,
                        octant: o,
                        // Only the part of the octant under the cut has to stay drawn.
                        need: (!whole)
                            .then(|| cut_under.get(&cell).map_or(&[][..], |v| v.as_slice())),
                    };
                    octant_covered(&zone, list, entries, nodes, grid)
                }) || (1..=f.rep.saturating_sub(3)).any(|k| {
                    by_cell.get(&f.ancestor(k)).is_some_and(|&index| {
                        let parent = &entries[index];
                        f.octants_of(k, o)
                            .all(|a| parent.drawn[a].is_some() || parent.wanted[a].is_none())
                    })
                })
            };
            if (0..8).any(|o| short[o] && in_cut(o) && !covered(o)) {
                lost += 1;
            }
        }
        lost
    }

    fn alloc_mask(&mut self) -> u32 {
        self.free_masks.pop_first().unwrap_or_else(|| {
            self.next_mask += 1;
            self.next_mask - 1
        })
    }

    /// Recomputes the cached octant depths of the cut nodes and the fallback nodes `extra` that
    /// need it. Returns how many were recomputed and, apart for the cut and for `extra`, whether
    /// any depth, or the node data they came from, differs from before.
    #[allow(clippy::too_many_arguments)]
    fn refresh_depths(
        &mut self,
        cut: &[NodeId],
        extra: &[NodeId],
        nodes: &FastMap<NodeId, PoolNode>,
        pose: Pose,
        view: &ViewProjection,
        grid: &CellGrid,
        origin: [f64; 3],
        target_px: f64,
        settled: bool,
    ) -> (u32, bool, bool) {
        let key = ViewKey {
            target_px,
            viewport: view.viewport(),
            visual_scale: view.visual_scale(),
            grid: *grid,
            origin,
            min_side: self.min_side,
        };
        if self.depth_key != Some(key) {
            self.depth_key = Some(key);
            self.depths.clear();
        }
        self.depths.retain(|id, _| nodes.contains_key(id));
        let (mut refreshed, mut changed, mut extra_changed) = (0, false, false);
        let aspect = f64::from(view.viewport()[0]) / f64::from(view.viewport()[1].max(1));
        let tan_corner = (VERTICAL_FOV * 0.5).tan() * (1.0 + aspect * aspect).sqrt();
        let rotation_limit = ROTATION_SHARE / tan_corner;
        let finest_allowed = self
            .min_side
            .map(|side| representative_depth_for_spacing(grid, side));
        let extra_set: FastSet<NodeId> = extra.iter().copied().collect();
        let mut due: Vec<NodeId> = Vec::new();
        let mut early: Vec<(f64, NodeId)> = Vec::new();
        for id in cut.iter().chain(extra) {
            let data = &nodes[id];
            let current = self.depths.get(id).filter(|cached| cached.node == *data);
            let Some(cached) = current else {
                due.push(*id);
                continue;
            };
            let moved = distance(cached.pose.eye, pose.eye);
            let turned = distance(cached.pose.forward, pose.forward);
            if moved == 0.0 && turned == 0.0 {
                // A hinted entry of a camera that has stopped is redone in full, once: every
                // such entry is due on the first settled frame, so a still pose is exact.
                if settled && !cached.exact {
                    due.push(*id);
                }
                continue;
            }
            // Share of the node's own limit already used; unbounded when the eye is on the node.
            let used = (moved / (cached.scale * DRIFT_FRACTION * cached.depth))
                .max(turned / (cached.scale * rotation_limit));
            if settled || used.is_nan() || used >= 1.0 {
                due.push(*id);
            } else if used >= EARLY_SHARE {
                early.push((used, *id));
            }
        }
        // Nodes close to their limit go first, worst first, within a per-frame budget: a rotating
        // camera would otherwise bring every node to its limit on the same frame.
        early.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        due.extend(
            early
                .into_iter()
                .take((cut.len() / EARLY_FRACTION).max(8))
                .map(|(_, id)| id),
        );
        for id in due {
            let data = &nodes[&id];
            let was = self.depths.get(&id).filter(|cached| cached.node == *data);
            refreshed += 1;
            // A drift refresh starts from the cached answer; a new node or a settled camera
            // searches in full, so a still pose is exactly `octant_wanted_depth`.
            let hints = (!settled)
                .then_some(was)
                .flatten()
                .map(|cached| cached.depths);
            let exact = hints.is_none();
            // Octant mode keeps the geometric probe: it still has the degenerate-probe hole (a box
            // reaching the eye asks for the finest depth), tracked as a follow-up. Only cell
            // mode probes the data extent and gives behind or empty octants their siblings' wish.
            let cell = self.lod_mode == LodMode::Cell;
            let probe = |octant: usize| {
                if cell {
                    data.probe_box(octant)
                } else {
                    ProbeBox::Geometric
                }
            };
            let depths = std::array::from_fn(|octant| match hints {
                Some(hints) => octant_wanted_depth_near(
                    view,
                    grid,
                    origin,
                    data.bounds,
                    octant,
                    probe(octant),
                    data.buildable,
                    target_px,
                    hints[octant],
                ),
                None => octant_wanted_depth_in(
                    view,
                    grid,
                    origin,
                    data.bounds,
                    octant,
                    probe(octant),
                    data.buildable,
                    target_px,
                ),
            });
            let depths = if cell {
                behind_wanted(
                    depths,
                    behind_octants(
                        view,
                        grid,
                        origin,
                        data.bounds,
                        std::array::from_fn(|octant| data.probe_box(octant)),
                    ),
                )
            } else {
                depths
            };
            let depths = match finest_allowed {
                Some(limit) => depths.map(|depth| continuity_depth(data.buildable, depth, limit)),
                None => depths,
            };
            let far = if self.lod_mode == LodMode::Cell {
                let (far, front) = node_far_depth(
                    view,
                    grid,
                    origin,
                    data.bounds,
                    std::array::from_fn(|octant| data.probe_box(octant)),
                    data.buildable,
                    target_px,
                    hints
                        .is_some()
                        .then(|| was.map(|cached| cached.far.0))
                        .flatten(),
                );
                (
                    finest_allowed
                        .map_or(far, |limit| continuity_depth(data.buildable, far, limit)),
                    front,
                )
            } else {
                (0, 0)
            };
            let differs = was.is_none_or(|cached| cached.depths != depths || cached.far != far);
            if extra_set.contains(&id) {
                extra_changed |= differs;
            } else {
                changed |= differs;
            }
            let hashed = id.get().wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 40;
            self.depths.insert(
                id,
                NodeDepths {
                    node: data.clone(),
                    depths,
                    far,
                    pose,
                    depth: effective_depth(pose, data.bounds, origin),
                    exact,
                    scale: 0.5 + 0.5 * hashed as f64 / (1_u64 << 24) as f64,
                },
            );
        }
        (refreshed, changed, extra_changed)
    }

    /// Plans one frame for `cut`. `UnknownNode` and `NoStoredLevels` are the caller's errors; a
    /// `DescendantTooFine` is not an error here, it retains the previous cut.
    pub fn frame(
        &mut self,
        cut: &[NodeId],
        node: &dyn Fn(NodeId) -> Option<PoolNode>,
        view: &ViewProjection,
        grid: &CellGrid,
        coordinate_origin: [f64; 3],
        target_px: f64,
    ) -> Result<FramePlan, CellCutError> {
        let mut nodes: FastMap<NodeId, PoolNode> =
            FastMap::with_capacity_and_hasher(cut.len(), Default::default());
        for id in cut {
            let data = node(*id).ok_or(CellCutError::UnknownNode(*id))?;
            if data.stored.is_empty() {
                return Err(CellCutError::NoStoredLevels(*id));
            }
            nodes.insert(*id, data);
        }
        self.frame += 1;
        self.carry = Carry::default();
        let eye: [f64; 3] = view
            .camera()
            .position()
            .map(|value| value / view.visual_scale());
        let camera = view.camera();
        let (position, target) = (camera.position(), camera.target());
        let heading: [f64; 3] = std::array::from_fn(|a| target[a] - position[a]);
        let length = heading.iter().map(|v| v * v).sum::<f64>().sqrt();
        let pose = Pose {
            eye,
            forward: heading.map(|v| if length > 0.0 { v / length } else { 0.0 }),
        };
        let settled = self.last_pose == Some(pose);
        self.last_pose = Some(pose);
        let mut hold: FastSet<NodeId> = self
            .hold
            .iter()
            .filter(|id| nodes.contains_key(id))
            .copied()
            .collect();
        let mut refreshed_total = 0;
        let mut previous: Vec<NodeId> = Vec::new();
        for pass in 1..=self.hold_passes + 1 {
            let found = self.fallbacks(cut, &nodes, grid, &hold);
            let fallback = found.extra();
            for id in &previous {
                nodes.remove(id);
            }
            for id in &fallback {
                nodes.insert(*id, self.applied_index.nodes[id].clone());
            }
            previous.clone_from(&fallback);
            let (refreshed, depths_changed, fallback_changed) = self.refresh_depths(
                cut,
                &fallback,
                &nodes,
                pose,
                view,
                grid,
                coordinate_origin,
                target_px,
                settled,
            );
            refreshed_total += refreshed;
            let depths = std::mem::take(&mut self.depths);
            let result = self.plan(
                cut,
                &found,
                &fallback,
                &nodes,
                &depths,
                (depths_changed, fallback_changed),
                &hold,
                match pass.cmp(&self.hold_passes) {
                    std::cmp::Ordering::Less => Round::Free,
                    std::cmp::Ordering::Equal => Round::Last,
                    std::cmp::Ordering::Greater => Round::Settled,
                },
                view,
                grid,
                coordinate_origin,
            );
            self.depths = depths;
            match result? {
                Pass::Done(mut plan) => {
                    self.hold = hold;
                    plan.counters.depth_refreshes = refreshed_total;
                    return Ok(*plan);
                }
                Pass::Redo(next) => hold = next,
            }
        }
        unreachable!("a settled pass never asks for another")
    }

    #[allow(clippy::too_many_arguments)]
    fn plan(
        &mut self,
        cut: &[NodeId],
        found: &Fallbacks,
        fallback: &[NodeId],
        nodes: &FastMap<NodeId, PoolNode>,
        depths: &FastMap<NodeId, NodeDepths>,
        (depths_changed, fallback_changed): (bool, bool),
        hold: &FastSet<NodeId>,
        round: Round,
        view: &ViewProjection,
        grid: &CellGrid,
        coordinate_origin: [f64; 3],
    ) -> Result<Pass, CellCutError> {
        let ctx = Ctx {
            grid,
            origin: coordinate_origin,
            depths,
            evaluations: Cell::new(0),
        };
        let eye: [f64; 3] = view
            .camera()
            .position()
            .map(|value| value / view.visual_scale());
        let frame = self.frame;
        if self.sizes.len() > self.sizes_prune_at {
            let levels = &self.levels;
            self.sizes
                .retain(|key, _| nodes.contains_key(&key.node) || levels.contains_key(key));
            self.sizes_prune_at = (self.sizes.len() * 2).max(1024);
        }
        self.fresh.clear();
        let mut page_frees = std::mem::take(&mut self.carry.page_frees);
        self.release(frame, &mut page_frees);

        // Wanted cut and fit, reused while nothing they read has changed.
        // E7: the D5 depth of every node of the real cut with descendants in the wanted cut is
        // protected from eviction and requested before anything else; the fit counts its pages.
        let d5 = if self.lod_mode == LodMode::Cell {
            self.d5_keys(cut, &found.ancestors, nodes, grid)
        } else {
            FastSet::default()
        };
        let reusable = !depths_changed
            && self.wanted_memo.as_ref().is_some_and(|memo| {
                let moved = distance(memo.eye, eye);
                memo.cut == cut
                    && memo.size_epoch == self.size_epoch
                    && memo.d5 == d5
                    && !(memo.had_caps && moved > DRIFT_FRACTION * memo.min_depth && moved > 0.0)
            });
        if !reusable {
            let mut offenders = BTreeSet::new();
            let (mut needed, mut requests, overflow) =
                self.fit(cut, nodes, &mut offenders, eye, &ctx, &d5)?;
            needed.retain(|key| !offenders.contains(&key.node));
            requests.retain(|key| !offenders.contains(&key.node));
            self.wanted_generation += 1;
            let min_depth = cut
                .iter()
                .map(|id| {
                    effective_depth(
                        Pose {
                            eye,
                            forward: self.last_pose.map_or([0.0; 3], |p| p.forward),
                        },
                        nodes[id].bounds,
                        coordinate_origin,
                    )
                })
                .fold(f64::INFINITY, f64::min);
            self.wanted_memo = Some(WantedMemo {
                cut: cut.to_vec(),
                size_epoch: self.size_epoch,
                generation: self.wanted_generation,
                needed: Rc::new(needed),
                requests: Rc::new(requests),
                offenders,
                overflow,
                eye,
                min_depth,
                had_caps: !self.caps.is_empty(),
                d5: d5.clone(),
            });
        }
        let memo = self.wanted_memo.as_ref().expect("set above");
        let (overflow, generation, needed, requests) = (
            memo.overflow,
            memo.generation,
            Rc::clone(&memo.needed),
            Rc::clone(&memo.requests),
        );
        if overflow && self.overflow_frame != frame {
            self.overflow += 1;
            self.overflow_frame = frame;
        }
        self.carry.wanted_reused.get_or_insert(reusable);
        let caps = self.caps.clone();

        // Real cut on the resident levels.
        let hold_digest = set_digest(hold);
        let real_reused = !fallback_changed
            && self.real_memo.as_ref().is_some_and(|memo| {
                memo.generation == generation
                    && memo.residency_epoch == self.residency_epoch
                    && memo.fallback == fallback
                    && memo.excluded == found.excluded
                    && memo.hold_digest == hold_digest
                    && memo.applied_digest == self.applied_digest
            });
        if !real_reused {
            // Excluded nodes (R3-Q20) have no level and nothing that draws them: out of the cut.
            let live: Vec<NodeId>;
            let live_cut: &[NodeId] = if found.excluded.is_empty() {
                cut
            } else {
                live = cut
                    .iter()
                    .filter(|id| !found.excluded.contains(id))
                    .copied()
                    .collect();
                &live
            };
            let joined: Vec<NodeId>;
            let real_cut: &[NodeId] = if fallback.is_empty() {
                live_cut
            } else {
                joined = live_cut.iter().chain(fallback).copied().collect();
                &joined
            };
            let kept = std::cell::RefCell::new(Vec::new());
            let lookup = |id: NodeId| {
                let data = nodes.get(&id)?;
                let stored = capped(&data.stored, caps.get(&id));
                let resident = if hold.contains(&id) {
                    Vec::new()
                } else {
                    stored
                        .iter()
                        .copied()
                        .filter(|depth| self.is_resident(self.key(id, *depth)))
                        .collect()
                };
                let mut node = CellCutNode {
                    bounds: data.bounds,
                    buildable: capped_range(data.buildable, caps.get(&id)),
                    stored,
                    resident,
                };
                if !hold.contains(&id)
                    && let Some(cap) = caps.get(&id)
                    && let Some(old) = self.finer_drawn(id, *cap, &node, &needed, &ctx)
                {
                    node.resident = old;
                    kept.borrow_mut().push(id);
                }
                Some(node)
            };
            ctx.evaluations.set(ctx.evaluations.get() + 1);
            let entries = match cell_cut_ranged(
                real_cut,
                &lookup,
                grid,
                self.lod_mode,
                &|id, o, data| ctx.depth(id, o, data),
                Some(&|id, data| ctx.far(id, data)),
                Some(&|id| match nodes.get(&id) {
                    Some(data) => self.prev_state_seeded(
                        id,
                        data.bounds,
                        capped_range(data.buildable, caps.get(&id)),
                        grid,
                    ),
                    None => self.prev_state(id),
                }),
            ) {
                Ok(entries) => Some(entries),
                Err(CellCutError::DescendantTooFine { .. }) => None,
                Err(other) => return Err(other),
            };
            let planned_missing = entries.as_deref().map_or(cut.len() as u32, |entries| {
                missing_levels(entries) + found.excluded.len() as u32
            });
            // An excluded node that is too fine for an ancestor of the cut is never requested (its
            // level would make the real cut fail with `DescendantTooFine`, and the ancestor,
            // which is requested, draws its zone): it must not keep the previous cut by itself.
            let waits_for_level = found.excluded.iter().any(|id| {
                self.wanted_memo
                    .as_ref()
                    .is_none_or(|memo| !memo.offenders.contains(id))
            });
            let (lost, blank) = entries.as_deref().map_or((0, false), |entries| {
                (
                    self.lost_zones(entries, cut, nodes, grid),
                    entries.is_empty() && waits_for_level && !self.applied.is_empty(),
                )
            });
            let (_, covered_octants) =
                entries.as_deref().map_or((cut.len() as u32, 0), |entries| {
                    uncovered_levels(entries, nodes, grid)
                });
            let eval = entries.as_deref().map_or_else(
                || HoldEval {
                    hold: hold.clone(),
                    settle: hold.clone(),
                    ..HoldEval::default()
                },
                |entries| {
                    self.eval_hold(entries, live_cut, nodes, grid, hold, &|id, o, data| {
                        ctx.depth(id, o, data)
                    })
                },
            );
            self.real_memo = Some(RealMemo {
                generation,
                residency_epoch: self.residency_epoch,
                entries,
                planned_missing,
                covered_octants,
                fallback: fallback.to_vec(),
                excluded: found.excluded.clone(),
                lost,
                blank,
                hold_digest,
                applied_digest: self.applied_digest,
                kept_finer: {
                    let mut kept = kept.into_inner();
                    kept.sort();
                    kept.dedup();
                    kept
                },
                next_hold: eval.hold,
                settle: eval.settle,
                hold_ancestors: eval.ancestors,
                applied: false,
            });
        }
        let memo = self.real_memo.as_mut().expect("set above");
        if round != Round::Settled && memo.next_hold != *hold {
            let next = if round == Round::Last {
                // No fixed point: keep held what is held in either set and has a covering
                // ancestor (a drawn octant never gets coarser), and count it.
                self.carry.unsettled = memo
                    .settle
                    .iter()
                    .filter(|id| hold.contains(id) != memo.next_hold.contains(id))
                    .count() as u32;
                memo.settle.clone()
            } else {
                memo.next_hold.clone()
            };
            if next != *hold {
                self.carry.page_frees = page_frees;
                self.carry.evaluations += ctx.evaluations.get();
                return Ok(Pass::Redo(next));
            }
        }
        let (too_fine, planned_missing, lost) =
            (memo.entries.is_none(), memo.planned_missing, memo.lost);
        let apply = if !too_fine && !overflow && memo.lost == 0 && !memo.blank {
            CutApply::Applied
        } else {
            CutApply::RetainedPrevious
        };
        // An applied cut loses no zone, and a held node has its zone drawn by an ancestor or by
        // descendants of the real cut (nodes without a level may wait: R3-Q20).
        debug_assert!(
            apply != CutApply::Applied
                || memo.entries.as_ref().is_none_or(|entries| {
                    let position: FastMap<NodeId, usize> = entries
                        .iter()
                        .enumerate()
                        .map(|(index, entry)| (entry.node, index))
                        .collect();
                    memo.lost == 0
                        && !memo.blank
                        && hold
                            .iter()
                            .filter(|id| !found.excluded.contains(id))
                            .all(|id| {
                                let f = Footprint::new(grid, nodes[id].bounds);
                                // A node held by the D5 gate is covered by an ancestor of the
                                // real cut itself.
                                (self.lod_mode == LodMode::Cell
                                    && f.gaps().any(|gap| {
                                        entries.iter().any(|a| {
                                            let af = Footprint::new(grid, nodes[&a.node].bounds);
                                            (af.rep, af.lo) == f.ancestor(gap)
                                                && covers_in(entries, &position, a.node, &f, gap)
                                        })
                                    }))
                                    || f.gaps().any(|gap| {
                                        self.applied_index.cells.get(&f.ancestor(gap)).is_some_and(
                                            |a| covers_in(entries, &position, *a, &f, gap),
                                        )
                                    })
                                    || self.applied_index.below.get(&(f.rep, f.lo)).is_some_and(
                                        |list| list.iter().any(|(d, _)| position.contains_key(d)),
                                    )
                            })
                }),
            "an applied cut loses a zone, or a held node has nothing drawing its zone"
        );
        // Levels in use: the drawn ones and, when retaining, the previous cut's.
        let mut protected: FastSet<LevelKey> = FastSet::default();
        let mut pinned: FastSet<LevelKey> = FastSet::default();
        let mut step_hops = 0;
        if apply == CutApply::Applied {
            if let Some(memo) = self.real_memo.as_ref().filter(|memo| !memo.applied)
                && let Some(entries) = memo.entries.as_deref()
            {
                step_hops = self.step_hops(entries);
            }
            let memo = self.real_memo.as_mut().expect("set above");
            let fresh = !memo.applied;
            memo.applied = true;
            self.applied_covered = memo.covered_octants;
            if fresh {
                self.applied = memo.entries.clone().expect("applied cut exists");
                self.applied_drawn = self
                    .applied
                    .iter()
                    .map(|entry| (entry.node, entry.drawn))
                    .collect();
                self.applied_digest = drawn_digest(&self.applied);
                self.applied_cover.clone_from(&memo.hold_ancestors);
                self.index_applied(nodes, grid);
            }
        }
        // The wait of each stepping octant of what is on screen: every frame counts. At the
        // limit the cut is planned again, so the octants hop (`prev_state`).
        let mut waits = FastMap::default();
        let mut expired = false;
        for entry in &self.applied {
            for o in 0..8 {
                if let (Some(w), Some(drawn), Some(next)) =
                    (entry.wanted[o], entry.drawn[o], entry.step[o])
                {
                    let key = (drawn, w > drawn, next);
                    let frames = match self.step_waits.get(&(entry.node, o)) {
                        Some(&(waiting, frames)) if waiting == key => frames + 1,
                        // This frame is the first of the wait.
                        _ => 1,
                    };
                    expired |= frames == STEP_WAIT_FRAMES;
                    waits.insert((entry.node, o), (key, frames));
                }
            }
        }
        self.step_waits = waits;
        if expired {
            self.residency_epoch += 1;
        }
        for entry in &self.applied {
            for depth in entry.required_levels() {
                protected.insert(self.key(entry.node, depth));
            }
        }
        for id in cut.iter().chain(&found.ancestors) {
            let depth = match self.lod_mode {
                LodMode::Octant => coarsest(&nodes[id].stored, caps.get(id)),
                // The coarsest level that can draw in the node's span, stored or not yet.
                LodMode::Cell => {
                    let node = &nodes[id];
                    let stored = capped(&node.stored, caps.get(id));
                    self.drawable_stored(&stored, grid, node.bounds)
                        .first()
                        .copied()
                        .or_else(|| {
                            let buildable = capped_range(node.buildable, caps.get(id));
                            self.coarsest_in_span(buildable, grid, node.bounds)
                        })
                }
            };
            if let Some(depth) = depth {
                pinned.insert(self.key(*id, depth));
            }
        }
        if apply == CutApply::Applied {
            self.applied_pins = pinned.clone();
        } else {
            pinned.extend(self.applied_pins.iter().copied());
        }
        // Overflow keeps only the previous cut: nothing of the new one is requested.
        let no_levels = FastSet::default();
        let want: &FastSet<LevelKey> = if overflow { &no_levels } else { &needed };
        // ADR 0053: the adjacent depth each stepping octant waits for is requested right after
        // D5, and kept from eviction while it loads.
        let steps: FastSet<LevelKey> = if overflow {
            FastSet::default()
        } else {
            self.real_memo
                .as_ref()
                .and_then(|memo| memo.entries.as_deref())
                .into_iter()
                .flatten()
                .flat_map(|entry| {
                    entry
                        .step
                        .iter()
                        .flatten()
                        .map(|d| self.key(entry.node, *d))
                })
                .chain(
                    self.step_waits
                        .iter()
                        .filter(|((node, _), _)| nodes.contains_key(node))
                        .map(|((node, _), ((_, _, next), _))| self.key(*node, *next)),
                )
                .collect()
        };
        // Range depths no octant wants rank after the wanted ones and the step.
        let range_only: FastSet<LevelKey> = if overflow {
            FastSet::default()
        } else {
            self.real_memo
                .as_ref()
                .and_then(|memo| memo.entries.as_deref())
                .into_iter()
                .flatten()
                .flat_map(|entry| {
                    self.range_depths(entry, &nodes[&entry.node], &self.caps, grid)
                        .1
                        .into_iter()
                        .filter(|d| !entry.wanted.iter().flatten().any(|w| w == d))
                        .map(|d| self.key(entry.node, d))
                })
                .collect()
        };
        let stored_step = |key: &LevelKey| {
            nodes
                .get(&key.node)
                .is_some_and(|node| node.stored.contains(&key.depth))
        };
        protected.extend(want.iter().copied());
        protected.extend(steps.iter().filter(|key| stored_step(key)).copied());
        protected.extend(d5.iter().copied());
        protected.extend(pinned.iter().copied());
        self.pinned = pinned;

        // Cancel what nobody needs any more.
        let mut freed_now = Vec::new();
        let cancelled: Vec<LevelKey> = self
            .levels
            .iter()
            .filter(|(key, level)| !level.is_resident() && !protected.contains(key))
            .map(|(key, _)| *key)
            .collect();
        for key in cancelled {
            let before = self.known_pages(key);
            if let Some(level) = self.levels.remove(&key) {
                if before != self.known_pages(key) {
                    self.size_epoch += 1;
                }
                if let LevelState::Uploading { slots, .. } = level.state {
                    freed_now.extend(slots);
                }
                self.events.push(CellPoolEvent::Cancel(key));
            }
        }
        if !freed_now.is_empty() {
            self.pending.push((frame, freed_now));
        }

        // Use: LRU stamps.
        for key in &protected {
            if let Some(level) = self.levels.get_mut(key) {
                level.last_used = frame;
            }
        }

        // Requests.
        let mut order: Vec<(LevelKey, Need)> = Vec::new();
        let mut fetch_requests = Vec::new();
        let mut drawable: FastSet<NodeId> = FastSet::default();
        for key in self
            .levels
            .iter()
            .filter(|(key, l)| {
                l.is_resident() && caps.get(&key.node).is_none_or(|cap| key.depth <= *cap)
            })
            .map(|(k, _)| *k)
        {
            // Cell mode: a level outside the node's span never draws.
            if self.lod_mode == LodMode::Cell
                && !nodes.get(&key.node).is_some_and(|node| {
                    in_cell_range(&[key.depth], Self::node_depth(grid, node.bounds)).len() == 1
                })
            {
                continue;
            }
            drawable.insert(key.node);
        }
        let mut wanted_keys: Vec<LevelKey> = if overflow {
            want.iter().copied().collect()
        } else {
            want.iter()
                .chain(&d5)
                .chain(steps.iter().filter(|key| stored_step(key)))
                .copied()
                .collect()
        };
        wanted_keys.sort();
        wanted_keys.dedup();
        for key in wanted_keys {
            let data = &nodes[&key.node];
            if let std::collections::hash_map::Entry::Vacant(slot) = self.levels.entry(key) {
                slot.insert(Level {
                    state: LevelState::Requested,
                    data: None,
                    last_used: frame,
                    last_active: 0,
                    refresh: None,
                });
                fetch_requests.push(key);
            }
            if !self.levels[&key].is_resident() {
                let coarsest_drawable = match self.lod_mode {
                    LodMode::Octant => coarsest(&data.stored, caps.get(&key.node)),
                    LodMode::Cell => self
                        .drawable_stored(
                            &capped(&data.stored, caps.get(&key.node)),
                            grid,
                            data.bounds,
                        )
                        .first()
                        .copied(),
                };
                let first = d5.contains(&key)
                    || (!drawable.contains(&key.node) && coarsest_drawable == Some(key.depth));
                order.push((
                    key,
                    Need {
                        first,
                        step: steps.contains(&key),
                        distance: distance_sq(eye, data.bounds, coordinate_origin),
                    },
                ));
            }
        }

        // Coarse to fine (R3-Q19): the first level of the nodes that draw nothing, so no region
        // starves behind refinements elsewhere; then level depth, so a node's level is not
        // requested before the one its covering ancestor needs; then distance.
        order.sort_by(|(ka, a), (kb, b)| {
            (!a.first)
                .cmp(&!b.first)
                .then((!a.step).cmp(&!b.step))
                .then(ka.depth.cmp(&kb.depth))
                .then(a.distance.total_cmp(&b.distance))
                .then(ka.node.cmp(&kb.node))
        });
        let fresh: FastSet<LevelKey> = fetch_requests.iter().copied().collect();
        fetch_requests = order
            .iter()
            .map(|(key, _)| *key)
            .filter(|key| fresh.contains(key))
            .collect();

        // Missing levels the view wants and the node can build, coarse to fine like the fetches.
        // They hold no pool state: the level arrives as a stored one (`add_packed_levels`).
        let mut level_requests: Vec<LevelKey> = if overflow {
            Vec::new()
        } else {
            requests.iter().copied().collect()
        };
        for key in &d5 {
            let stored = nodes
                .get(&key.node)
                .is_some_and(|node| node.stored.contains(&key.depth));
            if !overflow && !stored && !level_requests.contains(key) {
                level_requests.push(*key);
            }
        }
        for key in &steps {
            if !overflow && !stored_step(key) && !level_requests.contains(key) {
                level_requests.push(*key);
            }
        }
        level_requests.sort_by(|a, b| {
            let distance =
                |key: &LevelKey| distance_sq(eye, nodes[&key.node].bounds, coordinate_origin);
            (!d5.contains(a))
                .cmp(&!d5.contains(b))
                .then((!steps.contains(a)).cmp(&!steps.contains(b)))
                .then(range_only.contains(a).cmp(&range_only.contains(b)))
                .then(a.depth.cmp(&b.depth))
                .then(distance(a).total_cmp(&distance(b)))
                .then(a.node.cmp(&b.node))
        });

        // Eviction: on demand for what is missing, and down to the soft limit.
        let missing: usize = order
            .iter()
            .map(|(key, _)| {
                let level = &self.levels[key];
                match &level.state {
                    LevelState::Uploading { .. } => 0,
                    _ => level
                        .pages()
                        .unwrap_or_else(|| self.pages_of(*key, &nodes[&key.node], grid)),
                }
            })
            .sum();
        let soft = (self.usable() * SOFT_PERCENT / 100) as usize;
        let mut candidates: Vec<(u64, LevelKey)> = self
            .levels
            .iter()
            .filter(|(key, level)| level.is_resident() && !protected.contains(key))
            .map(|(key, level)| (level.last_used, *key))
            .collect();
        candidates.sort();
        for (_, key) in candidates {
            let short = self.free.len() + self.pending_pages() < missing;
            if !short && self.allocated_pages() <= soft {
                break;
            }
            self.evict_resident(key, frame);
        }
        // The finer levels kept for capped nodes (`finer_drawn`) are protected while they draw;
        // when the capped levels still lack pages after every other eviction, they are given up
        // (the real cut drops to the resident capped levels next frame, which unprotects them).
        if self.free.len() + self.pending_pages() < missing
            && let Some(memo) = self.real_memo.as_ref().filter(|memo| memo.applied)
            && !memo.kept_finer.is_empty()
        {
            let kept = memo.kept_finer.clone();
            for id in kept {
                if let Some(cap) = caps.get(&id) {
                    self.old_released.insert(id, *cap);
                }
            }
            self.residency_epoch += 1;
        }
        self.release(frame, &mut page_frees);

        // Uploads, spread over frames, in the same order.
        let mut budget = (self.config.upload_budget_bytes / POOL_PAGE_BYTES).max(1) as usize;
        let mut uploads = Vec::new();
        let mut uploaded_pages = self.refresh_uploads(&mut budget, &mut uploads);
        let next_free = self.pending_pages_by(frame + 1);
        let mut blocked = false;
        for (key, _) in &order {
            if budget == 0 {
                break;
            }
            let Some(pages) = self.levels[key].pages() else {
                continue;
            };
            if matches!(self.levels[key].state, LevelState::Requested) {
                if blocked {
                    continue;
                }
                if self.free.len() < pages {
                    // Pages about to be released: nobody of lower priority takes the free ones.
                    blocked = self.free.len() + next_free >= pages;
                    continue;
                }
                let slots: Vec<u32> = (0..pages)
                    .map(|_| self.free.pop_first().expect("checked above"))
                    .collect();
                self.levels.get_mut(key).expect("level").state = LevelState::Uploading {
                    slots,
                    pages_done: 0,
                };
            }
            let level = self.levels.get_mut(key).expect("level");
            let LevelState::Uploading { slots, pages_done } = &mut level.state else {
                continue;
            };
            let count = budget.min(pages - *pages_done);
            let range = *pages_done..*pages_done + count;
            uploads.push(PageUpload {
                key: *key,
                slots: slots[range.clone()].to_vec(),
                page_range: range,
            });
            *pages_done += count;
            budget -= count;
            uploaded_pages += count;
            if *pages_done == pages {
                let slots = std::mem::take(slots);
                level.state = LevelState::Resident { slots };
                self.residency_epoch += 1;
                self.fresh.insert(*key);
            }
        }

        // Masks and active levels of the cut being drawn.
        let mut mask_updates = Vec::new();
        let mut active = Vec::new();
        if apply == CutApply::Applied {
            let live: FastSet<NodeId> = self.applied.iter().map(|entry| entry.node).collect();
            let gone: Vec<NodeId> = self
                .mask_slots
                .keys()
                .filter(|id| !live.contains(id))
                .copied()
                .collect();
            for id in gone {
                if let Some(old) = self.mask_slots.remove(&id) {
                    self.free_masks.insert(old.slot);
                }
            }
            let mut finest = FastMap::default();
            let mut d5_depths = FastMap::default();
            for index in 0..self.applied.len() {
                let (id, mask) = (self.applied[index].node, self.applied[index].mask);
                let bounds = nodes
                    .get(&id)
                    .or_else(|| self.applied_index.nodes.get(&id))
                    .map(|node| node.bounds);
                debug_assert!(bounds.is_some(), "a node of the applied cut has bounds");
                let Some(bounds) = bounds else {
                    continue;
                };
                let node_depth = Self::node_depth(grid, bounds);
                let cell = self.lod_mode == LodMode::Cell;
                // Octant mode keeps `active` zero in its slot: only cell mode compares it, and
                // the update below computes it for the nodes it sends.
                let mut active_bits = 0_u64;
                if cell {
                    let d5 = self.d5_active(&self.applied[index], false);
                    if let Some(d5) = d5 {
                        d5_depths.insert(id, d5);
                    }
                    // Exactly the depths emitted below as `ActiveLevel`s of this node.
                    active_bits = self
                        .cell_active_depths(&self.applied[index], node_depth, d5)
                        .into_iter()
                        .fold(0_u64, |bits, depth| bits | 1 << (depth - node_depth));
                    if active_bits != 0 {
                        let top = node_depth + (63 - active_bits.leading_zeros()) as u8;
                        finest.insert(id, top);
                    }
                }
                let next = |slot: u32| MaskSlot {
                    slot,
                    mask,
                    active: active_bits,
                    node_depth,
                    stale: false,
                };
                let update = |this: &Self, slot: u32| {
                    let active = if cell {
                        active_bits
                    } else {
                        this.applied[index]
                            .required_levels()
                            .into_iter()
                            .filter(|depth| {
                                (node_depth..=node_depth.saturating_add(63)).contains(depth)
                            })
                            .fold(0_u64, |bits, depth| bits | 1 << (depth - node_depth))
                    };
                    MaskUpdate {
                        slot,
                        mask,
                        active,
                        node_depth,
                        node_lo: std::array::from_fn(|axis| {
                            crate::node_cells::node_range(grid, bounds, node_depth, axis).0
                        }),
                    }
                };
                match self.mask_slots.get(&id) {
                    // Octant mode sends a node only when its mask changes; cell mode also when
                    // its active depths or node depth do.
                    Some(old)
                        if !old.stale
                            && old.mask == mask
                            && (!cell
                                || (old.active, old.node_depth) == (active_bits, node_depth)) => {}
                    Some(old) => {
                        let slot = old.slot;
                        mask_updates.push(update(self, slot));
                        self.mask_slots.insert(id, next(slot));
                    }
                    None => {
                        let slot = self.alloc_mask();
                        mask_updates.push(update(self, slot));
                        self.mask_slots.insert(id, next(slot));
                    }
                }
            }
            self.applied_d5 = d5_depths;
            self.applied_finest = finest;
        }
        let mut applied_levels = FastSet::default();
        for entry in &self.applied {
            let Some(MaskSlot { slot, .. }) = self.mask_slots.get(&entry.node) else {
                continue;
            };
            let cell_depths = (self.lod_mode == LodMode::Cell).then(|| {
                // Retained frames keep the D5 depth the records were last written with.
                let d5 = self.d5_active(entry, true);
                let node_depth = nodes
                    .get(&entry.node)
                    .or_else(|| self.applied_index.nodes.get(&entry.node))
                    .map(|node| Self::node_depth(grid, node.bounds));
                node_depth.map_or_else(Vec::new, |d| self.cell_active_depths(entry, d, d5))
            });
            // Cell mode: active depths the node does not draw itself (its D5 depth) come last.
            let extra = cell_depths.as_ref().map_or_else(Vec::new, |depths| {
                let drawn: Vec<u8> = entry.required_levels();
                depths
                    .iter()
                    .filter(|depth| !drawn.contains(depth))
                    .map(|depth| (*depth, 0xff))
                    .collect()
            });
            for (depth, bits) in entry.draws().into_iter().chain(extra) {
                if cell_depths
                    .as_ref()
                    .is_some_and(|depths| !depths.contains(&depth))
                {
                    continue;
                }
                let key = self.key(entry.node, depth);
                if !self.is_resident(key) {
                    continue;
                }
                applied_levels.insert(key);
                active.push(ActiveLevel {
                    key,
                    // The GPU decides per cell which of the node's active levels draws.
                    octant_bits: if self.lod_mode == LodMode::Cell {
                        0xff
                    } else {
                        bits
                    },
                    mask_slot: *slot,
                });
            }
        }
        for key in &applied_levels {
            if let Some(level) = self.levels.get_mut(key) {
                level.last_active = frame;
            }
        }
        self.applied_levels = applied_levels;

        let cover: FastSet<NodeId> = fallback
            .iter()
            .chain(&self.applied_cover)
            .copied()
            .collect();
        let fallback_pages = self
            .applied
            .iter()
            .filter(|entry| cover.contains(&entry.node))
            .flat_map(|entry| {
                entry
                    .required_levels()
                    .into_iter()
                    .map(|depth| self.key(entry.node, depth))
            })
            .filter_map(|key| self.known_pages(key))
            .sum::<usize>() as u32;
        // Excluded nodes too fine for an ancestor of the cut are never requested (see
        // `waits_for_level`): the ancestor draws their zone, coarser, so they are a stage and not
        // a node waiting for a level (otherwise the counters and `busy` would never settle).
        let too_fine_excluded: FastSet<NodeId> =
            self.wanted_memo
                .as_ref()
                .map_or_else(FastSet::default, |memo| {
                    found
                        .excluded
                        .iter()
                        .filter(|id| memo.offenders.contains(id))
                        .copied()
                        .collect()
                });
        let waiting_excluded = found.excluded.len() as u32 - too_fine_excluded.len() as u32;
        // A held or excluded node that waited last frame too is one frame older.
        let waiting: FastMap<NodeId, u32> = hold
            .iter()
            .chain(
                found
                    .excluded
                    .iter()
                    .filter(|id| !too_fine_excluded.contains(id)),
            )
            .map(|id| (*id, self.wait_ages.get(id).map_or(1, |age| age + 1)))
            .collect();
        let age_max = waiting.values().copied().max().unwrap_or(0);
        self.wait_ages = waiting;
        // The cut being drawn: after a retained frame it is the previous one, so the cut nodes it
        // does not hold are the ones without a level.
        // A held node whose octants are all hidden by its descendants has no unmasked octant for
        // `missing_levels` to count, but it is still waiting.
        let hidden_held = if apply == CutApply::Applied {
            self.applied
                .iter()
                .filter(|e| hold.contains(&e.node) && e.wanted.iter().all(Option::is_none))
                .count() as u32
        } else {
            0
        };
        let absent = if apply == CutApply::Applied {
            waiting_excluded
        } else {
            cut.iter()
                .filter(|id| {
                    !self.applied_drawn.contains_key(id) && !too_fine_excluded.contains(id)
                })
                .count() as u32
        };
        let counters = PoolCounters {
            stages: self
                .applied
                .iter()
                .map(|entry| entry.stage.iter().filter(|s| **s).count() as u32)
                .sum::<u32>()
                + self.applied_covered
                + 8 * too_fine_excluded.len() as u32,
            nodes_without_level: if self.applied.is_empty() {
                planned_missing
            } else {
                missing_levels(&self.applied) + absent + hidden_held
            },
            pool_coarsened: caps.len() as u32,
            pool_overflow: self.overflow,
            resident_pages: self.resident_pages(),
            cut_evaluations: self.carry.evaluations + ctx.evaluations.get(),
            descendant_too_fine: too_fine,
            uploaded_bytes: uploaded_pages as u64 * POOL_PAGE_BYTES,
            depth_refreshes: 0,
            wanted_reused: self.carry.wanted_reused.unwrap_or(reusable),
            real_reused,
            held_nodes: hold.len() as u32,
            fallback_pages,
            hold_unsettled: self.carry.unsettled,
            excluded_nodes: absent,
            lost_nodes: if apply == CutApply::Applied { 0 } else { lost },
            hold_age_max: age_max,
            step_hops,
        };
        Ok(Pass::Done(Box::new(FramePlan {
            apply,
            entries: self.applied.clone(),
            fetch_requests,
            level_requests,
            uploads,
            page_frees,
            mask_updates,
            active,
            events: std::mem::take(&mut self.events),
            counters,
            coarsened_nodes: {
                let mut ids: Vec<NodeId> = caps.keys().copied().collect();
                ids.sort_unstable();
                ids
            },
        })))
    }
}

/// An octant of an applied node whose zone has to stay drawn.
struct Zone<'a> {
    f: &'a Footprint,
    /// Cells of the node that its own children hid in the applied cut.
    mask: &'a [u64; 8],
    octant: usize,
    /// The cut nodes (and levels below the node) whose cells are the only ones that have to stay
    /// drawn; `None`: all the visible cells of the octant.
    need: Option<&'a [(NodeId, u8)]>,
}

/// Do the drawn octants of the entries `below` (index, levels below the node) cover every cell of
/// the octant of `zone` that has to stay drawn? The union is measured at the finest resolution
/// among the boxes, so one descendant drawing a sub-cell does not cover the whole octant.
fn octant_covered(
    zone: &Zone,
    below: &[(usize, u8)],
    entries: &[CellCutEntry],
    nodes: &FastMap<NodeId, PoolNode>,
    grid: &CellGrid,
) -> bool {
    type Box3 = ([u64; 3], [u64; 3]);
    let f = zone.f;
    let scale = below
        .iter()
        .map(|(_, k)| *k)
        .chain(zone.need.into_iter().flatten().map(|(_, k)| *k))
        .max()
        .unwrap_or(0);
    // Half-open boxes in units of a cell of the deepest node, relative to the block of `f`.
    let boxed = |k: u8, (lo, hi): ([u64; 3], [u64; 3])| -> Box3 {
        let origin = f.lo.map(|c| c << k);
        let shift = scale - k;
        (
            std::array::from_fn(|a| lo[a].saturating_sub(origin[a]) << shift),
            std::array::from_fn(|a| (hi[a] + 1).saturating_sub(origin[a]) << shift),
        )
    };
    let mut covers: Vec<Box3> = Vec::new();
    for (index, k) in below {
        let entry = &entries[*index];
        let d = Footprint::new(grid, nodes[&entry.node].bounds);
        for o2 in (0..8).filter(|o2| entry.drawn[*o2].is_some()) {
            covers.push(boxed(*k, d.octant_range(o2)));
        }
    }
    let needs: Option<Vec<Box3>> = zone.need.map(|list| {
        list.iter()
            .map(|(id, k)| {
                let d = Footprint::new(grid, nodes[id].bounds);
                boxed(*k, (d.lo, d.hi))
            })
            .collect()
    });
    let region: [(u64, u64); 3] = std::array::from_fn(|axis| {
        let start = 4 * (zone.octant >> (2 - axis) & 1) as u64;
        (start << scale, (start + 4) << scale)
    });
    // Compressed grid of the box edges inside the region: each piece is in or out of every box.
    let edges: [Vec<u64>; 3] = std::array::from_fn(|axis| {
        let (lo, hi) = region[axis];
        let mut edges = vec![lo, hi];
        for (bl, bh) in covers.iter().chain(needs.iter().flatten()) {
            edges.extend(
                [bl[axis], bh[axis]]
                    .into_iter()
                    .filter(|e| (lo..=hi).contains(e)),
            );
        }
        edges.sort_unstable();
        edges.dedup();
        edges
    });
    let inside = |boxes: &[Box3], at: [u64; 3]| {
        boxes
            .iter()
            .any(|(bl, bh)| (0..3).all(|a| bl[a] <= at[a] && at[a] < bh[a]))
    };
    for x in edges[0].windows(2) {
        for y in edges[1].windows(2) {
            for z in edges[2].windows(2) {
                let at = [x[0], y[0], z[0]];
                let cell = at.map(|c| (c >> scale) as usize);
                if zone.mask[cell[2]] >> (cell[0] + 8 * cell[1]) & 1 == 1
                    || needs.as_ref().is_some_and(|needs| !inside(needs, at))
                {
                    continue;
                }
                if !inside(&covers, at) {
                    return false;
                }
            }
        }
    }
    true
}

/// The first depth of `stored` truncated to the cap, if any.
fn coarsest(stored: &[u8], cap: Option<&u8>) -> Option<u8> {
    stored
        .first()
        .copied()
        .filter(|depth| cap.is_none_or(|cap| depth <= cap))
}

fn missing_levels(entries: &[CellCutEntry]) -> u32 {
    entries
        .iter()
        .filter(|entry| (0..8).any(|o| entry.wanted[o].is_some() && entry.drawn[o].is_none()))
        .count() as u32
}

/// R3-Q19 on a real cut: `(nodes, octants)` of the entries that have an unmasked octant without a
/// level and no ancestor in the cut that draws where the node lies (the nodes), and the unmasked
/// octants without a level of the nodes that do have such an ancestor (the octants, which draw a
/// coarser level: stages).
fn uncovered_levels(
    entries: &[CellCutEntry],
    nodes: &FastMap<NodeId, PoolNode>,
    grid: &CellGrid,
) -> (u32, u32) {
    let missing = |entry: &CellCutEntry| {
        (0..8)
            .filter(|o| entry.wanted[*o].is_some() && entry.drawn[*o].is_none())
            .count() as u32
    };
    if entries.iter().all(|entry| missing(entry) == 0) {
        return (0, 0);
    }
    let feet: Vec<Footprint> = entries
        .iter()
        .map(|entry| Footprint::new(grid, nodes[&entry.node].bounds))
        .collect();
    let mut by_cell: FastMap<(u8, [u64; 3]), usize> = FastMap::default();
    for (index, f) in feet.iter().enumerate() {
        by_cell.entry((f.rep, f.lo)).or_insert(index);
    }
    let (mut uncovered, mut octants) = (0, 0);
    for (index, entry) in entries.iter().enumerate() {
        let count = missing(entry);
        if count == 0 {
            continue;
        }
        let covered = feet[index].gaps().any(|k| {
            let Some(&parent) = by_cell.get(&feet[index].ancestor(k)) else {
                return false;
            };
            let parent = &entries[parent];
            // A hidden octant (`wanted` is None) is drawn by the resident nodes that hide it.
            feet[index]
                .octants_in(k)
                .all(|o| parent.drawn[o].is_some() || parent.wanted[o].is_none())
        });
        if covered {
            octants += count;
        } else {
            uncovered += 1;
        }
    }
    (uncovered, octants)
}

/// `wanted` limited to the finest buildable depth at or below `limit` (cells of that depth or
/// coarser have a side of at least the continuity minimum); the coarsest buildable depth when all
/// are finer.
fn continuity_depth(buildable: (u8, u8), wanted: u8, limit: u8) -> u8 {
    wanted.min(limit.clamp(buildable.0, buildable.1))
}

/// `buildable` with its finest depth lowered to the cap (never below its coarsest).
fn capped_range(buildable: (u8, u8), cap: Option<&u8>) -> (u8, u8) {
    cap.map_or(buildable, |cap| {
        (buildable.0, buildable.1.min(*cap).max(buildable.0))
    })
}

/// `stored` truncated to depths at or below the cap.
///
/// Never empty while `stored` is not: a cap below every stored level (its depth was dropped by a
/// recolour) leaves the coarsest one.
fn capped(stored: &[u8], cap: Option<&u8>) -> Vec<u8> {
    let kept: Vec<u8> = stored
        .iter()
        .copied()
        .filter(|depth| cap.is_none_or(|cap| depth <= cap))
        .collect();
    if kept.is_empty() {
        stored.first().copied().into_iter().collect()
    } else {
        kept
    }
}

#[cfg(test)]
#[path = "cell_pool_tests.rs"]
mod tests;
