//! Camera-independent cells per LOD node on the dyadic grid anchored at the root.
//!
//! An internal node yields a pruned pyramid of levels from 8 to 256 cells per axis; a leaf yields
//! a chain of finer levels down to the density of its source points. Grid, node bounds and point
//! positions must share one coordinate frame (the caller subtracts the coordinate origin).

use std::{collections::HashSet, fmt, ops::Range};

use viewer_core::{NodeId, PointBlock, SpatialBounds};

use crate::attributes::ScalarAttribute;

/// Deepest absolute dyadic level representable by the grid coordinate type.
pub const MAX_CELL_DEPTH: u8 = 63;
/// Previous fixed leaf-chain span, retained for benchmark comparisons.
pub const CHAIN_SPAN: u8 = 7;
/// Minimum cell-count growth between two stored levels of a leaf chain. Stored counts then grow
/// geometrically towards the finest level, which holds at most one cell per point, so the chain
/// has at most `g / (g - 1)` times the cells of its finest level: 3x for 1.5.
pub const CHAIN_MIN_GROWTH: f64 = 1.5;

/// The dyadic grid; R4 changes its anchor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CellGrid {
    pub root_min: [f64; 3],
    pub root_side: f64,
}

// Node sides carry float noise from the frame shifts: on real data log2(root_side / side) lies
// ~1e-5 off an integer.
const DEPTH_SNAP_TOLERANCE: f64 = 1e-3;

/// How far a grid anchor at `scale` may lie from its exact value: the rounding of `world -
/// origin` (half an ulp of the larger operand) plus the reader's own rounding of the stored
/// corner. A node corner closer than this to a cell edge is on the edge, within what the grid
/// can express.
pub(crate) fn anchor_tolerance(scale: f64) -> f64 {
    4.0 * f64::EPSILON * scale
}

/// Depth at which a node's side spans 8 cells, derived from the node side so it holds for any
/// `--spatial-divisions` of the converter (2, 4 or 8).
pub fn representative_depth(grid: &CellGrid, bounds: SpatialBounds) -> u8 {
    representative_depth_for_spacing(grid, side_of(bounds) / 8.0)
}

/// Depth of the cells whose side equals `spacing`, relative to the stored root. `v7.4-pre` nodes
/// pass their index `spacing`, so a node of 256 representatives per axis lands 8 levels below its
/// own depth instead of 3.
pub fn representative_depth_for_spacing(grid: &CellGrid, spacing: f64) -> u8 {
    // Node sides are root_side / 2^k up to float noise (log2 ≈ 13.99999 on real data); snap
    // before flooring so such nodes keep their intended depth.
    let depth = (grid.root_side / spacing).log2();
    let snapped = if (depth - depth.round()).abs() < DEPTH_SNAP_TOLERANCE {
        depth.round()
    } else {
        depth.floor()
    };
    snapped.clamp(0.0, f64::from(MAX_CELL_DEPTH)) as u8
}

fn side_of(bounds: SpatialBounds) -> f64 {
    (0..3)
        .map(|axis| bounds.max()[axis] - bounds.min()[axis])
        .fold(0.0, f64::max)
}

/// Exact per-cell sums, accumulated in point order like the CPU voxel build.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NodeCell {
    pub coord: [u64; 3],
    pub rgb_sum: [u64; 3],
    pub intensity_sum: f64,
    pub intensity_count: u32,
    pub count: u32,
    /// Classification of the first point of the cell; -1 without the attribute.
    pub classification: i16,
    pub z_min: f32,
    pub z_max: f32,
}

/// Cells of one depth in Morton order.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeCellLevel {
    pub depth: u8,
    pub cells: Vec<NodeCell>,
    /// Start in `cells` of each octant, then `cells.len()`. Octant `o = 4x + 2y + z` (x, y, z in
    /// {0, 1} are the node's half on each axis, the Morton order), so the cells of one octant are
    /// contiguous. `None` for a level that spans octants (depth <= node depth, possible in
    /// leaves): only levels at depth >= node depth + 1 can be split by octant.
    pub octant_offsets: Option<[u32; 9]>,
}

impl NodeCellLevel {
    /// Cells of octant `octant` (0..8); the whole level when it spans octants.
    pub fn octant_range(&self, octant: usize) -> Range<usize> {
        match self.octant_offsets {
            Some(offsets) => offsets[octant] as usize..offsets[octant + 1] as usize,
            None => 0..self.cells.len(),
        }
    }
}

