//! Pure per-node cell cut (R3 Task 3): which stored level each cut node draws, and which of its
//! 8^3 mask cells are hidden under a resident selected descendant.
//!
//! Grid, node bounds and the camera's dataset frame are one frame; `coordinate_origin` is only
//! subtracted by the view. Cut nodes must sit on the dyadic grid: each spans 8 cells per axis at
//! its representative depth, so a mask cell is one such cell.
//!
//! Octants: a node's 8 children, halved on each axis; index `o = 4x + 2y + z` with x, y, z in
//! {0, 1} (the Morton order of `node_cells`). Mask bit `x + 8y + 64z` of the 8^3 grid belongs to
//! octant `4 * (x >> 2) + 2 * (y >> 2) + (z >> 2)`. The level choice is per octant in the data
//! model and in the rule (ADR 0034): each octant asks for the coarsest stored level whose cell,
//! placed at the octant point nearest the eye, would not refine (its children would project below
//! the target, ADR 0054).

use std::{
    collections::{HashMap, HashSet},
    fmt,
    hash::{BuildHasherDefault, Hasher},
    ops::Range,
};

use viewer_core::{NodeId, SpatialBounds};

use crate::{
    ViewProjection,
    node_cells::{CellGrid, NodeCellLevel, node_range, representative_depth},
};

/// Hasher for the small integer keys of the cut and the pool (Fx): these maps are probed per node
/// per frame, where SipHash dominated the profile. Not DoS-resistant, which does not matter for
/// keys the renderer itself produces.
#[derive(Default, Clone, Copy)]
pub(crate) struct FxHasher(u64);

impl FxHasher {
    fn add(&mut self, word: u64) {
        self.0 = (self.0.rotate_left(5) ^ word).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
}

impl Hasher for FxHasher {
    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut word = [0_u8; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            self.add(u64::from_le_bytes(word));
        }
    }
    fn write_u8(&mut self, value: u8) {
        self.add(u64::from(value));
    }
    fn write_u32(&mut self, value: u32) {
        self.add(u64::from(value));
    }
    fn write_u64(&mut self, value: u64) {
        self.add(value);
    }
    fn write_usize(&mut self, value: usize) {
        self.add(value as u64);
    }
    fn finish(&self) -> u64 {
        self.0
    }
}

pub(crate) type FastMap<K, V> = HashMap<K, V, BuildHasherDefault<FxHasher>>;
pub(crate) type FastSet<K> = HashSet<K, BuildHasherDefault<FxHasher>>;

/// How a node's levels are chosen for drawing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LodMode {
    /// One level per octant; a spanning level (depth <= the node's) is drawn whole and alone.
    #[default]
    Octant,
    /// Several levels per node may draw at once, chosen per cell by the GPU: the active set is
    /// the closest resident level of each octant, restricted to the depths a node record can
    /// hold (`cell_depth_range`).
    Cell,
}

/// Depths `node_depth..=node_depth + MAX_NODE_SPAN` a node may activate in cell mode (E2).
pub(crate) fn cell_depth_range(node_depth: u8) -> (u8, u8) {
    (
        node_depth,
        node_depth.saturating_add(crate::cell_lod::MAX_NODE_SPAN),
    )
}

/// `depths` restricted to `cell_depth_range(node_depth)`.
pub(crate) fn in_cell_range(depths: &[u8], node_depth: u8) -> Vec<u8> {
    let (lo, hi) = cell_depth_range(node_depth);
    depths
        .iter()
        .copied()
        .filter(|depth| (lo..=hi).contains(depth))
        .collect()
}

/// Deepest ancestor-descendant gap (in node levels) a mask can express: 8 cells -> 1 cell.
pub(crate) const MAX_MASK_LEVELS: u8 = 3;

