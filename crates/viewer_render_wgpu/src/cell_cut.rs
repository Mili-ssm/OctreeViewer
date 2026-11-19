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
    node_cells::{
        CellGrid, NodeCellLevel, node_range, representative_depth, representative_depth_for_spacing,
    },
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
    /// The depth each octant waits for to take its next step towards `wanted` (ADR 0053): the
    /// adjacent depth, drawn once resident. `None` when the octant is not stepping.
    pub step: [Option<u8>; 8],
    /// The depths `(lo, hi)` the node spans on screen, `lo <= hi`. `hi` is the
    /// finest near depth of its octants wholly in front of the eye plane, `lo` the depth wanted at its corner
    /// farthest along the view (ADR 0054 rule). `None` for a node with no
    /// visible octant and for one with no buildable depth inside its span.
    pub range: Option<(u8, u8)>,
    /// Node-level fields of plan task 10 (`cell_pool/run.rs`), computed by the pool next to the
    /// per-octant ones and read by nobody yet: the anchor depth, its target, the single active
    /// run, whether the node is staged and the depth that extends the run. `None`/`false` out of
    /// the pool.
    pub node_anchor: Option<u8>,
    /// The finest depth the node can reach (`min(hi, finest stored or buildable depth, fit cap)`).
    pub node_target: Option<u8>,
    pub node_run: Option<(u8, u8)>,
    pub node_stage: bool,
    pub node_step: Option<u8>,
    /// Bit `x + 8y + 64z` of the node's 8^3 grid; 1 = hidden under a resident selected
    /// descendant.
    pub mask: [u64; 8],
}

impl CellCutEntry {
    /// Is the 4^3 block of `octant` fully hidden under resident selected descendants?
    pub fn octant_hidden(&self, octant: usize) -> bool {
        octant_fully_masked(&self.mask, octant)
    }

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
pub(crate) fn octant_box(bounds: SpatialBounds, octant: usize) -> Option<SpatialBounds> {
    let mid: [f64; 3] = std::array::from_fn(|a| (bounds.min()[a] + bounds.max()[a]) / 2.0);
    let upper = |a: usize| octant >> (2 - a) & 1 == 1;
    let min = std::array::from_fn(|a| if upper(a) { mid[a] } else { bounds.min()[a] });
    let max = std::array::from_fn(|a| if upper(a) { bounds.max()[a] } else { mid[a] });
    SpatialBounds::new(min, max).ok()
}

/// What the probe of an octant looks at: the octant's geometric box, or the tight box of its
/// stored data (dataset frame, like the node bounds).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum ProbeBox {
    /// No tighter extent is known (no frame data extent): the octant's geometric box stands in
    /// for its data.
    Geometric,
    Data(SpatialBounds),
    /// The octant holds no data: nothing to draw, so the coarsest depth.
    Empty,
}

/// The coarsest buildable depth (`buildable` is inclusive) for which a cell of that depth, placed
/// at the point of `octant` nearest the eye and shifted to stay inside it, would not refine: its
/// children would project below `target_px` (ADR 0054, the target is a minimum size); else the
/// finest (source density, RQ-09/R3-Q9). An infinite or NaN edge never stops the refinement,
/// except for an octant with nothing in front of the eye (see `OctantProbe::classify`).
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
    octant_wanted_depth_in(
        view,
        grid,
        coordinate_origin,
        bounds,
        octant,
        ProbeBox::Geometric,
        buildable,
        target_px,
    )
}

/// `octant_wanted_depth` probing `probe` (see `ProbeBox`).
#[allow(clippy::too_many_arguments)]
pub(crate) fn octant_wanted_depth_in(
    view: &ViewProjection,
    grid: &CellGrid,
    coordinate_origin: [f64; 3],
    bounds: SpatialBounds,
    octant: usize,
    probe: ProbeBox,
    buildable: (u8, u8),
    target_px: f64,
) -> u8 {
    debug_assert!(buildable.0 <= buildable.1, "buildable range is ordered");
    match OctantProbe::classify(
        view,
        grid,
        coordinate_origin,
        bounds,
        octant,
        probe,
        target_px,
    ) {
        Probe::Finest => buildable.1,
        Probe::Coarsest | Probe::Behind => buildable.0,
        Probe::Meets(meets) => (buildable.0..=buildable.1)
            .find(|depth| meets.at(*depth))
            .unwrap_or(buildable.1),
    }
}

