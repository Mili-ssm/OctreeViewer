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
    /// Sorts spatial keys once, preserving input row order within each leaf.
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
        // Input rows are enumerated in ascending order. A stable LSD radix sort by key alone
        // therefore has the same `(key, row)` order as tuple sorting, while avoiding the
        // comparison cost for the many large blocks produced by the format writer.
        radix_sort_by_key(
            &mut entries,
            usize::from(root.depth()).saturating_mul(3).div_ceil(8),
        );
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

fn radix_sort_by_key(entries: &mut Vec<(u128, usize)>, key_bytes: usize) {
    if entries.len() < 2 || key_bytes == 0 {
        return;
    }
    let mut scratch = Vec::with_capacity(entries.len());
    for byte in 0..key_bytes {
        let shift = byte * 8;
        let mut counts = [0_usize; 256];
        for &(key, _) in entries.iter() {
            counts[((key >> shift) & 0xff) as usize] += 1;
        }
        let mut next = [0_usize; 256];
        let mut total = 0;
        for (offset, count) in next.iter_mut().zip(counts) {
            *offset = total;
            total += count;
        }
        scratch.clear();
        scratch.resize(entries.len(), (0, 0));
        for &(key, row) in entries.iter() {
            let bucket = ((key >> shift) & 0xff) as usize;
            scratch[next[bucket]] = (key, row);
            next[bucket] += 1;
        }
        std::mem::swap(entries, &mut scratch);
    }
}

#[cfg(test)]
mod tests {
    use super::radix_sort_by_key;

    #[test]
    fn stable_radix_sort_matches_key_and_input_row_order() {
        for key_bytes in [0, 1, 3, 12] {
            let bits = key_bytes * 8;
            let mask = if bits == 0 { 0 } else { (1_u128 << bits) - 1 };
            let mut state = 0x9e37_79b9_7f4a_7c15_u128;
            let mut actual = Vec::new();
            for row in 0..4096 {
                state ^= state << 23;
                state ^= state >> 17;
                state ^= state << 26;
                actual.push((state & mask, row));
            }
            let mut expected = actual.clone();
            expected.sort_unstable();

            radix_sort_by_key(&mut actual, key_bytes);

            assert_eq!(actual, expected, "key_bytes={key_bytes}");
        }
    }
}