/// What `cell_cut` needs to know about a cut node.
#[derive(Debug, Clone, PartialEq)]
pub struct CellCutNode {
    /// Dataset frame, like the grid.
    pub bounds: SpatialBounds,
    /// Absolute depths of the stored levels, ascending.
    pub stored: Vec<u8>,
    /// Depths the node can build on demand, inclusive: `stored` lies inside it. The view wants a
    /// depth of this range; `stored` and `resident` only say what can be drawn now.
    pub buildable: (u8, u8),
    /// Absolute depths of the resident levels.
    pub resident: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellCutEntry {
    pub node: NodeId,
    /// Buildable depth the view asks for, per octant.
    pub wanted: [Option<u8>; 8],
    /// Resident depth to draw, per octant; `None` when no level is resident.
    pub drawn: [Option<u8>; 8],
    /// `drawn` is coarser than `wanted`, per octant (R3-Q3).
    pub stage: [bool; 8],
    /// Bit `x + 8y + 64z` of the node's 8^3 grid; 1 = hidden under a resident selected
    /// descendant.
    pub mask: [u64; 8],
}

impl CellCutEntry {
    /// Distinct drawn depths, ascending: the levels that must be resident.
    pub fn required_levels(&self) -> Vec<u8> {
        let mut depths: Vec<u8> = self.drawn.iter().flatten().copied().collect();
        depths.sort_unstable();
        depths.dedup();
        depths
    }

    /// Per distinct drawn depth (ascending), the mask of octants drawing it (bit `o` = octant `o`).
    pub fn draws(&self) -> Vec<(u8, u8)> {
        self.required_levels()
            .into_iter()
            .map(|depth| {
                let octants = (0..8).fold(0_u8, |bits, o| {
                    bits | (u8::from(self.drawn[o] == Some(depth)) << o)
                });
                (depth, octants)
            })
            .collect()
    }
}

/// Ranges of `level.cells` covered by the octants in `octants` (bit `o` = octant `o`), with
/// adjacent octants coalesced and empty ones dropped. A level spanning octants yields the whole
/// level once when any bit is set.
pub fn octant_ranges(level: &NodeCellLevel, octants: u8) -> Vec<Range<usize>> {
    if level.octant_offsets.is_none() {
        // A spanning level is drawn once, whole, if any octant uses it.
        let mut whole = Vec::new();
        if octants != 0 {
            whole.push(0..level.cells.len());
        }
        return whole;
    }
    let mut ranges: Vec<Range<usize>> = Vec::new();
    for o in (0..8).filter(|o| octants >> o & 1 == 1) {
        let range = level.octant_range(o);
        if range.is_empty() {
            continue;
        }
        match ranges.last_mut() {
            Some(last) if last.end == range.start => last.end = range.end,
            _ => ranges.push(range),
        }
    }
    ranges
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellCutError {
    /// A resident cut node lies more than 3 levels below a cut ancestor.
    DescendantTooFine {
        ancestor: NodeId,
        descendant: NodeId,
    },
    UnknownNode(NodeId),
    NoStoredLevels(NodeId),
}

impl fmt::Display for CellCutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DescendantTooFine {
                ancestor,
                descendant,
            } => write!(
                f,
                "node {} is more than {MAX_MASK_LEVELS} levels below cut ancestor {}",
                descendant.get(),
                ancestor.get()
            ),
            Self::UnknownNode(node) => write!(f, "cut node {} is unknown", node.get()),
            Self::NoStoredLevels(node) => write!(f, "cut node {} has no stored levels", node.get()),
        }
    }
}

impl std::error::Error for CellCutError {}

/// Octant `octant` of `bounds` (both in any one frame), split at the mid-plane of each axis.
fn octant_box(bounds: SpatialBounds, octant: usize) -> Option<SpatialBounds> {
    let mid: [f64; 3] = std::array::from_fn(|a| (bounds.min()[a] + bounds.max()[a]) / 2.0);
    let upper = |a: usize| octant >> (2 - a) & 1 == 1;
    let min = std::array::from_fn(|a| if upper(a) { mid[a] } else { bounds.min()[a] });
    let max = std::array::from_fn(|a| if upper(a) { bounds.max()[a] } else { mid[a] });
    SpatialBounds::new(min, max).ok()
}