/// Octants of the node with no data in view (bit `o`: outside the view cone or behind the LOD
/// near distance, so also a box off-screen sideways, or empty),
/// and those with data in front (straddling or in front): a behind octant makes no demand of its
/// own, so the pool gives it the finest wish of the in-front ones (`behind_wanted`) instead of a
/// coarsest level that would need pages and coarsen the visible result.
pub(crate) fn behind_octants(
    view: &ViewProjection,
    grid: &CellGrid,
    coordinate_origin: [f64; 3],
    bounds: SpatialBounds,
    probes: [ProbeBox; 8],
) -> (u8, u8) {
    (0..8).fold((0, 0), |(behind, front), octant| {
        match OctantProbe::classify(
            view,
            grid,
            coordinate_origin,
            bounds,
            octant,
            probes[octant],
            1.0,
        ) {
            // Behind, or without data: nothing of it is drawn.
            Probe::Behind | Probe::Coarsest => (behind | 1 << octant, front),
            Probe::Finest | Probe::Meets(_) => (behind, front | 1 << octant),
        }
    })
}

/// The probes of the octants with data in view: a `behind` octant (see `behind_octants`) becomes
/// `Empty`, so `probe_region` and the data bound measure only what can be drawn. The `behind` set
/// (not "not in front of the eye plane") is the mask because an octant that crosses the plane is
/// visible: dropping it would collapse the eye node's far depth.
pub(crate) fn mask_behind(probes: [ProbeBox; 8], behind: u8) -> [ProbeBox; 8] {
    std::array::from_fn(|o| {
        if behind >> o & 1 == 1 {
            ProbeBox::Empty
        } else {
            probes[o]
        }
    })
}

/// The probes of a node masked at the given view, with the `(behind, front)` sets they came from:
/// the one place the mask is computed, so the far depth and the data bound always use the octants
/// in view at the pose they are evaluated at.
pub(crate) fn masked_probes(
    view: &ViewProjection,
    grid: &CellGrid,
    coordinate_origin: [f64; 3],
    bounds: SpatialBounds,
    probes: [ProbeBox; 8],
) -> ([ProbeBox; 8], (u8, u8)) {
    let sets = behind_octants(view, grid, coordinate_origin, bounds, probes);
    (mask_behind(probes, sets.0), sets)
}

/// A behind octant wants the finest wish among the in-front octants (none in front: its own).
pub(crate) fn behind_wanted(depths: [u8; 8], (behind, front): (u8, u8)) -> [u8; 8] {
    let Some(finest) = (0..8)
        .filter(|o| front >> o & 1 == 1)
        .map(|o| depths[o])
        .max()
    else {
        return depths;
    };
    std::array::from_fn(|o| {
        if behind >> o & 1 == 1 {
            finest
        } else {
            depths[o]
        }
    })
}

/// `octant_wanted_depth_in` searched outward from `hint` (the answer of a nearby view), assuming
/// that once a depth stops refining every finer one does too (its children are half as large at
/// about the same distance). It then costs 2 probes when the answer has not moved, instead of one
/// per coarser depth, and returns the same depth whenever that holds. The
/// `the_hinted_search_equals_the_full_search` test (cell_pool_tests.rs) found no difference in
/// 79 993 random poses. A `hint` outside the range falls back to the full search.
#[allow(clippy::too_many_arguments)]
pub(crate) fn octant_wanted_depth_near(
    view: &ViewProjection,
    grid: &CellGrid,
    coordinate_origin: [f64; 3],
    bounds: SpatialBounds,
    octant: usize,
    probe: ProbeBox,
    buildable: (u8, u8),
    target_px: f64,
    hint: u8,
) -> u8 {
    let (coarsest, finest) = buildable;
    if !(coarsest..=finest).contains(&hint) {
        return octant_wanted_depth_in(
            view,
            grid,
            coordinate_origin,
            bounds,
            octant,
            probe,
            buildable,
            target_px,
        );
    }
    match OctantProbe::classify(
        view,
        grid,
        coordinate_origin,
        bounds,
        octant,
        probe,
        target_px,
    ) {
        Probe::Finest => finest,
        Probe::Coarsest | Probe::Behind => coarsest,
        Probe::Meets(meets) => hinted_search(&meets, buildable, hint),
    }
}