/// A pruned pyramid for an internal node; a chain of increasing depth for a leaf.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeCells {
    pub node: NodeId,
    pub levels: Vec<NodeCellLevel>,
    /// Inclusive first and last depth `node_cell_level` can build for this node.
    pub buildable: (u8, u8),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NodeCellError {
    /// The root anchor or side is not finite and positive.
    InvalidGrid,
    /// The node has no extent, so no representative depth exists.
    DegenerateBounds,
    /// A leaf still has distinct finite positions sharing a cell at depth 63, or a requested
    /// depth exceeds it (ADR 0032: an error, never a silent merge).
    AbsoluteDepthExceeded,
}

impl fmt::Display for NodeCellError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidGrid => f.write_str("cell grid anchor or side is not finite and positive"),
            Self::DegenerateBounds => f.write_str("node bounds have no extent"),
            Self::AbsoluteDepthExceeded => f.write_str("absolute cell depth exceeds 63"),
        }
    }
}

impl std::error::Error for NodeCellError {}

pub trait NodeCellSource {
    fn node_cells(
        &self,
        grid: &CellGrid,
        node: NodeId,
        bounds: SpatialBounds,
        spacing: Option<f64>,
        is_leaf: bool,
        blocks: &[PointBlock],
    ) -> Result<NodeCells, NodeCellError>;
}

/// Cells computed directly from the node's decoded points.
pub struct PointNodeCellSource {
    /// Minimum cell-count growth between stored leaf levels (see `chain_keep_mask`); `1.0` keeps
    /// every level (benchmark baseline).
    pub min_growth: f64,
}

impl Default for PointNodeCellSource {
    fn default() -> Self {
        Self {
            min_growth: CHAIN_MIN_GROWTH,
        }
    }
}

impl NodeCellSource for PointNodeCellSource {
    fn node_cells(
        &self,
        grid: &CellGrid,
        node: NodeId,
        bounds: SpatialBounds,
        spacing: Option<f64>,
        is_leaf: bool,
        blocks: &[PointBlock],
    ) -> Result<NodeCells, NodeCellError> {
        let setup = NodeSetup::new(grid, bounds, spacing, is_leaf)?;
        let (first, last) = setup.buildable;
        let node_depth = setup.node_depth;
        let points: Vec<_> = blocks.iter().map(Points::new).collect();
        let distinct = distinct_position_count(blocks, is_leaf);
        let mut levels = Vec::new();
        for depth in first..=last {
            let (level, fully_separated) =
                build_checked_level(grid, bounds, &points, node_depth, depth, is_leaf, distinct)?;
            levels.push(level);
            if fully_separated {
                break;
            }
        }
        let counts: Vec<usize> = levels.iter().map(|level| level.cells.len()).collect();
        let mut keep = chain_keep_mask(&counts, self.min_growth).into_iter();
        levels.retain(|_| keep.next().unwrap_or(true));
        Ok(NodeCells {
            node,
            levels,
            buildable: setup.buildable,
        })
    }
}

/// Builds the cells of one `depth` on their own, equal to the level the unpruned chain of
/// [`PointNodeCellSource::node_cells`] holds at that depth. Depths outside `NodeCells::buildable`
/// fail with `AbsoluteDepthExceeded`.
pub fn node_cell_level(
    grid: &CellGrid,
    _node: NodeId,
    bounds: SpatialBounds,
    spacing: Option<f64>,
    is_leaf: bool,
    blocks: &[PointBlock],
    depth: u8,
) -> Result<NodeCellLevel, NodeCellError> {
    let setup = NodeSetup::new(grid, bounds, spacing, is_leaf)?;
    if depth < setup.buildable.0 || depth > setup.buildable.1 {
        return Err(NodeCellError::AbsoluteDepthExceeded);
    }
    let points: Vec<_> = blocks.iter().map(Points::new).collect();
    let distinct = distinct_position_count(blocks, is_leaf);
    build_checked_level(
        grid,
        bounds,
        &points,
        setup.node_depth,
        depth,
        is_leaf,
        distinct,
    )
    .map(|(level, _)| level)
}

/// Distinct finite positions of a leaf (0 for an internal node, which never separates fully).
fn distinct_position_count(blocks: &[PointBlock], is_leaf: bool) -> usize {
    blocks
        .iter()
        .flat_map(|block| block.points().points())
        .filter(|point| is_leaf && point.position.iter().all(|value| value.is_finite()))
        .map(|point| {
            point
                .position
                .map(|value| if value == 0.0 { 0 } else { value.to_bits() })
        })
        .collect::<HashSet<_>>()
        .len()
}

