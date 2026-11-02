//! Camera-independent cells per LOD node on the dyadic grid anchored at the root.
//!
//! An internal node yields one level at its representative depth (8 cells per axis); a leaf yields
//! a chain of finer levels down to the density of its source points. Grid, node bounds and point
//! positions must share one coordinate frame (the caller subtracts the coordinate origin).

use std::fmt;

use viewer_core::{NodeId, PointBlock, SpatialBounds};

use crate::attributes::ScalarAttribute;

/// Deepest dyadic level a cell can have (24 bits per axis).
pub const MAX_CELL_DEPTH: u8 = 24;
/// Levels a leaf chain may add to its representative depth: 10 bits per axis for a root-sized node.
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

// Bounds are stored as f32: real sides deviate ~1e-5 relative from root_side / 2^k.
const DEPTH_SNAP_TOLERANCE: f64 = 1e-3;

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
    pub coord: [u32; 3],
    pub rgb_sum: [u64; 3],
    pub intensity_sum: f64,
    pub count: u32,
    /// Classification of the first point of the cell; 0 without the attribute.
    pub classification: u8,
    pub z_min: f32,
    pub z_max: f32,
}

/// Cells of one depth in Morton order.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeCellLevel {
    pub depth: u8,
    pub cells: Vec<NodeCell>,
}

/// One level for an internal node; a chain of increasing depth for a leaf.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeCells {
    pub node: NodeId,
    pub levels: Vec<NodeCellLevel>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NodeCellError {
    /// The root anchor or side is not finite and positive.
    InvalidGrid,
    /// The node has no extent, so no representative depth exists.
    DegenerateBounds,
}

impl fmt::Display for NodeCellError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidGrid => f.write_str("cell grid anchor or side is not finite and positive"),
            Self::DegenerateBounds => f.write_str("node bounds have no extent"),
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
        block: &PointBlock,
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
        block: &PointBlock,
    ) -> Result<NodeCells, NodeCellError> {
        if !(grid.root_side.is_finite()
            && grid.root_side > 0.0
            && grid.root_min.iter().all(|value| value.is_finite()))
        {
            return Err(NodeCellError::InvalidGrid);
        }
        if (0..3).all(|axis| bounds.max()[axis] <= bounds.min()[axis]) {
            return Err(NodeCellError::DegenerateBounds);
        }
        let first = match spacing {
            Some(spacing) => representative_depth_for_spacing(grid, spacing),
            None => representative_depth(grid, bounds),
        };
        let last = if is_leaf {
            first.saturating_add(CHAIN_SPAN).min(MAX_CELL_DEPTH)
        } else {
            first
        };
        let points = Points::new(block);
        let mut levels: Vec<NodeCellLevel> = Vec::new();
        for depth in first..=last {
            let cells = build_level(grid, bounds, &points, depth);
            // A level with as many cells as the previous one adds no detail.
            if levels
                .last()
                .is_some_and(|previous| previous.cells.len() == cells.len())
            {
                break;
            }
            levels.push(NodeCellLevel { depth, cells });
        }
        if is_leaf {
            let counts: Vec<usize> = levels.iter().map(|level| level.cells.len()).collect();
            let mut keep = chain_keep_mask(&counts, self.min_growth).into_iter();
            levels.retain(|_| keep.next().unwrap_or(true));
        }
        Ok(NodeCells { node, levels })
    }
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
fn node_range(grid: &CellGrid, bounds: SpatialBounds, depth: u8, axis: usize) -> (u32, u32) {
    let count = 1_u64 << depth;
    let cell_side = grid.root_side / count as f64;
    let edge = |value: f64| ((value - grid.root_min[axis]) / cell_side).round();
    let last = (count - 1) as f64;
    let lo = edge(bounds.min()[axis]).clamp(0.0, last);
    let hi = (edge(bounds.max()[axis]) - 1.0).clamp(lo, last);
    (lo as u32, hi as u32)
}

fn build_level(
    grid: &CellGrid,
    bounds: SpatialBounds,
    points: &Points<'_>,
    depth: u8,
) -> Vec<NodeCell> {
    let count = f64::from(1_u32 << depth);
    let range: [(u32, u32); 3] = std::array::from_fn(|axis| node_range(grid, bounds, depth, axis));
    let mut keyed: Vec<(u128, usize, [u32; 3])> = Vec::new();
    for (index, point) in points.block.points().points().iter().enumerate() {
        if point.position.iter().any(|value| !value.is_finite()) {
            continue;
        }
        // Same floor as the CPU grid, then clamped to the node: a point on the max face (or off
        // by rounding) lands in the node's last cell and every finite point counts once.
        let coord = std::array::from_fn(|axis| {
            let scaled = ((f64::from(point.position[axis]) - grid.root_min[axis]) / grid.root_side
                * count)
                .floor();
            (scaled.clamp(f64::from(range[axis].0), f64::from(range[axis].1))) as u32
        });
        keyed.push((morton(coord), index, coord));
    }
    // Stable by construction: the index breaks ties, so sums follow point order.
    keyed.sort_unstable_by_key(|(key, index, _)| (*key, *index));
    let mut cells: Vec<NodeCell> = Vec::new();
    let mut current = None;
    for (key, index, coord) in keyed {
        let point = &points.block.points().points()[index];
        if current != Some(key) {
            current = Some(key);
            cells.push(NodeCell {
                coord,
                rgb_sum: [0; 3],
                intensity_sum: 0.0,
                count: 0,
                classification: points
                    .classification
                    .and_then(|values| values.value_at(index))
                    .map_or(0, |value| value as u8),
                z_min: point.position[2],
                z_max: point.position[2],
            });
        }
        let cell = cells.last_mut().expect("a cell was just pushed");
        if let Some(value) = points.intensity.and_then(|values| values.value_at(index)) {
            cell.intensity_sum += f64::from(value);
        }
        cell.rgb_sum[0] += u64::from(point.color.r);
        cell.rgb_sum[1] += u64::from(point.color.g);
        cell.rgb_sum[2] += u64::from(point.color.b);
        cell.count += 1;
        cell.z_min = cell.z_min.min(point.position[2]);
        cell.z_max = cell.z_max.max(point.position[2]);
    }
    cells
}

/// Interleaves 24 bits per axis (x most significant) into a 72-bit Morton code.
fn morton(coord: [u32; 3]) -> u128 {
    let mut code = 0_u128;
    for bit in 0..u32::from(MAX_CELL_DEPTH) {
        for (axis, value) in coord.iter().enumerate() {
            code |= u128::from((value >> bit) & 1) << (3 * bit + (2 - axis as u32));
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
            feed(&cell.count.to_le_bytes());
            feed(&[cell.classification]);
            feed(&cell.z_min.to_bits().to_le_bytes());
            feed(&cell.z_max.to_bits().to_le_bytes());
        }
    }
    hash
}