/// The hinted search of `octant_wanted_depth_near` over an already built probe; `hint` lies in
/// `buildable`.
fn hinted_search(meets: &OctantProbe, (coarsest, finest): (u8, u8), hint: u8) -> u8 {
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

/// Cell mode: the octants of a node that lie wholly in front of the eye plane, as bit `o` set for
/// octant `o` (its data box, `probes`). The others (behind it or crossing it) want the finest depth
/// only because their cubes measure `+inf`, which says nothing about the node's range: they still
/// draw their own depth but do not set `hi`.
pub(crate) fn octants_in_front(
    view: &ViewProjection,
    coordinate_origin: [f64; 3],
    bounds: SpatialBounds,
    probes: [ProbeBox; 8],
) -> u8 {
    let Some(relative) = relative_bounds(bounds, coordinate_origin) else {
        return 0;
    };
    let eye = view_eye(view);
    let forward = view.camera().forward();
    // An octant counts when it lies wholly in front of the eye plane: one that crosses or lies
    // behind it holds cubes with a corner at or behind the eye, which measure `+inf` and want the
    // finest depth only for that reason.
    (0..8).fold(0_u8, |bits, octant| {
        let region = match probes[octant] {
            ProbeBox::Geometric => octant_box(relative, octant),
            ProbeBox::Data(data) => relative_bounds(data, coordinate_origin),
            ProbeBox::Empty => None,
        };
        let in_front = region.is_some_and(|b| {
            (0..3)
                .map(|a| {
                    let near = if forward[a] > 0.0 {
                        b.min()[a]
                    } else {
                        b.max()[a]
                    };
                    (near - eye[a]) * forward[a]
                })
                .sum::<f64>()
                > 0.0
        });
        bits | (u8::from(in_front) << octant)
    })
}

/// The region of a node whose depth range matters: the union box of its octants' probe boxes
/// (the node box for geometric probes), `None` when no octant holds data.
pub(crate) fn probe_region(bounds: SpatialBounds, probes: [ProbeBox; 8]) -> Option<SpatialBounds> {
    let mut region: Option<([f64; 3], [f64; 3])> = None;
    for probe in probes {
        let (min, max) = match probe {
            ProbeBox::Geometric => (bounds.min(), bounds.max()),
            ProbeBox::Data(data) => (data.min(), data.max()),
            ProbeBox::Empty => continue,
        };
        region = Some(region.map_or((min, max), |(lo, hi)| {
            (
                std::array::from_fn(|a| lo[a].min(min[a])),
                std::array::from_fn(|a| hi[a].max(max[a])),
            )
        }));
    }
    region.and_then(|(min, max)| SpatialBounds::new(min, max).ok())
}

/// Cell mode: the candidate depth range `(lo_c, hi_c)` of a node over its `region` (the node box
/// clipped to the data, dataset frame like `grid` and `coordinate_origin`), `buildable` being its
/// buildable span. `None` when the region is empty or wholly behind the eye or the LOD near depth.
///
/// - `lo_c`: the coarsest depth that does not refine for a cell ON THE VIEW AXIS at the view depth
///   of the farthest corner of the region. On the axis the projected cell does not depend on where
///   in the frame the region is, unlike a cell placed at the farthest corner, which perspective
///   stretches when that corner is far off screen.
/// - `hi_c`: the same at the nearest view depth the region can show (`nearest_visible_depth`, not
///   nearer than the LOD near depth), with the px target divided by the off-axis stretch
///   `sqrt(1 + tan^2)` at the corner of the field of view.
///
/// Both are limited to `buildable` and to `min_side` (a cell whose children are shorter does not
/// refine, as `continuity_depth` applies in the pool), with `lo_c <= hi_c`.
pub fn node_view_range(
    view: &ViewProjection,
    grid: &CellGrid,
    coordinate_origin: [f64; 3],
    region: Option<SpatialBounds>,
    buildable: (u8, u8),
    target_px: f64,
    min_side: Option<f64>,
) -> Option<(u8, u8)> {
    node_view_range_at(
        view,
        grid,
        coordinate_origin,
        region,
        buildable,
        target_px,
        min_side,
        None,
    )
}

/// `node_view_range` with `hi_c` evaluated at `max(nearest depth, occupied)` instead of at the
/// nearest depth alone, `occupied` being the view depth of the node's nearest occupied geometry
/// (`nearest_occupied_depth`, same units as the view depth of the region). A farther `occupied`
/// never gives a finer `hi`; `None` is `node_view_range`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn node_view_range_at(
    view: &ViewProjection,
    grid: &CellGrid,
    coordinate_origin: [f64; 3],
    region: Option<SpatialBounds>,
    buildable: (u8, u8),
    target_px: f64,
    min_side: Option<f64>,
    occupied: Option<f64>,
) -> Option<(u8, u8)> {
    let span = ViewSpan::new(view, grid, coordinate_origin, region, buildable, min_side)?;
    let lo = span.lo(target_px);
    Some((lo, span.hi(target_px, occupied).max(lo)))
}