/// One level plus whether a leaf holds one cell per distinct position there. A leaf still merged
/// at `MAX_CELL_DEPTH` is an error (ADR 0032), for the chain and the single-level build alike.
fn build_checked_level(
    grid: &CellGrid,
    bounds: SpatialBounds,
    points: &[Points<'_>],
    node_depth: u8,
    depth: u8,
    is_leaf: bool,
    distinct: usize,
) -> Result<(NodeCellLevel, bool), NodeCellError> {
    let cells = build_level(grid, bounds, points, depth)?;
    let fully_separated = is_leaf && cells.len() == distinct;
    if is_leaf && depth == MAX_CELL_DEPTH && !fully_separated {
        return Err(NodeCellError::AbsoluteDepthExceeded);
    }
    let octant_offsets = octant_offsets(grid, bounds, node_depth, depth, &cells);
    let level = NodeCellLevel {
        depth,
        cells,
        octant_offsets,
    };
    Ok((level, fully_separated))
}

/// Validated depth range of a node, shared by the chain and the single-level build.
struct NodeSetup {
    /// Inclusive first and last depth that can be built.
    buildable: (u8, u8),
    node_depth: u8,
}

impl NodeSetup {
    fn new(
        grid: &CellGrid,
        bounds: SpatialBounds,
        spacing: Option<f64>,
        is_leaf: bool,
    ) -> Result<Self, NodeCellError> {
        if !(grid.root_side.is_finite()
            && grid.root_side > 0.0
            && grid.root_min.iter().all(|value| value.is_finite()))
        {
            return Err(NodeCellError::InvalidGrid);
        }
        if (0..3).all(|axis| bounds.max()[axis] <= bounds.min()[axis]) {
            return Err(NodeCellError::DegenerateBounds);
        }
        let requested_spacing = if is_leaf {
            spacing.unwrap_or(side_of(bounds) / 8.0)
        } else {
            side_of(bounds) / 8.0
        };
        if !requested_spacing.is_finite()
            || requested_spacing <= 0.0
            || (grid.root_side / requested_spacing).log2() > f64::from(MAX_CELL_DEPTH)
        {
            return Err(NodeCellError::AbsoluteDepthExceeded);
        }
        let first = if is_leaf {
            spacing.map_or_else(
                || representative_depth(grid, bounds),
                |spacing| representative_depth_for_spacing(grid, spacing),
            )
        } else {
            representative_depth(grid, bounds)
        };
        let last = if is_leaf {
            MAX_CELL_DEPTH
        } else {
            first.saturating_add(5)
        };
        if last > MAX_CELL_DEPTH {
            return Err(NodeCellError::AbsoluteDepthExceeded);
        }
        Ok(Self {
            buildable: (first, last),
            node_depth: representative_depth(grid, bounds).saturating_sub(3),
        })
    }
}

/// Octant starts of a level (see `NodeCellLevel::octant_offsets`). The octant of a cell is read
/// from the bit of its coordinate relative to the node, the same relative coordinate that orders
/// the Morton keys, so octants are contiguous.
fn octant_offsets(
    grid: &CellGrid,
    bounds: SpatialBounds,
    node_depth: u8,
    depth: u8,
    cells: &[NodeCell],
) -> Option<[u32; 9]> {
    if depth <= node_depth {
        return None;
    }
    let shift = u32::from(depth - node_depth - 1);
    let lo: [u64; 3] = std::array::from_fn(|axis| node_range(grid, bounds, depth, axis).0);
    let mut counts = [0_u32; 8];
    let mut previous = 0;
    for cell in cells {
        let octant = (0..3).fold(0_usize, |acc, axis| {
            let half = ((cell.coord[axis] - lo[axis]) >> shift).min(1) as usize;
            acc * 2 + half
        });
        debug_assert!(octant >= previous, "octants are contiguous in Morton order");
        previous = octant;
        counts[octant] += 1;
    }
    let mut offsets = [0_u32; 9];
    for octant in 0..8 {
        offsets[octant + 1] = offsets[octant] + counts[octant];
    }
    Some(offsets)
}

/// Which levels of a chain (cell counts, coarse to fine) to store. The finest level is always
/// kept; a coarser level is kept only if it has at most `1 / growth` of the cells of the next
/// kept level. A dropped level adds no detail over the kept finer one (similar cell count), and
/// a lookup that falls back to the next finer stored level never draws cells larger than the
/// target (RQ-09).
pub fn chain_keep_mask(counts: &[usize], growth: f64) -> Vec<bool> {
    let mut keep = vec![false; counts.len()];
    let mut finer: Option<usize> = None;
    for (index, count) in counts.iter().enumerate().rev() {
        if finer.is_none_or(|cells| (*count as f64) * growth <= cells as f64) {
            keep[index] = true;
            finer = Some(*count);
        }
    }
    keep
}

struct Points<'a> {
    block: &'a PointBlock,
    intensity: Option<ScalarAttribute<'a>>,
    classification: Option<ScalarAttribute<'a>>,
}