/// The coarsest buildable depth (`buildable` is inclusive) for which a cell of that depth, placed
/// at the point of `octant` nearest the eye and shifted to stay inside it, would not refine: its
/// children would project below `target_px` (ADR 0054, the target is a minimum size); else the
/// finest (source density, RQ-09/R3-Q9). An infinite or NaN edge never stops the refinement.
///
/// The test runs relative to `coordinate_origin` (bounds minus origin, eye `position / scale`,
/// origin `[0; 3]` for the view), the frame the view itself uses, so large origins lose no
/// precision. `buildable.0 <= buildable.1`.
pub fn octant_wanted_depth(
    view: &ViewProjection,
    grid: &CellGrid,
    coordinate_origin: [f64; 3],
    bounds: SpatialBounds,
    octant: usize,
    buildable: (u8, u8),
    target_px: f64,
) -> u8 {
    debug_assert!(buildable.0 <= buildable.1, "buildable range is ordered");
    let Some(meets) = OctantProbe::new(view, grid, coordinate_origin, bounds, octant, target_px)
    else {
        return buildable.1;
    };
    (buildable.0..=buildable.1)
        .find(|depth| meets.at(*depth))
        .unwrap_or(buildable.1)
}

/// `octant_wanted_depth` searched outward from `hint` (the answer of a nearby view), assuming that
/// once a depth stops refining every finer one does too (its children are half as large at about
/// the same distance; `the_hinted_search_equals_the_full_search` checks it on random poses). It then costs 2 projections when
/// the answer has not moved, instead of one per coarser depth, and returns the same depth
/// whenever that holds. A `hint` outside the range falls back to the full search.
#[allow(clippy::too_many_arguments)]
pub(crate) fn octant_wanted_depth_near(
    view: &ViewProjection,
    grid: &CellGrid,
    coordinate_origin: [f64; 3],
    bounds: SpatialBounds,
    octant: usize,
    buildable: (u8, u8),
    target_px: f64,
    hint: u8,
) -> u8 {
    let (coarsest, finest) = buildable;
    if !(coarsest..=finest).contains(&hint) {
        return octant_wanted_depth(
            view,
            grid,
            coordinate_origin,
            bounds,
            octant,
            buildable,
            target_px,
        );
    }
    let Some(meets) = OctantProbe::new(view, grid, coordinate_origin, bounds, octant, target_px)
    else {
        return finest;
    };
    let mut depth = hint;
    if meets.at(depth) {
        while depth > coarsest && meets.at(depth - 1) {
            depth -= 1;
        }
        depth
    } else {
        while depth < finest {
            depth += 1;
            if meets.at(depth) {
                return depth;
            }
        }
        finest
    }
}

/// The stored depth that serves a wanted `depth`: the first stored one at least as fine, else the
/// finest stored. Levels are fetched and drawn from `stored` until they are built on demand.
pub(crate) fn served_depth(stored: &[u8], depth: u8) -> u8 {
    stored
        .iter()
        .copied()
        .find(|stored| *stored >= depth)
        .or_else(|| stored.last().copied())
        .expect("stored levels are not empty")
}

/// The test of `octant_wanted_depth` for one octant: does a cell of a given depth, placed at the
/// point of the octant nearest the eye and shifted to stay inside it, project to at most the
/// target (see `at`)?
struct OctantProbe<'a> {
    view: &'a ViewProjection,
    grid: &'a CellGrid,
    octant_box: SpatialBounds,
    eye: [f64; 3],
    target_px: f64,
}

impl<'a> OctantProbe<'a> {
    /// `None` when the octant has no valid box: every depth then fails and the finest is used.
    fn new(
        view: &'a ViewProjection,
        grid: &'a CellGrid,
        coordinate_origin: [f64; 3],
        bounds: SpatialBounds,
        octant: usize,
        target_px: f64,
    ) -> Option<Self> {
        let relative = SpatialBounds::new(
            std::array::from_fn(|a| bounds.min()[a] - coordinate_origin[a]),
            std::array::from_fn(|a| bounds.max()[a] - coordinate_origin[a]),
        )
        .ok()?;
        let octant_box = octant_box(relative, octant)?;
        let eye: [f64; 3] = view
            .camera()
            .position()
            .map(|value| value / view.visual_scale());
        Some(Self {
            view,
            grid,
            octant_box,
            eye,
            target_px,
        })
    }