/// `hi_c` of `node_view_range_at` alone, not raised to `lo`: one search instead of two.
#[allow(clippy::too_many_arguments)]
pub(crate) fn view_range_hi_at(
    view: &ViewProjection,
    grid: &CellGrid,
    coordinate_origin: [f64; 3],
    region: Option<SpatialBounds>,
    buildable: (u8, u8),
    target_px: f64,
    min_side: Option<f64>,
    occupied: Option<f64>,
) -> Option<u8> {
    ViewSpan::new(view, grid, coordinate_origin, region, buildable, min_side)
        .map(|span| span.hi(target_px, occupied))
}

/// `lo_c` of `node_view_range` alone: one search instead of two, for callers that ignore `hi_c`.
pub(crate) fn view_range_lo(
    view: &ViewProjection,
    grid: &CellGrid,
    coordinate_origin: [f64; 3],
    region: Option<SpatialBounds>,
    buildable: (u8, u8),
    target_px: f64,
    min_side: Option<f64>,
) -> Option<u8> {
    ViewSpan::new(view, grid, coordinate_origin, region, buildable, min_side)
        .map(|span| span.lo(target_px))
}

/// What `node_view_range` and `view_range_lo` share: the region's view depths and the depth cap.
struct ViewSpan<'a> {
    view: &'a ViewProjection,
    grid: &'a CellGrid,
    relative: SpatialBounds,
    eye: [f64; 3],
    buildable: (u8, u8),
    cap: u8,
    nearest: f64,
    farthest: f64,
    stretch: f64,
}

impl<'a> ViewSpan<'a> {
    fn new(
        view: &'a ViewProjection,
        grid: &'a CellGrid,
        coordinate_origin: [f64; 3],
        region: Option<SpatialBounds>,
        buildable: (u8, u8),
        min_side: Option<f64>,
    ) -> Option<Self> {
        debug_assert!(buildable.0 <= buildable.1, "buildable range is ordered");
        let relative = relative_bounds(region?, coordinate_origin)?;
        let eye = view_eye(view);
        let forward = view.camera().forward();
        let [width, height] = view.viewport();
        let aspect = f64::from(width) / f64::from(height.max(1));
        let tan_corner = (crate::camera::VERTICAL_FOV * 0.5).tan() * (1.0 + aspect * aspect).sqrt();
        let stretch = (1.0 + tan_corner * tan_corner).sqrt();
        let near = view.camera().lod_near_depth() / view.visual_scale();
        let nearest = near.max(nearest_visible_depth(view, &relative, eye));
        let farthest = farthest_corner_depth(&relative, eye, forward);
        if farthest <= nearest {
            return None;
        }
        let cap = min_side.map_or(buildable.1, |side| {
            representative_depth_for_spacing(grid, side).clamp(buildable.0, buildable.1)
        });
        Some(Self {
            view,
            grid,
            relative,
            eye,
            buildable,
            cap,
            nearest,
            farthest,
            stretch,
        })
    }

    fn on_axis(&self, depth: f64, target_px: f64) -> u8 {
        let meets = OctantProbe {
            view: self.view,
            grid: self.grid,
            octant_box: self.relative,
            anchor: self.eye,
            on_axis: Some(depth),
            target_px,
        };
        (self.buildable.0..=self.buildable.1)
            .find(|d| meets.at(*d))
            .unwrap_or(self.buildable.1)
            .min(self.cap)
    }

    fn lo(&self, target_px: f64) -> u8 {
        self.on_axis(self.farthest, target_px)
    }

    /// Not yet raised to `lo`.
    fn hi(&self, target_px: f64, occupied: Option<f64>) -> u8 {
        let nearest = occupied.map_or(self.nearest, |depth| self.nearest.max(depth));
        self.on_axis(nearest, target_px / self.stretch)
    }
}