impl<'a> Points<'a> {
    fn new(block: &'a PointBlock) -> Self {
        Self {
            block,
            intensity: ScalarAttribute::find(block, "intensity"),
            classification: ScalarAttribute::find(block, "classification"),
        }
    }
}

/// Inclusive cell range of the node on one axis at `depth`.
pub(crate) fn node_range(
    grid: &CellGrid,
    bounds: SpatialBounds,
    depth: u8,
    axis: usize,
) -> (u64, u64) {
    let count = 1_u64 << depth;
    let cell_side = grid.root_side / count as f64;
    let anchor = axis_anchor(grid, bounds, depth, axis);
    let edge = |value: f64| ((value - anchor.position) / cell_side).round();
    let lo = offset_cell(anchor.cell, edge(bounds.min()[axis]), 0, count - 1);
    let hi = offset_cell(anchor.cell, edge(bounds.max()[axis]) - 1.0, lo, count - 1);
    (lo, hi)
}

/// A position on one axis and its exact cell index at some depth; indices are measured from it.
#[derive(Debug, Clone, Copy)]
struct AxisAnchor {
    position: f64,
    cell: u64,
}

/// Indices measured from the root corner in `f64` resolve only `root_side * 2^-53` units: a few
/// centimetres when a global cloud keeps the whole 2^64-quanta lattice as its root (points on
/// both sides of a sign bit, issue #30), so close points never separate. Deeper than the node,
/// indices are measured from the node's minimum corner instead when that corner is a cell edge at
/// the node depth up to [`anchor_tolerance`] (always for global formats, whose nodes lie on the
/// dyadic grid): its index is the rounded edge there, exact while the node side exceeds twice
/// that tolerance, and shifts left below it. Corners off the grid keep the root anchor.
fn axis_anchor(grid: &CellGrid, bounds: SpatialBounds, depth: u8, axis: usize) -> AxisAnchor {
    let root = AxisAnchor {
        position: grid.root_min[axis],
        cell: 0,
    };
    let node_depth = representative_depth(grid, bounds).saturating_sub(3);
    if depth <= node_depth {
        return root;
    }
    let node_cell_side = grid.root_side / (1_u64 << node_depth) as f64;
    let edge = (bounds.min()[axis] - grid.root_min[axis]) / node_cell_side;
    let scale = grid.root_min[axis].abs().max(bounds.min()[axis].abs());
    if (edge - edge.round()).abs() * node_cell_side > anchor_tolerance(scale) {
        return root;
    }
    let (corner, _) = node_range(grid, bounds, node_depth, axis);
    AxisAnchor {
        position: bounds.min()[axis],
        cell: corner << (depth - node_depth),
    }
}

/// `anchor + offset` clamped to `[lo, hi]` in the integer domain; `offset` is a whole `f64`. From
/// depth 54 on, `hi as f64` rounds up past the grid (2^63 - 1 becomes 2^63), so an `f64` clamp
/// would emit a cell outside it. Every operand fits `i64` (cells are below 2^63).
fn offset_cell(anchor: u64, offset: f64, lo: u64, hi: u64) -> u64 {
    (anchor as i64)
        .saturating_add(offset as i64)
        .clamp(lo as i64, hi as i64) as u64
}

#[cfg(test)]
type KeyedPoint = (u64, usize, usize, [u64; 3]);

