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
        let key_bytes = usize::from(root.depth()).saturating_mul(3).div_ceil(8);
        if key_bytes <= std::mem::size_of::<u64>() {
            let mut entries = cells
                .into_iter()
                .enumerate()
                .map(|(row, coord)| {
                    root.key(coord)
                        .map(|key| (key as u64, row))
                        .ok_or(OccupiedOctreeError::OutsideRoot(coord))
                })
                .collect::<Result<Vec<_>, _>>()?;
            radix_sort_by_key(&mut entries, key_bytes, u128::from);
            return Ok(Self::from_sorted_entries(root, entries, u128::from));
        }

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
        radix_sort_by_key(&mut entries, key_bytes, |key| key);
        Ok(Self::from_sorted_entries(root, entries, |key| key))
    }

    fn from_sorted_entries<K: Copy + Eq>(
        root: OccupiedOctreeRoot,
        entries: Vec<(K, usize)>,
        widen_key: impl Fn(K) -> u128,
    ) -> Self {
        let mut keys = Vec::new();
        let mut leaf_offsets = Vec::new();
        // Allocate compact row storage explicitly: collecting a consuming tuple iterator
        // can reuse its wider allocation and retain excessive capacity for usize rows.
        let mut row_ids = Vec::with_capacity(entries.len());
        let mut previous = None;
        for (offset, (key, row)) in entries.into_iter().enumerate() {
            row_ids.push(row);
            if previous != Some(key) {
                keys.push(widen_key(key));
                leaf_offsets.push(offset);
            }
            previous = Some(key);
        }
        // Sentinel also represents the empty mapping as [0].
        leaf_offsets.push(row_ids.len());
        let topology = OccupiedOctree::from_sorted_unique_keys(root, &keys);
        Self {
            topology,
            leaf_offsets,
            row_ids,
        }
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

fn radix_sort_by_key<K: Copy>(
    entries: &mut Vec<(K, usize)>,
    key_bytes: usize,
    key_value: impl Fn(K) -> u128,
) {
    if entries.len() < 2 || key_bytes == 0 {
        return;
    }
    let mut scratch = Vec::with_capacity(entries.len());
    for byte in 0..key_bytes {
        let shift = byte * 8;
        let mut counts = [0_usize; 256];
        for &(key, _) in entries.iter() {
            counts[((key_value(key) >> shift) & 0xff) as usize] += 1;
        }
        let mut next = [0_usize; 256];
        let mut total = 0;
        for (offset, count) in next.iter_mut().zip(counts) {
            *offset = total;
            total += count;
        }
        scratch.clear();
        scratch.resize(entries.len(), (entries[0].0, 0));
        for &(key, row) in entries.iter() {
            let bucket = ((key_value(key) >> shift) & 0xff) as usize;
            scratch[next[bucket]] = (key, row);
            next[bucket] += 1;
        }
        std::mem::swap(entries, &mut scratch);
    }
}

#[cfg(test)]
mod tests {
    use super::radix_sort_by_key;
    use crate::OccupiedOctreeRoot;

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

            radix_sort_by_key(&mut actual, key_bytes, |key| key);

            assert_eq!(actual, expected, "key_bytes={key_bytes}");
        }
    }

    #[test]
    fn compact_key_width_path_matches_full_morton_order() {
        for depth in [8, 21, 22, 32] {
            let root = OccupiedOctreeRoot::new([i32::MIN; 3], depth).unwrap();
            let mut state = 0x1234_5678_u32;
            let cells = (0..4096)
                .map(|_| {
                    let mut values = [0; 3];
                    for value in &mut values {
                        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                        let offset = if depth == 32 {
                            i64::from(state as i32) - i64::from(i32::MIN)
                        } else {
                            i64::from(state & ((1_u32 << depth) - 1))
                        };
                        *value = (i64::from(i32::MIN) + offset) as i32;
                    }
                    crate::VoxelCoord::new(values[0], values[1], values[2])
                })
                .collect::<Vec<_>>();
            let mut expected = cells
                .iter()
                .enumerate()
                .map(|(row, &cell)| (root.key(cell).unwrap(), row))
                .collect::<Vec<_>>();
            expected.sort_unstable();

            let index = crate::OccupiedCellIndex::new(root, cells).unwrap();
            let mut expected_keys = Vec::new();
            let mut expected_rows = Vec::new();
            let mut expected_offsets = Vec::new();
            for (key, row) in expected {
                if expected_keys.last() != Some(&key) {
                    expected_keys.push(key);
                    expected_offsets.push(expected_rows.len());
                }
                expected_rows.push(row);
            }
            expected_offsets.push(expected_rows.len());

            assert_eq!(
                index
                    .topology()
                    .cells()
                    .map(|cell| root.key(cell).unwrap())
                    .collect::<Vec<_>>(),
                expected_keys,
                "depth={depth}"
            );
            let actual_rows = (0..index.leaf_count())
                .flat_map(|leaf| index.rows_for_leaf(leaf).unwrap().iter().copied())
                .collect::<Vec<_>>();
            assert_eq!(actual_rows, expected_rows, "depth={depth}");
            for leaf in 0..index.leaf_count() {
                assert_eq!(
                    index.rows_for_leaf(leaf).unwrap(),
                    &expected_rows[expected_offsets[leaf]..expected_offsets[leaf + 1]],
                    "depth={depth}, leaf={leaf}"
                );
            }
        }
    }
}
