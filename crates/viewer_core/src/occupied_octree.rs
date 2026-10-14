//! Experimental occupancy topology, independent of storage and physical grid/CRS.
use thiserror::Error;

use crate::VoxelCoord;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OccupiedOctreeRoot {
    origin: [i32; 3],
    depth: u8,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum OccupiedOctreeError {
    #[error("root depth must be at most 32 and its cells must fit i32")]
    InvalidRoot,
    #[error("leaf cell {0:?} is outside the root")]
    OutsideRoot(VoxelCoord),
}

impl OccupiedOctreeRoot {
    /// A half-open cube of side `2^depth` in unit leaf cells. No physical units implied.
    pub fn new(origin: [i32; 3], depth: u8) -> Result<Self, OccupiedOctreeError> {
        if depth > 32
            || origin
                .iter()
                .any(|&v| i64::from(v) + (1_i64 << depth) > i64::from(i32::MAX) + 1)
        {
            return Err(OccupiedOctreeError::InvalidRoot);
        }
        Ok(Self { origin, depth })
    }

    pub fn origin(self) -> [i32; 3] {
        self.origin
    }

    pub fn depth(self) -> u8 {
        self.depth
    }

    fn key(self, coord: VoxelCoord) -> Option<u128> {
        let values = [coord.x, coord.y, coord.z];
        let relative: [i64; 3] =
            std::array::from_fn(|i| i64::from(values[i]) - i64::from(self.origin[i]));
        if relative
            .iter()
            .any(|&v| v < 0 || v >= (1_i64 << self.depth))
        {
            return None;
        }
        let mut key = 0;
        for bit in (0..self.depth).rev() {
            // Same octant convention as SpatialSubdivision::cell_index: x high, z low.
            for value in relative {
                key = (key << 1) | u128::from(((value >> bit) & 1) as u8);
            }
        }
        Some(key)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OccupiedOctreeNode {
    child_mask: u8,
    first_child: usize,
}

impl OccupiedOctreeNode {
    pub fn child_mask(self) -> u8 {
        self.child_mask
    }

    /// Index in the next level; zero for leaves. This is not a serialized address.
    pub fn first_child(self) -> usize {
        self.first_child
    }

    pub fn child_count(self) -> usize {
        self.child_mask.count_ones() as usize
    }

    pub fn child_index(self, octant: u8) -> Option<usize> {
        if octant >= 8 || self.child_mask & (1 << octant) == 0 {
            return None;
        }
        Some(self.first_child + (self.child_mask & ((1 << octant) - 1)).count_ones() as usize)
    }
}

/// Dense arrays of occupied nodes per level, with no retained node coordinates or payloads.
/// Duplicate cells collapse; input order does not affect topology. This is not a disk layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OccupiedOctree {
    root: OccupiedOctreeRoot,
    levels: Vec<Vec<OccupiedOctreeNode>>,
}

impl OccupiedOctree {
    /// Sorts temporary 96-bit path keys, then groups each level in O(cells * depth).
    /// Temporary keys and ranges are released on return; allocation failures are not recovered.
    pub fn from_cells(
        root: OccupiedOctreeRoot,
        cells: impl IntoIterator<Item = VoxelCoord>,
    ) -> Result<Self, OccupiedOctreeError> {
        let mut keys = cells
            .into_iter()
            .map(|coord| {
                root.key(coord)
                    .ok_or(OccupiedOctreeError::OutsideRoot(coord))
            })
            .collect::<Result<Vec<_>, _>>()?;
        keys.sort_unstable();
        keys.dedup();
        let mut ranges = Vec::new();
        if !keys.is_empty() {
            ranges.push(0..keys.len());
        }
        let mut levels = Vec::with_capacity(usize::from(root.depth) + 1);
        for level in 0..=root.depth {
            let mut nodes = Vec::with_capacity(ranges.len());
            let mut next = Vec::new();
            for range in ranges {
                let mut node = OccupiedOctreeNode {
                    child_mask: 0,
                    first_child: 0,
                };
                if level < root.depth {
                    node.first_child = next.len();
                    let shift = 3 * u32::from(root.depth - level - 1);
                    let mut start = range.start;
                    while start < range.end {
                        let octant = ((keys[start] >> shift) & 7) as u8;
                        let mut end = start + 1;
                        while end < range.end && ((keys[end] >> shift) & 7) as u8 == octant {
                            end += 1;
                        }
                        node.child_mask |= 1 << octant;
                        next.push(start..end);
                        start = end;
                    }
                }
                nodes.push(node);
            }
            levels.push(nodes);
            ranges = next;
        }
        Ok(Self { root, levels })
    }

    pub fn root(&self) -> OccupiedOctreeRoot {
        self.root
    }

    /// Root level first; an empty tree has depth + 1 empty levels.
    pub fn levels(&self) -> &[Vec<OccupiedOctreeNode>] {
        &self.levels
    }

    pub fn contains(&self, coord: VoxelCoord) -> bool {
        let Some(key) = self.root.key(coord) else {
            return false;
        };
        if self.levels[0].is_empty() {
            return false;
        }
        let mut index = 0;
        for level in 0..self.root.depth {
            let shift = 3 * u32::from(self.root.depth - level - 1);
            let Some(child) =
                self.levels[usize::from(level)][index].child_index(((key >> shift) & 7) as u8)
            else {
                return false;
            };
            index = child;
        }
        true
    }

    /// Reconstructs leaf cells in path order with O(depth) traversal state.
    pub fn cells(&self) -> impl Iterator<Item = VoxelCoord> + '_ {
        let mut stack = Vec::new();
        if !self.levels[0].is_empty() {
            stack.push((0_u8, 0_usize, self.root.origin.map(i64::from)));
        }
        std::iter::from_fn(move || {
            while let Some((level, index, origin)) = stack.pop() {
                if level == self.root.depth {
                    return Some(VoxelCoord::new(
                        origin[0] as i32,
                        origin[1] as i32,
                        origin[2] as i32,
                    ));
                }
                let node = self.levels[usize::from(level)][index];
                let side = 1_i64 << (self.root.depth - level - 1);
                for octant in (0..8).rev() {
                    if let Some(child) = node.child_index(octant) {
                        let child_origin = std::array::from_fn(|axis| {
                            origin[axis] + i64::from((octant >> (2 - axis)) & 1) * side
                        });
                        stack.push((level + 1, child, child_origin));
                    }
                }
            }
            None
        })
    }
}
