//! Experimental source-bound leaf-to-row mapping; no physical quantization or disk layout.
use thiserror::Error;

use crate::{
    AttributeColumn, DatasetError, OccupiedOctree, OccupiedOctreeError, OccupiedOctreeRoot,
    PointBlock, VoxelCoord,
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
    topology: OccupiedOctree,
    leaf_offsets: Vec<usize>,
    row_ids: Vec<usize>,
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
        let mut entries = cells
            .iter()
            .enumerate()
            .map(|(row, &coord)| {
                root.key(coord)
                    .map(|key| (key, row))
                    .ok_or(OccupiedOctreeError::OutsideRoot(coord))
            })
            .collect::<Result<Vec<_>, _>>()?;
        entries.sort_unstable();
        let mut keys = Vec::new();
        let mut leaf_offsets = Vec::new();
        // Allocate compact row storage explicitly: collecting a consuming tuple iterator
        // can reuse its wider allocation and retain excessive capacity for usize rows.
        let mut row_ids = Vec::with_capacity(entries.len());
        for (offset, &(key, row)) in entries.iter().enumerate() {
            row_ids.push(row);
            if keys.last() != Some(&key) {
                keys.push(key);
                leaf_offsets.push(offset);
            }
        }
        // Sentinel also represents the empty mapping as [0].
        leaf_offsets.push(entries.len());
        drop(entries);
        let topology = OccupiedOctree::from_sorted_unique_keys(root, &keys);
        Ok(Self {
            source,
            topology,
            leaf_offsets,
            row_ids,
        })
    }

    pub fn source(&self) -> &PointBlock {
        &self.source
    }

    pub fn into_source(self) -> PointBlock {
        self.source
    }

    pub fn topology(&self) -> &OccupiedOctree {
        &self.topology
    }

    /// Original source row indices for a leaf in final-level order; no payload copies.
    /// Returns `None` for an invalid leaf index, including `usize::MAX`.
    pub fn rows_for_leaf(&self, leaf: usize) -> Option<&[usize]> {
        let start = *self.leaf_offsets.get(leaf)?;
        let end = *self.leaf_offsets.get(leaf.checked_add(1)?)?;
        Some(&self.row_ids[start..end])
    }

    /// Original source row indices, ascending within the cell; `None` if unoccupied.
    pub fn rows_for_cell(&self, coord: VoxelCoord) -> Option<&[usize]> {
        self.rows_for_leaf(self.topology.leaf_index(coord)?)
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
