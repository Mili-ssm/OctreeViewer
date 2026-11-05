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
//! model; for now every octant carries the node-level value.

use std::{collections::HashMap, fmt, ops::Range};

use viewer_core::{NodeId, SpatialBounds};

use crate::{
    ViewProjection,
    node_cells::{CellGrid, NodeCellLevel, node_range, representative_depth},
};

/// Deepest ancestor-descendant gap (in node levels) a mask can express: 8 cells -> 1 cell.
const MAX_MASK_LEVELS: u8 = 3;

/// What `cell_cut` needs to know about a cut node.
#[derive(Debug, Clone, PartialEq)]
pub struct CellCutNode {
    /// Dataset frame, like the grid.
    pub bounds: SpatialBounds,
    /// Absolute depths of the stored levels, ascending.
    pub stored: Vec<u8>,
    /// Absolute depths of the resident levels.
    pub resident: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellCutEntry {
    pub node: NodeId,
    /// Stored depth the view asks for, per octant.
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

/// The first stored depth (ascending) whose projected error meets `target_px`, else the finest
/// (source density, RQ-09/R3-Q9). A pruned depth thus resolves to the next stored finer one, and
/// an infinite or NaN error (camera inside, corners behind) never meets the target.
///
/// `stored` must not be empty.
pub fn wanted_depth(
    view: &ViewProjection,
    grid: &CellGrid,
    coordinate_origin: [f64; 3],
    bounds: SpatialBounds,
    stored: &[u8],
    target_px: f64,
) -> u8 {
    debug_assert!(
        stored.windows(2).all(|pair| pair[0] < pair[1]),
        "stored depths ascend"
    );
    let finest = *stored.last().expect("stored levels are not empty");
    stored
        .iter()
        .copied()
        .find(|depth| {
            let spacing = grid.root_side / 2.0_f64.powi(i32::from(*depth));
            view.projected_error_pixels(bounds, spacing, coordinate_origin) <= target_px
        })
        .unwrap_or(finest)
}

/// `(depth, is_stage)`: the wanted level if resident, else the coarsest resident one. It is a stage
/// only when coarser than `wanted`; a finer level already meets the target (ADR 0031).
pub fn resident_depth(wanted: u8, resident: &[u8]) -> Option<(u8, bool)> {
    if resident.contains(&wanted) {
        Some((wanted, false))
    } else {
        resident
            .iter()
            .copied()
            .min()
            .map(|depth| (depth, depth < wanted))
    }
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
    let mut entries = Vec::with_capacity(cut.len());
    let mut placed = Vec::with_capacity(cut.len());
    let mut has_resident = Vec::with_capacity(cut.len());
    for id in cut {
        let data = node(*id).ok_or(CellCutError::UnknownNode(*id))?;
        if data.stored.is_empty() {
            return Err(CellCutError::NoStoredLevels(*id));
        }
        let wanted = wanted_depth(
            view,
            grid,
            coordinate_origin,
            data.bounds,
            &data.stored,
            target_px,
        );
        let chosen = resident_depth(wanted, &data.resident);
        entries.push(CellCutEntry {
            node: *id,
            wanted: [Some(wanted); 8],
            drawn: [chosen.map(|(depth, _)| depth); 8],
            stage: [chosen.is_some_and(|(_, stage)| stage); 8],
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
        has_resident.push(!data.resident.is_empty());
    }

    let mut by_cell: HashMap<(u8, [u64; 3]), usize> = HashMap::with_capacity(cut.len());
    for (index, node) in placed.iter().enumerate() {
        by_cell.entry((node.rep, node.lo)).or_insert(index);
    }
    for (child, d) in placed.iter().enumerate() {
        if !has_resident[child] {
            continue;
        }
        // Ancestors are nodes with a smaller representative depth (>= 3) whose aligned 8-cell
        // block contains this node's first cell.
        for k in 1..=d.rep.saturating_sub(3) {
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
        let uniform = entry.wanted.iter().all(|w| *w == entry.wanted[0])
            && entry.drawn.iter().all(|d| *d == entry.drawn[0])
            && entry.stage.iter().all(|s| *s == entry.stage[0]);
        // A uniform entry feeds the node-level bytes of the former layout; per-octant values
        // follow only when they differ. Step B must hash `wanted == None` distinctly from
        // `Some(0)` here; step A entries are always `Some`.
        debug_assert!(!uniform || entry.wanted[0].is_some());
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