    /// Would a cell of `depth` at the probe point stop here (ADR 0054)? Its children would project
    /// below the target: the child at the corner farthest along the view direction, the smallest
    /// on screen, is measured. A child with a corner at or behind the eye measures `+inf`, so it
    /// never stops the refinement. The length minimum is applied by the pool (`continuity_depth`).
    fn at(&self, depth: u8) -> bool {
        let side = self.grid.root_side / 2.0_f64.powi(i32::from(depth));
        let min: [f64; 3] = std::array::from_fn(|a| {
            let low = self.octant_box.min()[a];
            let high = (self.octant_box.max()[a] - side).max(low);
            self.eye[a]
                .clamp(self.octant_box.min()[a], self.octant_box.max()[a])
                .clamp(low, high)
        });
        let half = side / 2.0;
        let forward = self.view.camera().forward();
        let far: [f64; 3] = std::array::from_fn(|a| {
            if forward[a] > 0.0 {
                min[a] + half
            } else {
                min[a]
            }
        });
        SpatialBounds::new(far, far.map(|value| value + half)).is_ok_and(|child| {
            self.view.projected_voxel_edge_pixels(child, [0.0; 3]) < self.target_px
        })
    }
}

/// Is the 4^3 block of `octant` fully hidden in `mask`? Bit `x + 8y` of word `z`: the block is a
/// 4x4 patch (`x`, `y`) in each of 4 words (`z`).
fn octant_fully_masked(mask: &[u64; 8], octant: usize) -> bool {
    let (x_half, y_half, z_half) = (octant >> 2 & 1, octant >> 1 & 1, octant & 1);
    let patch = 0x0F0F_0F0F_u64 << (4 * x_half) << (32 * y_half);
    mask[4 * z_half..4 * z_half + 4]
        .iter()
        .all(|word| word & patch == patch)
}

/// `(depth, is_stage)`: the wanted level if resident, else the resident level closest to it
/// (R3-Q21); on a tie the finer one. It is a stage only when the level is coarser than wanted.
pub fn resident_depth(wanted: u8, resident: &[u8]) -> Option<(u8, bool)> {
    // The nearest resident level; on a tie the finer one (R3-Q21).
    resident
        .iter()
        .copied()
        .min_by_key(|depth| (depth.abs_diff(wanted), std::cmp::Reverse(*depth)))
        .map(|depth| (depth, depth < wanted))
}

/// `(drawn, stage)` of a node from its `wanted` depths and `resident` levels, as `cell_cut`
/// decides them; `node_depth` is the depth whose levels span octants. A wanted depth that is not
/// `stored` is drawn from the level `served_depth` picks, and it is a stage while that level is
/// coarser than the wanted depth itself.
pub(crate) fn drawn_levels(
    wanted: &[Option<u8>; 8],
    stored: &[u8],
    resident: &[u8],
    node_depth: u8,
    mode: LodMode,
) -> ([Option<u8>; 8], [bool; 8]) {
    let mut drawn = [None; 8];
    let mut stage = [false; 8];
    if mode == LodMode::Cell {
        // No level below the node or beyond its span can draw, and no depth is forced on the
        // others: the node draws the closest resident level of each octant.
        let stored = in_cell_range(stored, node_depth);
        let resident = in_cell_range(resident, node_depth);
        if stored.is_empty() {
            return (drawn, stage);
        }
        for o in 0..8 {
            if let Some(w) = wanted[o]
                && let Some((depth, _)) = resident_depth(served_depth(&stored, w), &resident)
            {
                drawn[o] = Some(depth);
                stage[o] = depth < w;
            }
        }
        return (drawn, stage);
    }
    for o in 0..8 {
        if let Some(w) = wanted[o]
            && let Some((depth, _)) = resident_depth(served_depth(stored, w), resident)
        {
            drawn[o] = Some(depth);
            stage[o] = depth < w;
        }
    }
    // A spanning level is drawn whole, so it must be the only level the node draws:
    // otherwise its cells would overlap the finer ones of the other octants.
    let spanning = drawn.iter().flatten().copied().find(|d| *d <= node_depth);
    if let Some(spanning) = spanning {
        for o in 0..8 {
            if let Some(w) = wanted[o] {
                drawn[o] = Some(spanning);
                stage[o] = spanning < w;
            }
        }
    }
    (drawn, stage)
}

