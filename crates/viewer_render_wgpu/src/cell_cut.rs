//! Pure per-node cell cut (R3 Task 3): which stored level each cut node draws, and which of its
//! 8^3 mask cells are hidden under a resident selected descendant.
//!
//! Grid, node bounds and the camera's dataset frame are one frame; `coordinate_origin` is only
//! subtracted by the view. Cut nodes must sit on the dyadic grid: each spans 8 cells per axis at
//! its representative depth, so a mask cell is one such cell.

use std::{collections::HashMap, fmt};

use viewer_core::{NodeId, SpatialBounds};

use crate::{
    ViewProjection,
    node_cells::{CellGrid, node_range, representative_depth},
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
    /// Stored depth the view asks for.
    pub wanted: u8,
    /// Resident depth to draw; `None` when no level is resident.
    pub drawn: Option<u8>,
    /// `drawn` is coarser than `wanted` (R3-Q3).
    pub stage: bool,
    /// Bit `x + 8y + 64z` of the node's 8^3 grid; 1 = hidden under a resident selected
    /// descendant.
    pub mask: [u64; 8],
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
            wanted,
            drawn: chosen.map(|(depth, _)| depth),
            stage: chosen.is_some_and(|(_, stage)| stage),
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
        feed(&[entry.wanted]);
        match entry.drawn {
            Some(depth) => feed(&[1, depth]),
            None => feed(&[0, 0]),
        }
        feed(&[u8::from(entry.stage)]);
        for word in entry.mask {
            feed(&word.to_le_bytes());
        }
    }
    hash
}

#[cfg(test)]
#[path = "cell_cut_tests.rs"]
mod tests;