/// Lower bound of the view depth of any point of `bounds` inside the view frustum: its
/// euclidean distance from `eye` times the cosine of the frustum's widest half angle (a point
/// at distance `r` and angle `a` off the axis lies at depth `r cos a`). A box that straddles the
/// plane through the eye still shows no data nearer than this, which the plane alone (depth 0)
/// does not tell.
fn nearest_visible_depth(view: &ViewProjection, bounds: &SpatialBounds, eye: [f64; 3]) -> f64 {
    let distance = (0..3)
        .map(|a| (eye[a].clamp(bounds.min()[a], bounds.max()[a]) - eye[a]).powi(2))
        .sum::<f64>()
        .sqrt();
    let [width, height] = view.viewport();
    let aspect = f64::from(width) / f64::from(height.max(1));
    let tan_corner = (crate::camera::VERTICAL_FOV * 0.5).tan() * (1.0 + aspect * aspect).sqrt();
    distance / (1.0 + tan_corner * tan_corner).sqrt()
}

/// View depth (along `forward`, from `eye`) of the corner of `bounds` farthest along `forward`.
fn farthest_corner_depth(bounds: &SpatialBounds, eye: [f64; 3], forward: [f64; 3]) -> f64 {
    (0..3)
        .map(|a| {
            let corner = if forward[a] > 0.0 {
                bounds.max()[a]
            } else {
                bounds.min()[a]
            };
            (corner - eye[a]) * forward[a]
        })
        .sum()
}

fn relative_bounds(bounds: SpatialBounds, origin: [f64; 3]) -> Option<SpatialBounds> {
    SpatialBounds::new(
        std::array::from_fn(|a| bounds.min()[a] - origin[a]),
        std::array::from_fn(|a| bounds.max()[a] - origin[a]),
    )
    .ok()
}

pub(crate) fn view_eye(view: &ViewProjection) -> [f64; 3] {
    view.camera()
        .position()
        .map(|value| value / view.visual_scale())
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

/// How an octant is probed.
enum Probe<'a> {
    /// No valid box: every depth fails and the finest is used.
    Finest,
    /// The octant has no data: it asks for the coarsest depth.
    Coarsest,
    /// No point of the octant's data is both in view (the cone `nearest_visible_depth` tests, so a
    /// box off-screen sideways counts) and past the LOD near distance: nothing of it is drawn, so it asks for the coarsest depth on its own (see `behind_octants`).
    Behind,
    Meets(OctantProbe<'a>),
}

/// The test of `octant_wanted_depth` for one octant: does a cell of a given depth, placed at the
/// point of the octant nearest the eye and shifted to stay inside it, project to at most the
/// target (see `at`)?
struct OctantProbe<'a> {
    view: &'a ViewProjection,
    grid: &'a CellGrid,
    octant_box: SpatialBounds,
    /// The point the cell is placed towards: the eye for an octant, the far corner for a node's
    /// far side.
    anchor: [f64; 3],
    /// View depth of the nearest face of the cell, which then sits on the view axis, when the
    /// octant's box straddles the LOD near distance (see `new`).
    on_axis: Option<f64>,
    target_px: f64,
}