fn build_level(
    grid: &CellGrid,
    bounds: SpatialBounds,
    points: &[Points<'_>],
    depth: u8,
) -> Result<Vec<NodeCell>, NodeCellError> {
    let count = (1_u64 << depth) as f64;
    let range: [(u64, u64); 3] = std::array::from_fn(|axis| node_range(grid, bounds, depth, axis));
    let anchors = std::array::from_fn(|axis| axis_anchor(grid, bounds, depth, axis));
    let single_region = range.iter().all(|(lo, hi)| hi - lo < (1 << 21));
    if single_region {
        build_level_keyed(grid, points, count, range, anchors, morton, Some)
    } else {
        build_level_keyed(grid, points, count, range, anchors, region_morton, |_| None)
    }
}

fn build_level_keyed<K: Ord + Copy>(
    grid: &CellGrid,
    points: &[Points<'_>],
    count: f64,
    range: [(u64, u64); 3],
    anchors: [AxisAnchor; 3],
    key_for: impl Fn([u64; 3]) -> K,
    as_u64: impl Fn(K) -> Option<u64>,
) -> Result<Vec<NodeCell>, NodeCellError> {
    let mut keyed: Vec<(K, usize, usize, [u64; 3])> = Vec::new();
    for (block_index, block) in points.iter().enumerate() {
        for (index, point) in block.block.points().points().iter().enumerate() {
            if point.position.iter().any(|value| !value.is_finite()) {
                continue;
            }
            let coord = std::array::from_fn(|axis| {
                let scaled = ((f64::from(point.position[axis]) - anchors[axis].position)
                    / grid.root_side
                    * count)
                    .floor();
                offset_cell(anchors[axis].cell, scaled, range[axis].0, range[axis].1)
            });
            let relative = std::array::from_fn(|axis| coord[axis] - range[axis].0);
            let key = key_for(relative);
            keyed.push((key, block_index, index, coord));
        }
    }
    // Entries are pushed in (block, index) order, so a stable sort by key alone equals sorting by
    // (key, block, index). pdqsort is ~8x slower on shuffled keys than on sorted ones; the radix
    // sort keeps the build time independent of the point order.
    // `order` lists the positions of `keyed` in sorted order; `None` means `keyed` is already
    // sorted. Iterating positions avoids copying the 48-byte entries into a second vector.
    let mut order: Option<Vec<(u64, u32)>> = None;
    let mut pairs = Vec::new();
    let all_u64 = keyed.len() < u32::MAX as usize
        && pairs.try_reserve_exact(keyed.len()).is_ok()
        && keyed.iter().enumerate().all(|(position, entry)| {
            as_u64(entry.0).is_some_and(|key| {
                pairs.push((key, position as u32));
                true
            })
        });
    if all_u64 {
        if pairs.windows(2).any(|pair| pair[0].0 > pair[1].0) {
            order = Some(radix_sort_stable(pairs));
        }
    } else {
        keyed.sort_unstable_by_key(|(key, block, index, _)| (*key, *block, *index));
    }
    let positions: Box<dyn Iterator<Item = usize> + '_> = match &order {
        Some(order) => Box::new(order.iter().map(|(_, position)| *position as usize)),
        None => Box::new(0..keyed.len()),
    };
    let mut cells: Vec<NodeCell> = Vec::new();
    let mut current = None;
    for position in positions {
        let (key, block_index, index, coord) = keyed[position];
        let points = &points[block_index];
        let point = &points.block.points().points()[index];
        if current != Some(key) {
            current = Some(key);
            cells.push(NodeCell {
                coord,
                rgb_sum: [0; 3],
                intensity_sum: 0.0,
                intensity_count: 0,
                count: 0,
                classification: points
                    .classification
                    .and_then(|values| values.value_at(index))
                    .map_or(-1, |value| value as i16),
                z_min: point.position[2],
                z_max: point.position[2],
            });
        }
        let cell = cells.last_mut().expect("a cell was just pushed");
        if let Some(value) = points.intensity.and_then(|values| values.value_at(index)) {
            cell.intensity_sum += f64::from(value);
            cell.intensity_count += 1;
        }
        cell.rgb_sum[0] += u64::from(point.color.r);
        cell.rgb_sum[1] += u64::from(point.color.g);
        cell.rgb_sum[2] += u64::from(point.color.b);
        cell.count += 1;
        cell.z_min = cell.z_min.min(point.position[2]);
        cell.z_max = cell.z_max.max(point.position[2]);
    }
    Ok(cells)
}

/// LSD radix sort on the key; stable, so equal keys keep their input order.
fn radix_sort_stable(mut pairs: Vec<(u64, u32)>) -> Vec<(u64, u32)> {
    const BITS: u32 = 11;
    let top = pairs.iter().fold(0_u64, |acc, (key, _)| acc | key);
    let mut scratch = vec![(0_u64, 0_u32); pairs.len()];
    let mut shift = 0;
    while shift < 64 && (top >> shift) != 0 {
        let mut counts = [0_usize; 1 << BITS];
        for (key, _) in &pairs {
            counts[((key >> shift) & ((1 << BITS) - 1)) as usize] += 1;
        }
        let mut start = 0;
        for count in &mut counts {
            let n = *count;
            *count = start;
            start += n;
        }
        for pair in &pairs {
            let digit = ((pair.0 >> shift) & ((1 << BITS) - 1)) as usize;
            scratch[counts[digit]] = *pair;
            counts[digit] += 1;
        }
        std::mem::swap(&mut pairs, &mut scratch);
        shift += BITS;
    }
    pairs
}

fn region_morton(coord: [u64; 3]) -> (u128, u64) {
    let mut high = 0_u128;
    for bit in 0..42 {
        for (axis, value) in coord.iter().enumerate() {
            high |= u128::from((value >> (bit + 21)) & 1) << (3 * bit + (2 - axis));
        }
    }
    (high, morton(coord))
}

fn morton(coord: [u64; 3]) -> u64 {
    let mut code = 0_u64;
    for bit in 0..21 {
        for (axis, value) in coord.iter().enumerate() {
            code |= ((value >> bit) & 1) << (3 * bit + (2 - axis));
        }
    }
    code
}

/// FNV-1a 64-bit digest of the node id, the level count and every level and cell in order.
pub fn node_cells_digest(cells: &NodeCells) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    let mut feed = |bytes: &[u8]| {
        for byte in bytes {
            hash = (hash ^ u64::from(*byte)).wrapping_mul(PRIME);
        }
    };
    feed(&cells.node.get().to_le_bytes());
    feed(&(cells.levels.len() as u64).to_le_bytes());
    for level in &cells.levels {
        feed(&[level.depth]);
        feed(&(level.cells.len() as u64).to_le_bytes());
        for cell in &level.cells {
            for value in cell.coord {
                feed(&value.to_le_bytes());
            }
            for value in cell.rgb_sum {
                feed(&value.to_le_bytes());
            }
            feed(&cell.intensity_sum.to_bits().to_le_bytes());
            feed(&cell.intensity_count.to_le_bytes());
            feed(&cell.count.to_le_bytes());
            feed(&cell.classification.to_le_bytes());
            feed(&cell.z_min.to_bits().to_le_bytes());
            feed(&cell.z_max.to_bits().to_le_bytes());
        }
    }
    hash
}

