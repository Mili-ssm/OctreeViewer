//! Experimental source-bound leaf-to-row mapping; no physical quantization or disk layout.
use thiserror::Error;

use crate::{
    AttributeColumn, DatasetError, OccupiedCellIndex, OccupiedOctree, OccupiedOctreeError,
    OccupiedOctreeRoot, PointBlock, VoxelCoord,
};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum OccupiedPointBlockError {
    #[error("expected one cell for each of {points} source rows, got {cells}")]
    CellCountMismatch { points: usize, cells: usize },
    #[error(transparent)]
    Topology(#[from] OccupiedOctreeError),
    #[error(transparent)]
    Dataset(#[from] DatasetError),
}

/// Associates occupied leaves with every source row, without averaging or removing points.
///
/// The source block is owned unchanged and exposed immutably, keeping row references valid.
/// Each input cell describes the corresponding source row; callers define the physical grid.
/// This type does not check whether a point position lies in that abstract integer cell.
#[derive(Debug, Clone, PartialEq)]
pub struct OccupiedPointBlock {
    source: PointBlock,
    index: OccupiedCellIndex,
}

impl OccupiedPointBlock {
    /// Validates payload alignment and cells before building a deterministic leaf-to-row map.
    ///
    /// Path/row pairs are sorted once. Rows within each leaf retain original source order.
    /// Temporary storage is O(rows); topology construction adds O(leaves * depth) work.
    /// Payloads are moved, not cloned. Allocation failures follow the normal Rust policy.
    pub fn new(
        root: OccupiedOctreeRoot,
        source: PointBlock,
        cells: &[VoxelCoord],
    ) -> Result<Self, OccupiedPointBlockError> {
        let points = source.points().len();
        if cells.len() != points {
            return Err(OccupiedPointBlockError::CellCountMismatch {
                points,
                cells: cells.len(),
            });
        }
        for column in source.attributes() {
            column.validate_row_count(points)?;
        }
        let index = OccupiedCellIndex::new(root, cells.iter().copied())?;
        Ok(Self { source, index })
    }

    pub fn source(&self) -> &PointBlock {
        &self.source
    }

    pub fn into_source(self) -> PointBlock {
        self.source
    }

    pub fn topology(&self) -> &OccupiedOctree {
        self.index.topology()
    }

    /// Original source row indices for a leaf in final-level order; no payload copies.
    pub fn rows_for_leaf(&self, leaf: usize) -> Option<&[usize]> {
        self.index.rows_for_leaf(leaf)
    }

    /// Original source row indices, ascending within the cell; `None` if unoccupied.
    pub fn rows_for_cell(&self, coord: VoxelCoord) -> Option<&[usize]> {
        self.index.rows_for_cell(coord)
    }

    /// Copies one cell's points, RGB and typed attributes in original source order.
    ///
    /// The returned block is a subset with the source ID as provenance; it is not a new
    /// hierarchy block and must not replace the full source in a dataset cache.
    /// Missing/out-of-root cells return `Ok(None)`; attribute selection errors propagate.
    pub fn select_cell(&self, coord: VoxelCoord) -> Result<Option<PointBlock>, DatasetError> {
        let Some(rows) = self.rows_for_cell(coord) else {
            return Ok(None);
        };
        let points = rows
            .iter()
            .map(|&row| self.source.points().points()[row])
            .collect::<Vec<_>>();
        let attributes = self
            .source
            .attributes()
            .iter()
            .map(|column| {
                let descriptor = column.descriptor();
                AttributeColumn::new(
                    descriptor.clone(),
                    column
                        .values()
                        .select_rows(rows, descriptor.component_count() as usize)?,
                    rows.len(),
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Some(PointBlock::new(
            self.source.id(),
            points.into(),
            attributes,
        )))
    }
}