struct Placed {
    rep: u8,
    lo: [u64; 3],
    hi: [u64; 3],
}

/// Chooses a level per cut node and builds the masks; output follows the order of `cut`.
pub fn cell_cut(
    cut: &[NodeId],
    node: &dyn Fn(NodeId) -> Option<CellCutNode>,
    view: &ViewProjection,
    grid: &CellGrid,
    coordinate_origin: [f64; 3],
    target_px: f64,
) -> Result<Vec<CellCutEntry>, CellCutError> {
    cell_cut_with(cut, node, grid, LodMode::Octant, &|_, octant, data| {
        octant_wanted_depth(
            view,
            grid,
            coordinate_origin,
            data.bounds,
            octant,
            data.buildable,
            target_px,
        )
    })
}

/// `cell_cut` with the per-octant depth of the view supplied by `wanted_depth(node, octant,
/// data)`, which must return what `octant_wanted_depth` would for `data.buildable`. It lets a caller
/// that evaluates the same view several times (the cell pool) compute each depth once; the output
/// is identical to `cell_cut` when the depths are.
pub(crate) fn cell_cut_with(
    cut: &[NodeId],
    node: &dyn Fn(NodeId) -> Option<CellCutNode>,
    grid: &CellGrid,
    mode: LodMode,
    wanted_depth: &dyn Fn(NodeId, usize, &CellCutNode) -> u8,
) -> Result<Vec<CellCutEntry>, CellCutError> {
    let mut entries = Vec::with_capacity(cut.len());
    let mut nodes = Vec::with_capacity(cut.len());
    let mut placed = Vec::with_capacity(cut.len());
    let mut has_resident = Vec::with_capacity(cut.len());
    for id in cut {
        let data = node(*id).ok_or(CellCutError::UnknownNode(*id))?;
        if data.stored.is_empty() {
            return Err(CellCutError::NoStoredLevels(*id));
        }
        entries.push(CellCutEntry {
            node: *id,
            wanted: [None; 8],
            drawn: [None; 8],
            stage: [false; 8],
            mask: [0; 8],
        });
        let rep = representative_depth(grid, data.bounds);
        let range: [(u64, u64); 3] =
            std::array::from_fn(|axis| node_range(grid, data.bounds, rep, axis));
        placed.push(Placed {
            rep,
            lo: range.map(|(lo, _)| lo),
            hi: range.map(|(_, hi)| hi),
        });
        // Cell mode: a node whose resident levels all lie outside its span draws nothing, so it
        // must not hide its ancestors' cells.
        has_resident.push(if mode == LodMode::Cell {
            !in_cell_range(&data.resident, rep.saturating_sub(MAX_MASK_LEVELS)).is_empty()
        } else {
            !data.resident.is_empty()
        });
        nodes.push(data);
    }

    let mut by_cell: FastMap<(u8, [u64; 3]), usize> =
        FastMap::with_capacity_and_hasher(cut.len(), BuildHasherDefault::default());
    let mut present = [false; 256];
    for (index, node) in placed.iter().enumerate() {
        by_cell.entry((node.rep, node.lo)).or_insert(index);
        present[usize::from(node.rep)] = true;
    }
    for (child, d) in placed.iter().enumerate() {
        if !has_resident[child] {
            continue;
        }
        // Ancestors are nodes with a smaller representative depth (>= 3) whose aligned 8-cell
        // block contains this node's first cell.
        for k in 1..=d.rep.saturating_sub(3) {
            if !present[usize::from(d.rep - k)] {
                continue;
            }
            let ancestor_lo = d.lo.map(|c| (c >> k) & !7);
            let Some(&parent) = by_cell.get(&(d.rep - k, ancestor_lo)) else {
                continue;
            };
            if k > MAX_MASK_LEVELS {
                return Err(CellCutError::DescendantTooFine {
                    ancestor: entries[parent].node,
                    descendant: entries[child].node,
                });
            }
            let span: [(u64, u64); 3] = std::array::from_fn(|axis| {
                (
                    (d.lo[axis] >> k) - ancestor_lo[axis],
                    (d.hi[axis] >> k) - ancestor_lo[axis],
                )
            });
            let mask = &mut entries[parent].mask;
            for z in span[2].0..=span[2].1.min(7) {
                for y in span[1].0..=span[1].1.min(7) {
                    for x in span[0].0..=span[0].1.min(7) {
                        mask[z as usize] |= 1 << (x + 8 * y);
                    }
                }
            }
        }
    }

    for ((entry, data), placed) in entries.iter_mut().zip(&nodes).zip(&placed) {
        let finest = data.buildable.1;
        let node_depth = placed.rep.saturating_sub(MAX_MASK_LEVELS);
        // A level at depth <= the node's spans octants and cannot be split; when no finer level is
        // buildable, the whole node uses the finest one.
        let spans_all = finest <= node_depth;
        // Cell mode: a wanted depth is clamped to the usable range; nothing below the node is
        // wanted, and a node with no buildable depth inside the range keeps the view's depth
        // (nothing of it can draw, see `drawn_levels`).
        let cell_range = {
            let (lo, hi) = cell_depth_range(node_depth);
            let (lo, hi) = (lo.max(data.buildable.0), hi.min(data.buildable.1));
            (mode == LodMode::Cell && lo <= hi).then_some((lo, hi))
        };
        for o in 0..8 {
            if octant_fully_masked(&entry.mask, o) {
                continue;
            }
            if mode == LodMode::Cell {
                let w = wanted_depth(entry.node, o, data);
                entry.wanted[o] = Some(cell_range.map_or(w, |(lo, hi)| w.clamp(lo, hi)));
                continue;
            }
            let mut wanted = if spans_all {
                finest
            } else {
                wanted_depth(entry.node, o, data)
            };
            if wanted <= node_depth && !spans_all {
                wanted = (data.buildable.0..=data.buildable.1)
                    .find(|depth| *depth > node_depth)
                    .expect("a finer buildable level exists");
            }
            entry.wanted[o] = Some(wanted);
        }
        (entry.drawn, entry.stage) = drawn_levels(
            &entry.wanted,
            &data.stored,
            &data.resident,
            node_depth,
            mode,
        );
    }
    Ok(entries)
}

