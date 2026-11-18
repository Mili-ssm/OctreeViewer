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

pub use crate::cell_cut::{ActiveRule, LodMode};

use crate::camera::VERTICAL_FOV;
use crate::node_cells::{node_range, representative_depth_for_spacing};

use crate::{
    CellCutEntry, CellCutError, CellCutNode, CellGrid, PAGE_SLOTS, PackedLevel, ViewProjection,
    cell_cut::{
        FastMap, FastSet, MAX_MASK_LEVELS, PrevState, ProbeBox, STEP_WAIT_FRAMES, behind_octants,
        behind_wanted, cell_cut_ranged, cell_cut_with, cell_depth_range, drawn_levels,
        in_cell_range, node_view_range, octant_wanted_depth_in, octant_wanted_depth_near,
        octants_in_front, probe_region, served_depth,
    },
    representative_depth,
};

mod active;
mod cache;
mod fallback;
mod fit;
mod hold;
mod pages;
mod plan;
mod residency;
mod seed;
mod step;

use self::{cache::*, fallback::*, fit::*, residency::*};

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
    /// (`node_view_range`, `octants_in_front`); `(0, 0)` in octant mode, which never reads them.
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
    /// Cell mode: the `(min, max)` active depths of each node of the applied cut, as last
    /// written to the records.
    applied_span: FastMap<NodeId, (u8, u8)>,
    /// Per `(node, octant)` stepping towards its wanted depth: `((drawn depth, wanted depth is
    /// finer, adjacent depth awaited), frames waited)`. Every planned frame counts, applied or
    /// retained; the key ignores the exact wanted depth, so a wanted depth flipping within the
    /// same direction does not restart the wait, and a hop (a new drawn depth) does. At
    /// `STEP_WAIT_FRAMES` the octant hops to the resident level closest to its drawn depth
    /// (ADR 0053).
    step_waits: FastMap<(NodeId, usize), StepWait>,
    /// Cell mode: per node of the applied cut that was already active in the previous cut, the
    /// depths its range residents may take: that span widened by one level at each end.
    applied_window: FastMap<NodeId, (u8, u8)>,
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
    active_rule: ActiveRule,
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

impl CellPool {
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
            applied_span: FastMap::default(),
            step_waits: FastMap::default(),
            applied_window: FastMap::default(),
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
            active_rule: ActiveRule::Window,
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

    /// Cell mode: chooses which range depths are active (`ActiveRule`). Invalidates like
    /// `set_lod_mode`: the next plan re-derives the active depths and re-sends every record.
    pub fn set_active_rule(&mut self, rule: ActiveRule) {
        if self.active_rule == rule {
            return;
        }
        self.active_rule = rule;
        self.wanted_memo = None;
        self.real_memo = None;
        self.depths.clear();
        for slot in self.mask_slots.values_mut() {
            slot.stale = true;
        }
    }

    pub fn active_rule(&self) -> ActiveRule {
        self.active_rule
    }

    /// Node depth of `bounds`: the depth whose levels span its octants.
    fn node_depth(grid: &CellGrid, bounds: SpatialBounds) -> u8 {
        representative_depth(grid, bounds).saturating_sub(3)
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
}

#[cfg(test)]
#[path = "cell_pool_tests.rs"]
mod tests;
