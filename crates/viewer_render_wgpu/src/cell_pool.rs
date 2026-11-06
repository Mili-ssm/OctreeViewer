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
//! The octants of covered nodes count as stages. Requests and uploads go coarse to fine: the first
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

use std::{cell::Cell, collections::BTreeSet, ops::Range, rc::Rc};

use viewer_core::{NodeId, SpatialBounds};

use crate::camera::VERTICAL_FOV;
use crate::node_cells::node_range;

use crate::{
    CellCutEntry, CellCutError, CellCutNode, CellGrid, PAGE_SLOTS, PackedLevel, ViewProjection,
    cell_cut::{
        FastMap, FastSet, MAX_MASK_LEVELS, cell_cut_with, octant_wanted_depth,
        octant_wanted_depth_near,
    },
    representative_depth,
};

/// Bytes of one pool page.
pub const POOL_PAGE_BYTES: u64 = (PAGE_SLOTS * 16) as u64;
/// 256 MiB (ADR 0033).
pub const DEFAULT_POOL_PAGES: u32 = 32_768;
/// Default upload budget per frame.
pub const DEFAULT_UPLOAD_BUDGET_BYTES: u64 = 16 << 20;

/// The fit starts coarsening above this share of the usable capacity and relaxes below `LOW`.
const HIGH_PERCENT: u64 = 90;
const LOW_PERCENT: u64 = 80;
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
    /// Points of the node, for the size estimate of levels that have not arrived.
    pub points: u64,
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveLevel {
    pub key: LevelKey,
    /// Bit `o` = octant `o` draws this level.
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FramePlan {
    pub apply: CutApply,
    /// The cut to draw: the new one, or the retained one.
    pub entries: Vec<CellCutEntry>,
    pub fetch_requests: Vec<LevelKey>,
    pub uploads: Vec<PageUpload>,
    /// Page slots to make inert; they are free for the uploads of this same frame.
    pub page_frees: Vec<u32>,
    pub mask_updates: Vec<MaskUpdate>,
    pub active: Vec<ActiveLevel>,
    pub events: Vec<CellPoolEvent>,
    pub counters: PoolCounters,
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
}

/// Result of the wanted cut and the fit, valid while its inputs are.
struct WantedMemo {
    cut: Vec<NodeId>,
    size_epoch: u64,
    generation: u64,
    needed: Rc<FastSet<LevelKey>>,
    overflow: bool,
    /// Eye the fit ranked the nodes from, and the nearest eye-to-node view depth of the cut:
    /// while caps are active the memo is dropped when the eye moves past the drift limit.
    eye: [f64; 3],
    min_depth: f64,
    had_caps: bool,
}

/// Result of the real cut, valid for one wanted generation and one residency epoch.
struct RealMemo {
    generation: u64,
    residency_epoch: u64,
    /// `None`: the cut failed with `DescendantTooFine`.
    entries: Option<Vec<CellCutEntry>>,
    planned_missing: u32,
    /// Cut nodes without a level that no drawing ancestor covers (R3-Q19).
    uncovered: u32,
    /// Unmasked octants of the covered nodes without a level: they draw a coarser ancestor.
    covered_octants: u32,
    /// Fallback ancestors the real cut was computed with.
    fallback: Vec<NodeId>,
    /// `self.applied` already holds `entries`.
    applied: bool,
}

/// The nodes of the applied cut, as the fallback of R3-Q19 needs them.
struct AppliedIndex {
    nodes: FastMap<NodeId, PoolNode>,
    /// `(representative depth, first cell)` -> node, like `cell_cut` finds ancestors.
    cells: FastMap<(u8, [u64; 3]), NodeId>,
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
    ids: Vec<NodeId>,
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
        let base = self.ancestor(k).1;
        let span: [(usize, usize); 3] = std::array::from_fn(|axis| {
            (
                ((self.lo[axis] >> k) - base[axis]).min(7) as usize,
                ((self.hi[axis] >> k) - base[axis]).min(7) as usize,
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
    data: Option<PackedLevel>,
    last_used: u64,
    /// Last frame this level was in `active`; 0 when never.
    last_active: u64,
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
    distance: f64,
}

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
    /// `RealMemo::covered_octants` of the applied cut.
    applied_covered: u32,
    applied_levels: FastSet<LevelKey>,
    applied_pins: FastSet<LevelKey>,
    pinned: FastSet<LevelKey>,
    mask_slots: FastMap<NodeId, (u32, [u64; 8])>,
    free_masks: BTreeSet<u32>,
    next_mask: u32,
    events: Vec<CellPoolEvent>,
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
    /// `octant_wanted_depth` of the octant for `data.stored`, from the cache of the full list: the
    /// first stored depth that meets the target, else the finest, so a prefix of the list gets
    /// the same depth capped at its last element.
    fn depth(&self, id: NodeId, octant: usize, data: &CellCutNode) -> u8 {
        let last = *data.stored.last().expect("stored levels are not empty");
        self.depths[&id].depths[octant].min(last)
    }
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
                grid: None,
                epoch: 0,
            },
            fallback_memo: None,
            applied_covered: 0,
            applied_levels: FastSet::default(),
            applied_pins: FastSet::default(),
            pinned: FastSet::default(),
            mask_slots: FastMap::default(),
            free_masks: BTreeSet::new(),
            next_mask: 0,
            events: Vec::new(),
        }
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
        self.levels.get(&key)?.data.as_ref()
    }

    pub fn resident_pages(&self) -> u32 {
        self.levels
            .values()
            .filter(|level| level.is_resident())
            .filter_map(Level::pages)
            .sum::<usize>() as u32
    }

    /// Delivers the data of a requested level. A level nobody wants any more, one that already has
    /// data, or one larger than the pool is dropped (`false`).
    pub fn offer(&mut self, key: LevelKey, level: PackedLevel) -> bool {
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
    fn known_pages(&self, key: LevelKey) -> Option<usize> {
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
        self.applied.clear();
        self.applied_covered = 0;
        self.applied_levels.clear();
        self.applied_pins.clear();
        self.pinned.clear();
        self.applied_index.nodes.clear();
        self.applied_index.cells.clear();
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
        u64::from(self.config.capacity_pages - self.config.capacity_pages / 8)
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
                    stored,
                    resident,
                })
            };
            ctx.evaluations.set(ctx.evaluations.get() + 1);
            match cell_cut_with(cut, &lookup, ctx.grid, &|id, o, data| {
                ctx.depth(id, o, data)
            }) {
                Err(CellCutError::DescendantTooFine { descendant, .. })
                    if !offenders.contains(&descendant) =>
                {
                    offenders.insert(descendant);
                }
                other => return other,
            }
        }
    }

    /// Distinct levels the entries need: the wanted ones plus the pinned coarsest of every node.
    fn needed_levels(
        &self,
        entries: &[CellCutEntry],
        nodes: &FastMap<NodeId, PoolNode>,
        caps: &FastMap<NodeId, u8>,
    ) -> FastSet<LevelKey> {
        let mut set = FastSet::default();
        for entry in entries {
            let node = &nodes[&entry.node];
            if let Some(coarsest) = coarsest(&node.stored, caps.get(&entry.node)) {
                set.insert(self.key(entry.node, coarsest));
            }
            for depth in entry.wanted.iter().flatten() {
                set.insert(self.key(entry.node, *depth));
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
    ) -> (FastSet<LevelKey>, u64) {
        let needed = self.needed_levels(entries, nodes, caps);
        let total = needed
            .iter()
            .map(|key| self.pages_of(*key, &nodes[&key.node], grid) as u64)
            .sum();
        (needed, total)
    }

    /// R3-Q11. Returns the levels the wanted entries need after the fit, and whether even the
    /// coarsest levels overflow the usable capacity.
    #[allow(clippy::too_many_arguments)]
    fn fit(
        &mut self,
        cut: &[NodeId],
        nodes: &FastMap<NodeId, PoolNode>,
        offenders: &mut BTreeSet<NodeId>,
        eye: [f64; 3],
        ctx: &Ctx,
    ) -> Result<(FastSet<LevelKey>, bool), CellCutError> {
        let usable = self.usable();
        let (high, low) = (usable * HIGH_PERCENT / 100, usable * LOW_PERCENT / 100);
        let mut caps = std::mem::take(&mut self.caps);
        caps.retain(|id, _| nodes.contains_key(id));
        let mut entries = self.wanted_cut(cut, nodes, &caps, offenders, ctx)?;
        let (mut needed, mut total) = self.needed_and_pages(&entries, nodes, &caps, ctx.grid);
        // Lifting the caps needs the uncapped cut to fit under `low`. This runs only when the
        // wanted cut is recomputed (cut, sizes or depths changed), never on a reused frame; sizes
        // are not monotone in depth, so the capped total says nothing about the uncapped one.
        if !caps.is_empty() {
            let none = FastMap::default();
            let open = self.wanted_cut(cut, nodes, &none, offenders, ctx)?;
            let (open_needed, open_total) = self.needed_and_pages(&open, nodes, &none, ctx.grid);
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
                    // The finest level below `top` that is smaller; estimates can tie.
                    let size =
                        |depth: u8| self.pages_of(self.key(id, depth), &nodes[&id], ctx.grid);
                    if let Some(below) = stored
                        .iter()
                        .copied()
                        .filter(|d| *d < top && size(*d) < size(top))
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
                (needed, total) = self.needed_and_pages(&entries, nodes, &caps, ctx.grid);
            }
        }
        self.caps = caps;
        Ok((needed, total > usable))
    }

    /// Does any stored level of the node, below its fit cap, have pages resident?
    fn has_resident_level(&self, id: NodeId, stored: &[u8]) -> bool {
        let cap = self.caps.get(&id);
        stored
            .iter()
            .filter(|depth| cap.is_none_or(|cap| *depth <= cap))
            .any(|depth| self.is_resident(self.key(id, *depth)))
    }

    /// R3-Q19: for each cut node without a resident level that no cut ancestor with a level
    /// covers, its nearest ancestor (gap <= 3) from the applied cut that has a resident level and
    /// lies at most `MAX_MASK_LEVELS` above every resident cut node under it (a deeper one would
    /// make the real cut fail with `DescendantTooFine`). Levels beyond the fit cap do not count.
    /// The caps are those of the previous frame's fit; a change retains one frame at most.
    fn fallback_ancestors(
        &mut self,
        cut: &[NodeId],
        nodes: &FastMap<NodeId, PoolNode>,
        grid: &CellGrid,
    ) -> Vec<NodeId> {
        if let Some(memo) = &self.fallback_memo
            && memo.cut == cut
            && memo.caps == self.caps
            && memo.residency_epoch == self.residency_epoch
            && memo.applied_epoch == self.applied_index.epoch
            && memo.grid == *grid
        {
            return memo.ids.clone();
        }
        let ids = self.compute_fallback(cut, nodes, grid);
        self.fallback_memo = Some(FallbackMemo {
            cut: cut.to_vec(),
            caps: self.caps.clone(),
            residency_epoch: self.residency_epoch,
            applied_epoch: self.applied_index.epoch,
            grid: *grid,
            ids: ids.clone(),
        });
        ids
    }

    fn compute_fallback(
        &self,
        cut: &[NodeId],
        nodes: &FastMap<NodeId, PoolNode>,
        grid: &CellGrid,
    ) -> Vec<NodeId> {
        if self.applied_index.nodes.is_empty() || self.applied_index.grid != Some(*grid) {
            return Vec::new();
        }
        let (with_level, missing): (Vec<NodeId>, Vec<NodeId>) = cut
            .iter()
            .partition(|id| self.has_resident_level(**id, &nodes[*id].stored));
        if missing.is_empty() {
            return Vec::new();
        }
        let in_cut: FastSet<NodeId> = cut.iter().copied().collect();
        // Cells that hold a resident cut node more than `MAX_MASK_LEVELS` below them.
        let mut too_deep: FastSet<(u8, [u64; 3])> = FastSet::default();
        for id in &with_level {
            let f = Footprint::new(grid, nodes[id].bounds);
            for k in MAX_MASK_LEVELS + 1..=f.rep.saturating_sub(3) {
                too_deep.insert(f.ancestor(k));
            }
        }
        let drawing: FastSet<(u8, [u64; 3])> = with_level
            .iter()
            .map(|id| {
                let f = Footprint::new(grid, nodes[id].bounds);
                (f.rep, f.lo)
            })
            .collect();
        let mut out = Vec::new();
        let mut seen: FastSet<NodeId> = FastSet::default();
        for id in missing {
            let f = Footprint::new(grid, nodes[&id].bounds);
            let mut nearest = None;
            let mut covered = false;
            for k in f.gaps() {
                let cell = f.ancestor(k);
                if drawing.contains(&cell) {
                    covered = true;
                    break;
                }
                if nearest.is_none()
                    && let Some(&ancestor) = self.applied_index.cells.get(&cell)
                    && !in_cut.contains(&ancestor)
                    && !too_deep.contains(&cell)
                    && self
                        .has_resident_level(ancestor, &self.applied_index.nodes[&ancestor].stored)
                {
                    nearest = Some(ancestor);
                }
            }
            if let Some(ancestor) = nearest.filter(|_| !covered)
                && seen.insert(ancestor)
            {
                out.push(ancestor);
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
        for entry in &self.applied {
            let data = &nodes[&entry.node];
            let f = Footprint::new(grid, data.bounds);
            index.cells.entry((f.rep, f.lo)).or_insert(entry.node);
            index.nodes.insert(entry.node, data.clone());
        }
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
    ) -> (u32, bool, bool) {
        let settled = self.last_pose == Some(pose);
        self.last_pose = Some(pose);
        let key = ViewKey {
            target_px,
            viewport: view.viewport(),
            visual_scale: view.visual_scale(),
            grid: *grid,
            origin,
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
            let depths = std::array::from_fn(|octant| match hints {
                Some(hints) => octant_wanted_depth_near(
                    view,
                    grid,
                    origin,
                    data.bounds,
                    octant,
                    &data.stored,
                    target_px,
                    hints[octant],
                ),
                None => octant_wanted_depth(
                    view,
                    grid,
                    origin,
                    data.bounds,
                    octant,
                    &data.stored,
                    target_px,
                ),
            });
            let differs = was.is_none_or(|cached| cached.depths != depths);
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
        let fallback = self.fallback_ancestors(cut, &nodes, grid);
        for id in &fallback {
            nodes.insert(*id, self.applied_index.nodes[id].clone());
        }
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
        let (refreshed, depths_changed, fallback_changed) = self.refresh_depths(
            cut,
            &fallback,
            &nodes,
            pose,
            view,
            grid,
            coordinate_origin,
            target_px,
        );
        let depths = std::mem::take(&mut self.depths);
        let result = self.plan(
            cut,
            &fallback,
            &nodes,
            &depths,
            (refreshed, depths_changed, fallback_changed),
            view,
            grid,
            coordinate_origin,
        );
        self.depths = depths;
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn plan(
        &mut self,
        cut: &[NodeId],
        fallback: &[NodeId],
        nodes: &FastMap<NodeId, PoolNode>,
        depths: &FastMap<NodeId, NodeDepths>,
        (refreshed, depths_changed, fallback_changed): (u32, bool, bool),
        view: &ViewProjection,
        grid: &CellGrid,
        coordinate_origin: [f64; 3],
    ) -> Result<FramePlan, CellCutError> {
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
        let mut page_frees = Vec::new();
        self.release(frame, &mut page_frees);

        // Wanted cut and fit, reused while nothing they read has changed.
        let reusable = !depths_changed
            && self.wanted_memo.as_ref().is_some_and(|memo| {
                let moved = distance(memo.eye, eye);
                memo.cut == cut
                    && memo.size_epoch == self.size_epoch
                    && !(memo.had_caps && moved > DRIFT_FRACTION * memo.min_depth && moved > 0.0)
            });
        if !reusable {
            let mut offenders = BTreeSet::new();
            let (mut needed, overflow) = self.fit(cut, nodes, &mut offenders, eye, &ctx)?;
            needed.retain(|key| !offenders.contains(&key.node));
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
                overflow,
                eye,
                min_depth,
                had_caps: !self.caps.is_empty(),
            });
        }
        let memo = self.wanted_memo.as_ref().expect("set above");
        let (overflow, generation, needed) =
            (memo.overflow, memo.generation, Rc::clone(&memo.needed));
        if overflow {
            self.overflow += 1;
        }
        let caps = self.caps.clone();

        // Real cut on the resident levels.
        let real_reused = !fallback_changed
            && self.real_memo.as_ref().is_some_and(|memo| {
                memo.generation == generation
                    && memo.residency_epoch == self.residency_epoch
                    && memo.fallback == fallback
            });
        if !real_reused {
            let joined: Vec<NodeId>;
            let real_cut: &[NodeId] = if fallback.is_empty() {
                cut
            } else {
                joined = cut.iter().chain(fallback).copied().collect();
                &joined
            };
            let lookup = |id: NodeId| {
                let data = nodes.get(&id)?;
                let stored = capped(&data.stored, caps.get(&id));
                let resident = stored
                    .iter()
                    .copied()
                    .filter(|depth| self.is_resident(self.key(id, *depth)))
                    .collect();
                Some(CellCutNode {
                    bounds: data.bounds,
                    stored,
                    resident,
                })
            };
            ctx.evaluations.set(ctx.evaluations.get() + 1);
            let entries = match cell_cut_with(real_cut, &lookup, grid, &|id, o, data| {
                ctx.depth(id, o, data)
            }) {
                Ok(entries) => Some(entries),
                Err(CellCutError::DescendantTooFine { .. }) => None,
                Err(other) => return Err(other),
            };
            let planned_missing = entries.as_deref().map_or(cut.len() as u32, missing_levels);
            let (uncovered, covered_octants) =
                entries.as_deref().map_or((cut.len() as u32, 0), |entries| {
                    uncovered_levels(entries, nodes, grid)
                });
            self.real_memo = Some(RealMemo {
                generation,
                residency_epoch: self.residency_epoch,
                entries,
                planned_missing,
                uncovered,
                covered_octants,
                fallback: fallback.to_vec(),
                applied: false,
            });
        }
        let memo = self.real_memo.as_mut().expect("set above");
        let (too_fine, planned_missing) = (memo.entries.is_none(), memo.planned_missing);
        let apply = if !too_fine && !overflow && memo.uncovered == 0 {
            CutApply::Applied
        } else {
            CutApply::RetainedPrevious
        };

        // Levels in use: the drawn ones and, when retaining, the previous cut's.
        let mut protected: FastSet<LevelKey> = FastSet::default();
        let mut pinned: FastSet<LevelKey> = FastSet::default();
        if apply == CutApply::Applied {
            let memo = self.real_memo.as_mut().expect("set above");
            let fresh = !memo.applied;
            memo.applied = true;
            self.applied_covered = memo.covered_octants;
            if fresh {
                self.applied = memo.entries.clone().expect("applied cut exists");
                self.index_applied(nodes, grid);
            }
        }
        for entry in &self.applied {
            for depth in entry.required_levels() {
                protected.insert(self.key(entry.node, depth));
            }
        }
        for id in cut.iter().chain(fallback) {
            if let Some(depth) = coarsest(&nodes[id].stored, caps.get(id)) {
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
        protected.extend(want.iter().copied());
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
            drawable.insert(key.node);
        }
        let mut wanted_keys: Vec<LevelKey> = want.iter().copied().collect();
        wanted_keys.sort();
        for key in wanted_keys {
            let data = &nodes[&key.node];
            if let std::collections::hash_map::Entry::Vacant(slot) = self.levels.entry(key) {
                slot.insert(Level {
                    state: LevelState::Requested,
                    data: None,
                    last_used: frame,
                    last_active: 0,
                });
                fetch_requests.push(key);
            }
            if !self.levels[&key].is_resident() {
                let first = !drawable.contains(&key.node)
                    && coarsest(&data.stored, caps.get(&key.node)) == Some(key.depth);
                order.push((
                    key,
                    Need {
                        first,
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
        self.release(frame, &mut page_frees);

        // Uploads, spread over frames, in the same order.
        let mut budget = (self.config.upload_budget_bytes / POOL_PAGE_BYTES).max(1) as usize;
        let mut uploads = Vec::new();
        let mut uploaded_pages = 0_usize;
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
                if let Some((slot, _)) = self.mask_slots.remove(&id) {
                    self.free_masks.insert(slot);
                }
            }
            for index in 0..self.applied.len() {
                let (id, mask) = (self.applied[index].node, self.applied[index].mask);
                match self.mask_slots.get(&id) {
                    Some((_, old)) if *old == mask => {}
                    Some((slot, _)) => {
                        let slot = *slot;
                        self.mask_slots.insert(id, (slot, mask));
                        mask_updates.push(MaskUpdate { slot, mask });
                    }
                    None => {
                        let slot = self.alloc_mask();
                        self.mask_slots.insert(id, (slot, mask));
                        mask_updates.push(MaskUpdate { slot, mask });
                    }
                }
            }
        }
        let mut applied_levels = FastSet::default();
        for entry in &self.applied {
            let Some((slot, _)) = self.mask_slots.get(&entry.node) else {
                continue;
            };
            for (depth, bits) in entry.draws() {
                let key = self.key(entry.node, depth);
                if !self.is_resident(key) {
                    continue;
                }
                applied_levels.insert(key);
                active.push(ActiveLevel {
                    key,
                    octant_bits: bits,
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

        let counters = PoolCounters {
            stages: self
                .applied
                .iter()
                .map(|entry| entry.stage.iter().filter(|s| **s).count() as u32)
                .sum::<u32>()
                + self.applied_covered,
            nodes_without_level: if self.applied.is_empty() {
                planned_missing
            } else {
                missing_levels(&self.applied)
            },
            pool_coarsened: caps.len() as u32,
            pool_overflow: self.overflow,
            resident_pages: self.resident_pages(),
            cut_evaluations: ctx.evaluations.get(),
            descendant_too_fine: too_fine,
            uploaded_bytes: uploaded_pages as u64 * POOL_PAGE_BYTES,
            depth_refreshes: refreshed,
            wanted_reused: reusable,
            real_reused,
        };
        Ok(FramePlan {
            apply,
            entries: self.applied.clone(),
            fetch_requests,
            uploads,
            page_frees,
            mask_updates,
            active,
            events: std::mem::take(&mut self.events),
            counters,
        })
    }
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

/// `stored` truncated to depths at or below the cap.
fn capped(stored: &[u8], cap: Option<&u8>) -> Vec<u8> {
    stored
        .iter()
        .copied()
        .filter(|depth| cap.is_none_or(|cap| depth <= cap))
        .collect()
}

#[cfg(test)]
#[path = "cell_pool_tests.rs"]
mod tests;