/// FNV-1a 64-bit digest of every entry in order.
pub fn cell_cut_digest(entries: &[CellCutEntry]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    let mut feed = |bytes: &[u8]| {
        for byte in bytes {
            hash = (hash ^ u64::from(*byte)).wrapping_mul(PRIME);
        }
    };
    feed(&(entries.len() as u64).to_le_bytes());
    for entry in entries {
        feed(&entry.node.get().to_le_bytes());
        let uniform = entry.wanted[0].is_some()
            && entry.wanted.iter().all(|w| *w == entry.wanted[0])
            && entry.drawn.iter().all(|d| *d == entry.drawn[0])
            && entry.stage.iter().all(|s| *s == entry.stage[0]);
        // A uniform entry feeds the node-level bytes of the former layout; per-octant values
        // follow when they differ or an octant is hidden (`None` is tagged apart from `Some(0)`).
        feed(&[entry.wanted[0].unwrap_or(0)]);
        match entry.drawn[0] {
            Some(depth) => feed(&[1, depth]),
            None => feed(&[0, 0]),
        }
        feed(&[u8::from(entry.stage[0])]);
        for word in entry.mask {
            feed(&word.to_le_bytes());
        }
        if !uniform {
            for o in 0..8 {
                match entry.wanted[o] {
                    Some(depth) => feed(&[1, depth]),
                    None => feed(&[0, 0]),
                }
                match entry.drawn[o] {
                    Some(depth) => feed(&[1, depth]),
                    None => feed(&[0, 0]),
                }
                feed(&[u8::from(entry.stage[o])]);
            }
        }
    }
    hash
}

#[cfg(test)]
#[path = "cell_cut_tests.rs"]
mod tests;
