//! CPU residency of the GPU cell pool (R3 Task 7a): pure and deterministic, no GPU.
//!
//! The pool owns a fixed number of 8 KiB pages. Each frame the caller gives it the cut, and gets a
//! [`FramePlan`]: the cut to draw, the levels to fetch, the page uploads and frees to execute, the
//! mask writes and the active levels. Data arrives later through [`CellPool::offer`].
//!
//! Frame order: release delayed frees -> wanted cut (resident = stored) and fit (R3-Q11) -> real
//! cut on the resident levels -> apply or retain (R3-Q10) -> cancel, evict, request -> uploads.
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
//! residency: it applies when `cell_cut` is Ok and no node lacks a level, or when the applied cut
//! is itself incomplete; otherwise the previous cut stays (`RetainedPrevious`). A real cut that
//! fails with `DescendantTooFine` stays retained and is reported in the counters.
//!
//! Sizes: a level that has not arrived is estimated as `ceil(min(points, 8^gap) / 512)` pages.
//! `pack_level` may split a page when its extent would exceed 2^16, so the estimate is not a strict
//! upper bound; once offered, the exact size replaces it, the fit corrects on the next frame, and
//! the 1/8 reserve absorbs the transient.

use std::{
    cell::Cell,
    collections::{BTreeSet, HashMap, HashSet},
    ops::Range,
};

use viewer_core::{NodeId, SpatialBounds};

use crate::{
    CellCutEntry, CellCutError, CellCutNode, CellGrid, PAGE_SLOTS, PackedLevel, ViewProjection,
    cell_cut, representative_depth,
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
    levels: HashMap<LevelKey, Level>,
    caps: HashMap<NodeId, u8>,
    overflow: u32,
    applied: Vec<CellCutEntry>,
    /// The applied cut had no node without a level when it was applied.
    applied_complete: bool,
    applied_levels: HashSet<LevelKey>,
    applied_pins: HashSet<LevelKey>,
    pinned: HashSet<LevelKey>,
    mask_slots: HashMap<NodeId, (u32, [u64; 8])>,
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

/// Everything one frame computes from the arguments of `frame`.
struct Ctx<'a> {
    view: &'a ViewProjection,
    grid: &'a CellGrid,
    origin: [f64; 3],
    target_px: f64,
    evaluations: Cell<u32>,
}