#[cfg(test)]
mod layout_tests {
    use super::*;

    #[test]
    fn deepest_level_keeps_the_root_max_face_inside_the_grid() {
        // 2^63 - 1 rounds up to 2^63 as f64; clamping in f64 would yield a cell outside the grid.
        let grid = CellGrid {
            root_min: [0.0; 3],
            root_side: 1.0,
        };
        let bounds = SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap();
        let last = (1_u64 << MAX_CELL_DEPTH) - 1;
        assert_eq!(node_range(&grid, bounds, MAX_CELL_DEPTH, 0), (0, last));
    }

    #[test]
    fn single_region_record_fits_six_machine_words() {
        assert!(std::mem::size_of::<KeyedPoint>() <= 6 * std::mem::size_of::<u64>());
    }

    fn lcg_keys(seed: u64, len: usize, modulus: u64, shift: u32) -> Vec<u64> {
        let mut state = seed;
        (0..len)
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                ((state >> 20) % modulus) << shift
            })
            .collect()
    }

    fn assert_matches_stable_sort(keys: &[u64]) {
        let pairs: Vec<(u64, u32)> = keys
            .iter()
            .enumerate()
            .map(|(position, key)| (*key, position as u32))
            .collect();
        let mut expected = pairs.clone();
        expected.sort_by_key(|(key, _)| *key);
        assert_eq!(radix_sort_stable(pairs), expected);
    }

    #[test]
    fn radix_sort_equals_stable_sort_on_random_and_edge_keys() {
        assert_matches_stable_sort(&[]);
        assert_matches_stable_sort(&[7]);
        assert_matches_stable_sort(&[5; 100]);
        assert_matches_stable_sort(&[0; 100]);
        assert_matches_stable_sort(&lcg_keys(1, 5000, 50, 0));
        assert_matches_stable_sort(&lcg_keys(2, 5000, 1 << 40, 0));
        assert_matches_stable_sort(&lcg_keys(3, 5000, 1 << 10, 53));
        assert_matches_stable_sort(&[u64::MAX, 0, u64::MAX, 1 << 63, 1, 1 << 63]);
    }
}
