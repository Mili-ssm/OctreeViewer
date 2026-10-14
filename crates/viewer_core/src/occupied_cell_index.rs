//! Source-independent occupied-cell to original-row association.
use crate::{OccupiedOctree, OccupiedOctreeError, OccupiedOctreeRoot, VoxelCoord};

/// Groups every input row by its integer cell; duplicate cells retain distinct row IDs.
/// No payload, physical grid or coordinate system is inferred. IDs refer to input order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OccupiedCellIndex {
    topology: OccupiedOctree,
    leaf_offsets: Vec<usize>,
    row_ids: Vec<usize>,
}

impl OccupiedCellIndex {
    /// Sorts path/row pairs once, preserving ascending original row IDs within each leaf.
    /// Temporary storage is O(rows), released before topology construction from unique keys.
    /// Allocation failures follow the normal Rust policy.
    pub fn new(
        root: OccupiedOctreeRoot,
        cells: impl IntoIterator<Item = VoxelCoord>,
    ) -> Result<Self, OccupiedOctreeError> {
        let mut entries = cells
            .into_iter()
            .enumerate()
            .map(|(row, coord)| {
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
            topology,
            leaf_offsets,
            row_ids,
        })
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

    pub fn row_count(&self) -> usize {
        self.row_ids.len()
    }

    pub fn leaf_count(&self) -> usize {
        self.leaf_offsets.len() - 1
    }

    /// Retained inline data and vector capacities; excludes allocator overhead and build peak.
    pub fn estimated_memory_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            .saturating_add(
                self.topology.estimated_memory_bytes() - std::mem::size_of::<OccupiedOctree>(),
            )
            .saturating_add(
                self.leaf_offsets
                    .capacity()
                    .saturating_mul(std::mem::size_of::<usize>()),
            )
            .saturating_add(
                self.row_ids
                    .capacity()
                    .saturating_mul(std::mem::size_of::<usize>()),
            )
    }
}