impl CellPool {
    pub fn new(config: PoolConfig) -> Self {
        Self {
            config,
            frame: 0,
            free: (0..config.capacity_pages).collect(),
            pending: Vec::new(),
            levels: HashMap::new(),
            caps: HashMap::new(),
            overflow: 0,
            applied: Vec::new(),
            applied_complete: false,
            applied_levels: HashSet::new(),
            applied_pins: HashSet::new(),
            pinned: HashSet::new(),
            mask_slots: HashMap::new(),
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
        match self.levels.get_mut(&key) {
            Some(entry)
                if matches!(entry.state, LevelState::Requested)
                    && entry.data.is_none()
                    && !level.pages.is_empty()
                    && level.pages.len() <= capacity =>
            {
                entry.data = Some(level);
                true
            }
            _ => false,
        }
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

    fn evict_resident(&mut self, key: LevelKey, now: u64) {
        let Some(level) = self.levels.remove(&key) else {
            return;
        };
        if let LevelState::Resident { slots } = level.state {
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
        self.levels
            .get(&key)
            .and_then(Level::pages)
            .unwrap_or_else(|| {
                let node_depth = representative_depth(grid, node.bounds).saturating_sub(3);
                estimate_pages(node.points, key.depth, node_depth)
            })
    }

    /// Wanted entries with resident = stored (minus the offenders and below the caps). A
    /// `DescendantTooFine` adds its descendant to `offenders` and runs again.
    fn wanted_cut(
        &self,
        cut: &[NodeId],
        nodes: &HashMap<NodeId, PoolNode>,
        caps: &HashMap<NodeId, u8>,
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
            match cell_cut(cut, &lookup, ctx.view, ctx.grid, ctx.origin, ctx.target_px) {
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
        nodes: &HashMap<NodeId, PoolNode>,
        caps: &HashMap<NodeId, u8>,
    ) -> HashSet<LevelKey> {
        let mut set = HashSet::new();
        for entry in entries {
            let node = &nodes[&entry.node];
            if let Some(coarsest) = capped(&node.stored, caps.get(&entry.node)).first() {
                set.insert(self.key(entry.node, *coarsest));
            }
            for depth in entry.wanted.iter().flatten() {
                set.insert(self.key(entry.node, *depth));
            }
        }
        set
    }

    fn total_pages(
        &self,
        entries: &[CellCutEntry],
        nodes: &HashMap<NodeId, PoolNode>,
        caps: &HashMap<NodeId, u8>,
        grid: &CellGrid,
    ) -> u64 {
        self.needed_levels(entries, nodes, caps)
            .into_iter()
            .map(|key| self.pages_of(key, &nodes[&key.node], grid) as u64)
            .sum()
    }

    /// R3-Q11. Returns the wanted entries after the fit, and whether even the coarsest levels
    /// overflow the usable capacity.
    #[allow(clippy::too_many_arguments)]
    fn fit(
        &mut self,
        cut: &[NodeId],
        nodes: &HashMap<NodeId, PoolNode>,
        offenders: &mut BTreeSet<NodeId>,
        eye: [f64; 3],
        ctx: &Ctx,
    ) -> Result<(Vec<CellCutEntry>, bool), CellCutError> {
        let usable = self.usable();
        let (high, low) = (usable * HIGH_PERCENT / 100, usable * LOW_PERCENT / 100);
        let mut caps = std::mem::take(&mut self.caps);
        caps.retain(|id, _| nodes.contains_key(id));
        let mut entries = self.wanted_cut(cut, nodes, &caps, offenders, ctx)?;
        if !caps.is_empty() {
            let none = HashMap::new();
            let open = self.wanted_cut(cut, nodes, &none, offenders, ctx)?;
            if self.total_pages(&open, nodes, &none, ctx.grid) <= low {
                caps.clear();
                entries = open;
            }
        }
        let mut total = self.total_pages(&entries, nodes, &caps, ctx.grid);
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
            let position: HashMap<NodeId, usize> =
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
                total = self.total_pages(&entries, nodes, &caps, ctx.grid);
            }
        }
        self.caps = caps;
        Ok((entries, total > usable))
    }

    fn alloc_mask(&mut self) -> u32 {
        self.free_masks.pop_first().unwrap_or_else(|| {
            self.next_mask += 1;
            self.next_mask - 1
        })
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
        let ctx = Ctx {
            view,
            grid,
            origin: coordinate_origin,
            target_px,
            evaluations: Cell::new(0),
        };
        let mut nodes: HashMap<NodeId, PoolNode> = HashMap::with_capacity(cut.len());
        for id in cut {
            let data = node(*id).ok_or(CellCutError::UnknownNode(*id))?;
            if data.stored.is_empty() {
                return Err(CellCutError::NoStoredLevels(*id));
            }
            nodes.insert(*id, data);
        }
        self.frame += 1;
        let frame = self.frame;
        let mut page_frees = Vec::new();
        self.release(frame, &mut page_frees);
        let eye: [f64; 3] = view
            .camera()
            .position()
            .map(|value| value / view.visual_scale());

        // Wanted cut and fit.
        let mut offenders = BTreeSet::new();
        let (wanted, overflow) = self.fit(cut, &nodes, &mut offenders, eye, &ctx)?;
        if overflow {
            self.overflow += 1;
        }
        let caps = self.caps.clone();
        let mut needed = self.needed_levels(&wanted, &nodes, &caps);
        needed.retain(|key| !offenders.contains(&key.node));

        // Real cut on the resident levels.
        let real = {
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
            cell_cut(cut, &lookup, view, grid, coordinate_origin, target_px)
        };
        let (real, too_fine) = match real {
            Ok(entries) => (Some(entries), false),
            Err(CellCutError::DescendantTooFine { .. }) => (None, true),
            Err(other) => return Err(other),
        };
        let missing_levels = |entries: &[CellCutEntry]| {
            entries
                .iter()
                .filter(|entry| {
                    (0..8).any(|o| entry.wanted[o].is_some() && entry.drawn[o].is_none())
                })
                .count() as u32
        };
        let planned_missing = real.as_deref().map_or(cut.len() as u32, missing_levels);
        let apply =
            if real.is_some() && !overflow && (planned_missing == 0 || !self.applied_complete) {
                CutApply::Applied
            } else {
                CutApply::RetainedPrevious
            };

        // Levels in use: the drawn ones and, when retaining, the previous cut's.
        let mut protected: HashSet<LevelKey> = HashSet::new();
        let mut pinned: HashSet<LevelKey> = HashSet::new();
        if let (CutApply::Applied, Some(real)) = (apply, real) {
            self.applied = real;
            self.applied_complete = planned_missing == 0;
        }
        for entry in &self.applied {
            for depth in entry.required_levels() {
                protected.insert(self.key(entry.node, depth));
            }
        }
        for id in cut {
            if let Some(depth) = capped(&nodes[id].stored, caps.get(id)).first() {
                pinned.insert(self.key(*id, *depth));
            }
        }
        if apply == CutApply::Applied {
            self.applied_pins = pinned.clone();
        } else {
            pinned.extend(self.applied_pins.iter().copied());
        }
        // Overflow keeps only the previous cut: nothing of the new one is requested.
        let want: HashSet<LevelKey> = if overflow {
            HashSet::new()
        } else {
            needed.clone()
        };
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
            if let Some(level) = self.levels.remove(&key) {
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
        let mut drawable: HashSet<NodeId> = HashSet::new();
        for key in self
            .levels
            .iter()
            .filter(|(_, l)| l.is_resident())
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
                    && capped(&data.stored, caps.get(&key.node)).first() == Some(&key.depth);
                order.push((
                    key,
                    Need {
                        first,
                        distance: distance_sq(eye, data.bounds, coordinate_origin),
                    },
                ));
            }
        }

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

        // Uploads, spread over frames.
        order.sort_by(|(ka, a), (kb, b)| {
            (!a.first)
                .cmp(&!b.first)
                .then(a.distance.total_cmp(&b.distance))
                .then(ka.depth.cmp(&kb.depth))
                .then(ka.node.cmp(&kb.node))
        });
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
            }
        }

        // Masks and active levels of the cut being drawn.
        let mut mask_updates = Vec::new();
        let mut active = Vec::new();
        if apply == CutApply::Applied {
            let live: HashSet<NodeId> = self.applied.iter().map(|entry| entry.node).collect();
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
        let mut applied_levels = HashSet::new();
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
                .sum(),
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