impl<'a> OctantProbe<'a> {
    /// The box probed is the octant's data box (`ProbeBox::Data`) or, without one, the octant of
    /// `bounds`. A cell with a corner at or behind the eye projects to `+inf`, which never stops
    /// the refinement, so a box reaching the eye would always want the finest depth although its
    /// data may be far away or not in view at all. The view depths of the box decide:
    ///
    /// - every corner at depth <= `near` (the LOD near distance, `lod_near_depth`), or nearer than
    ///   anything of the box can be in view (`nearest_visible_depth`): nothing to see, the
    ///   coarsest depth;
    /// - the nearest corner at depth >= `near`: the cell at the point nearest the eye, as usual;
    /// - otherwise the box straddles `near`: the cell is placed on the view axis with its nearest
    ///   face at the nearest depth the box can show (`nearest_visible_depth`, at least `near`),
    ///   so the answer is at least as fine as the data in view needs.
    fn classify(
        view: &'a ViewProjection,
        grid: &'a CellGrid,
        coordinate_origin: [f64; 3],
        bounds: SpatialBounds,
        octant: usize,
        probe: ProbeBox,
        target_px: f64,
    ) -> Probe<'a> {
        let (boxed, measured) = match probe {
            ProbeBox::Empty => return Probe::Coarsest,
            ProbeBox::Data(data) => (relative_bounds(data, coordinate_origin), true),
            ProbeBox::Geometric => (
                relative_bounds(bounds, coordinate_origin)
                    .and_then(|relative| octant_box(relative, octant)),
                false,
            ),
        };
        let Some(octant_box) = boxed else {
            return Probe::Finest;
        };
        if !measured {
            // Without data the geometric box says nothing about what is in view: a box that
            // reaches the eye keeps wanting the finest depth.
            return Probe::Meets(Self::at_anchor(
                view,
                grid,
                octant_box,
                view_eye(view),
                target_px,
            ));
        }
        let eye = view_eye(view);
        let forward = view.camera().forward();
        let near = view.camera().lod_near_depth() / view.visual_scale();
        let depth_of = |toward: bool| -> f64 {
            (0..3)
                .map(|a| {
                    let corner = if (forward[a] > 0.0) == toward {
                        octant_box.max()[a]
                    } else {
                        octant_box.min()[a]
                    };
                    (corner - eye[a]) * forward[a]
                })
                .sum()
        };
        let (nearest, farthest) = (depth_of(false), depth_of(true));
        // Nothing of the box can be both in view and at least `near` away.
        let visible = near.max(nearest_visible_depth(view, &octant_box, eye));
        if farthest <= visible {
            return Probe::Behind;
        }
        // The point of the box nearest the eye may lie far off the view axis, at a depth no visible
        // point of the box has: the cell then goes on the axis at `visible`.
        let closest: f64 = (0..3)
            .map(|a| (eye[a].clamp(octant_box.min()[a], octant_box.max()[a]) - eye[a]) * forward[a])
            .sum();
        let on_axis = (nearest < near || closest < visible).then_some(visible);
        Probe::Meets(Self {
            view,
            grid,
            octant_box,
            anchor: eye,
            on_axis,
            target_px,
        })
    }

    fn at_anchor(
        view: &'a ViewProjection,
        grid: &'a CellGrid,
        octant_box: SpatialBounds,
        anchor: [f64; 3],
        target_px: f64,
    ) -> Self {
        Self {
            view,
            grid,
            octant_box,
            anchor,
            on_axis: None,
            target_px,
        }
    }

    /// Would a cell of `depth` at the probe point stop here (ADR 0054)? Its children would project
    /// below the target: the smallest of the 8 children is measured (off axis it is not always the
    /// farthest one). A child with a corner at or behind the eye measures `+inf`, so it never
    /// stops the refinement. The length minimum is applied by the pool (`continuity_depth`).
    fn at(&self, depth: u8) -> bool {
        let side = self.grid.root_side / 2.0_f64.powi(i32::from(depth));
        let min: [f64; 3] = match self.on_axis {
            Some(near) => {
                // The cell's nearest corner is at view depth `near`: its centre is half the
                // cell's extent along the view direction further.
                let forward = self.view.camera().forward();
                let extent: f64 = forward.iter().map(|v| v.abs()).sum::<f64>() * side;
                std::array::from_fn(|a| {
                    self.anchor[a] + forward[a] * (near + extent / 2.0) - side / 2.0
                })
            }
            None => std::array::from_fn(|a| {
                let low = self.octant_box.min()[a];
                let high = (self.octant_box.max()[a] - side).max(low);
                self.anchor[a]
                    .clamp(self.octant_box.min()[a], self.octant_box.max()[a])
                    .clamp(low, high)
            }),
        };
        let half = side / 2.0;
        let smallest = (0..8_usize)
            .map(|child| {
                let lo: [f64; 3] =
                    std::array::from_fn(|a| min[a] + half * ((child >> a & 1) as f64));
                SpatialBounds::new(lo, lo.map(|value| value + half)).map_or(f64::NAN, |c| {
                    self.view.projected_voxel_edge_pixels(c, [0.0; 3])
                })
            })
            .fold(
                f64::INFINITY,
                |acc, edge| if edge < acc { edge } else { acc },
            );
        smallest < self.target_px
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

/// `drawn`, `stage` and `step` per octant (see `CellCutEntry`).
pub(crate) type DrawnLevels = ([Option<u8>; 8], [bool; 8], [Option<u8>; 8]);

/// Frames an octant waits for the adjacent depth of each hop before it gives up on it (ADR
/// 0053): about 0.5 s at 60 fps. The ADR allows a longer total latency, but never a view stuck
/// behind a depth that does not arrive (refused, over the CPU budget or build cap, or evicted).
/// Each hop of a timed-out octant gets its own wait, so a stuck zone crosses `n` levels in at
/// most `n * STEP_WAIT_FRAMES` frames (the frame budgets of the simulations are sized on it).
pub const STEP_WAIT_FRAMES: u32 = 30;

/// The stepping rule (ADR 0053): `(draw, next)` for one octant that drew `prev` in the last
/// applied cut and now wants the stored depth `w`. With no `prev` it draws the resident level
/// closest to `w` (R3-Q21). A `prev` that is no longer resident (pool reset or eviction) draws
/// the resident level of the interval `prev..w` closest to `prev` (the closest to `w` when the
/// interval has none). Otherwise the octant moves at most one level towards `w` and never past
/// it: `next` is the adjacent servable depth in that direction (depths that are neither resident
/// nor buildable are skipped; `w` when none is servable), drawn once resident; until then `prev`
/// keeps drawing. `next` is `None` when `draw == w`, and a pending octant stays pending (the view
/// is never at rest before it reaches `w`). `buildable` is the depth range the node can serve.
/// `timed_out`: the adjacent depth did not arrive in `STEP_WAIT_FRAMES`, so the octant hops to the
/// resident level of the interval closest to `prev` (the smallest jump towards `w`); it stays
/// when there is none. The caller restarts the wait after every hop.
pub(crate) fn stepped_depth(
    prev: Option<u8>,
    w: u8,
    buildable: (u8, u8),
    resident: &[u8],
    timed_out: bool,
) -> Option<(u8, Option<u8>)> {
    let servable = |d: u8| resident.contains(&d) || (buildable.0..=buildable.1).contains(&d);
    // The adjacent servable depth from `from` towards `w` (`w` itself when none is).
    let adjacent = |from: u8| {
        if w > from {
            (from + 1..=w).find(|d| servable(*d))
        } else {
            (w..from).rev().find(|d| servable(*d))
        }
        .unwrap_or(w)
    };
    // The resident level of `prev..w` closest to `prev`, other than `prev` itself.
    let nearest_to_prev = |prev: u8| {
        let (lo, hi) = (prev.min(w), prev.max(w));
        resident
            .iter()
            .copied()
            .filter(|d| (lo..=hi).contains(d) && *d != prev)
            .min_by_key(|d| d.abs_diff(prev))
    };
    let Some(prev) = prev else {
        let (depth, _) = resident_depth(w, resident)?;
        return Some((depth, (depth != w).then(|| adjacent(depth))));
    };
    if !resident.contains(&prev) {
        let depth = match nearest_to_prev(prev) {
            Some(depth) => depth,
            None => resident_depth(w, resident)?.0,
        };
        return Some((depth, (depth != w).then(|| adjacent(depth))));
    }
    if w == prev {
        return Some((prev, None));
    }
    let next = adjacent(prev);
    let draw = if resident.contains(&next) {
        next
    } else if timed_out {
        nearest_to_prev(prev).unwrap_or(prev)
    } else {
        prev
    };
    // Pending while the octant has not reached `w`: the depth it takes after `draw`.
    Some((draw, (draw != w).then(|| adjacent(draw))))
}

/// `(drawn, stage, step)` of a node, `DrawnLevels`, from its `wanted` depths and `resident` levels, as `cell_cut`
/// decides them; `node_depth` is the depth whose levels span octants. A wanted depth that is not
/// `stored` is drawn from the level `served_depth` picks, and it is a stage while that level is
/// coarser than the wanted depth itself. `prev` is what each octant drew in the last applied cut:
/// an octant steps one level at a time towards its wanted depth (`stepped_depth`); `step` is the
/// depth it waits for to take the next step.
pub(crate) fn drawn_levels(
    wanted: &[Option<u8>; 8],
    stored: &[u8],
    buildable: (u8, u8),
    resident: &[u8],
    prev: &PrevState,
    node_depth: u8,
) -> DrawnLevels {
    let mut drawn = [None; 8];
    let mut stage = [false; 8];
    let mut step = [None; 8];
    // No level below the node or beyond its span can draw, and no depth is forced on the
    // others: the node draws the closest resident level of each octant.
    let stored = in_cell_range(stored, node_depth);
    let resident = in_cell_range(resident, node_depth);
    if stored.is_empty() {
        return (drawn, stage, step);
    }
    let (lo, hi) = cell_depth_range(node_depth);
    let buildable = (buildable.0.max(lo), buildable.1.min(hi));
    for o in 0..8 {
        if let Some(w) = wanted[o]
            && let Some((depth, next)) = stepped_depth(
                prev.drawn[o],
                served_depth(&stored, w),
                buildable,
                &resident,
                prev.timed_out[o],
            )
        {
            drawn[o] = Some(depth);
            stage[o] = depth < w;
            step[o] = next;
        }
    }
    (drawn, stage, step)
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
    cell_cut_with(cut, node, grid, &|_, octant, data| {
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
    wanted_depth: &dyn Fn(NodeId, usize, &CellCutNode) -> u8,
) -> Result<Vec<CellCutEntry>, CellCutError> {
    cell_cut_ranged(cut, node, grid, wanted_depth, None, None)
}

/// `(far, front)` of a node: `far` as `node_view_range` gives `lo`, `front` from `octants_in_front`.
pub(crate) type FarDepth<'a> = dyn Fn(NodeId, &CellCutNode) -> (u8, u8) + 'a;

/// What each octant of a node drew in the last applied cut (the stepping rule's `prev`).
pub(crate) type PrevDrawn<'a> = dyn Fn(NodeId) -> PrevState + 'a;

/// What an octant drew in the last applied cut and whether its step timed out.
#[derive(Clone, Copy, Default)]
pub(crate) struct PrevState {
    pub drawn: [Option<u8>; 8],
    pub timed_out: [bool; 8],
}

/// `cell_cut_with` that also fills `CellCutEntry::range` in cell mode from `far(node, data)` =
/// `(far, front)` as `FarDepth` describes (`None`: no range), and steps each octant from
/// `prev(node)` (`None`: every octant draws its closest resident level).
pub(crate) fn cell_cut_ranged(
    cut: &[NodeId],
    node: &dyn Fn(NodeId) -> Option<CellCutNode>,
    grid: &CellGrid,
    wanted_depth: &dyn Fn(NodeId, usize, &CellCutNode) -> u8,
    far: Option<&FarDepth<'_>>,
    prev: Option<&PrevDrawn<'_>>,
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
            step: [None; 8],
            range: None,
            node_anchor: None,
            node_target: None,
            node_run: None,
            node_stage: false,
            node_step: None,
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
        has_resident
            .push(!in_cell_range(&data.resident, rep.saturating_sub(MAX_MASK_LEVELS)).is_empty());
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
        let node_depth = placed.rep.saturating_sub(MAX_MASK_LEVELS);
        // A wanted depth is clamped to the usable range; nothing below the node is
        // wanted, and a node with no buildable depth inside the range keeps the view's depth
        // (nothing of it can draw, see `drawn_levels`).
        let cell_range = {
            let (lo, hi) = cell_depth_range(node_depth);
            let (lo, hi) = (lo.max(data.buildable.0), hi.min(data.buildable.1));
            (lo <= hi).then_some((lo, hi))
        };
        for o in 0..8 {
            if octant_fully_masked(&entry.mask, o) {
                continue;
            }
            let w = wanted_depth(entry.node, o, data);
            entry.wanted[o] = Some(cell_range.map_or(w, |(lo, hi)| w.clamp(lo, hi)));
        }
        if let (Some(far), Some((lo, hi))) = (far, cell_range) {
            let (far, front) = far(entry.node, data);
            // Octants behind or crossing the eye plane want the finest depth only because their
            // cube measures `+inf`: they must not widen the range (`front`).
            let near = (0..8)
                .filter(|o| front >> o & 1 == 1)
                .filter_map(|o| entry.wanted[o])
                .max();
            let far = far.clamp(lo, hi);
            // No octant in front (all cross the eye plane or lie behind it): `hi = lo`.
            let hi_wanted = near.map_or(far, |near| near.clamp(lo, hi));
            let drawn_any = entry.wanted.iter().any(Option::is_some);
            entry.range = drawn_any.then_some((far.min(hi_wanted), hi_wanted));
        }
        (entry.drawn, entry.stage, entry.step) = drawn_levels(
            &entry.wanted,
            &data.stored,
            data.buildable,
            &data.resident,
            &prev.map_or_else(PrevState::default, |prev| prev(entry.node)),
            node_depth,
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
