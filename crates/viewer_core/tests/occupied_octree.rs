use std::collections::HashSet;
use viewer_core::{OccupiedOctree, OccupiedOctreeError, OccupiedOctreeRoot, VoxelCoord};

#[test]
fn empty_and_unit_root() {
    for depth in [0, 1, 32] {
        let root = OccupiedOctreeRoot::new([i32::MIN; 3], depth).unwrap();
        let tree = OccupiedOctree::from_cells(root, []).unwrap();
        assert_eq!(tree.levels().len(), usize::from(depth) + 1);
        assert!(tree.levels().iter().all(Vec::is_empty));
        assert_eq!(tree.cells().count(), 0);
        assert!(!tree.contains(VoxelCoord::new(i32::MIN, i32::MIN, i32::MIN)));
    }
    let root = OccupiedOctreeRoot::new([i32::MAX; 3], 0).unwrap();
    let cell = VoxelCoord::new(i32::MAX, i32::MAX, i32::MAX);
    let tree = OccupiedOctree::from_cells(root, [cell, cell]).unwrap();
    assert_eq!(tree.root(), root);
    assert_eq!(tree.cells().collect::<Vec<_>>(), vec![cell]);
    assert!(tree.contains(cell));
    assert_eq!(tree.levels()[0][0].child_mask(), 0);
    assert_eq!(tree.levels()[0][0].first_child(), 0);
}

#[test]
fn all_octants_follow_existing_xyz_order() {
    let cells = (0..8)
        .map(|i| VoxelCoord::new((i >> 2) & 1, (i >> 1) & 1, i & 1))
        .collect::<Vec<_>>();
    let tree =
        OccupiedOctree::from_cells(OccupiedOctreeRoot::new([0; 3], 1).unwrap(), cells.clone())
            .unwrap();
    let node = tree.levels()[0][0];
    assert_eq!(node.child_mask(), 255);
    assert_eq!(node.child_count(), 8);
    for octant in 0..8 {
        assert_eq!(node.child_index(octant), Some(usize::from(octant)));
    }
    assert_eq!(node.child_index(8), None);
    assert_eq!(node.child_index(255), None);
    assert_eq!(tree.cells().collect::<Vec<_>>(), cells);
}

#[test]
fn sparse_negative_cells_reconstruct_and_query_exactly() {
    let root = OccupiedOctreeRoot::new([-4; 3], 3).unwrap();
    let mut cells = Vec::new();
    for x in -4..4 {
        for y in -4..4 {
            for z in -4..4 {
                if (x * 7 + y * 3 + z) % 5 == 0 {
                    cells.push(VoxelCoord::new(x, y, z));
                }
            }
        }
    }
    let expected: HashSet<_> = cells.iter().copied().collect();
    let tree = OccupiedOctree::from_cells(root, cells.clone()).unwrap();
    cells.reverse();
    cells.extend(cells.clone());
    assert_eq!(tree, OccupiedOctree::from_cells(root, cells).unwrap());
    let reconstructed = tree.cells().collect::<Vec<_>>();
    assert_eq!(reconstructed.len(), expected.len());
    assert_eq!(reconstructed.into_iter().collect::<HashSet<_>>(), expected);
    for x in -5..5 {
        for y in -5..5 {
            for z in -5..5 {
                let cell = VoxelCoord::new(x, y, z);
                assert_eq!(tree.contains(cell), expected.contains(&cell));
            }
        }
    }
    // Independently propagate integer prefixes to check every mask, range and level.
    let mut origins = vec![[-4_i32; 3]];
    for level in 0..3 {
        let side = 1 << (2 - level);
        let mut next = Vec::new();
        for (node, origin) in tree.levels()[level].iter().zip(&origins) {
            let first = next.len();
            let mut mask = 0;
            for octant in 0..8 {
                let child: [i32; 3] =
                    std::array::from_fn(|axis| origin[axis] + ((octant >> (2 - axis)) & 1) * side);
                if expected.iter().any(|c| {
                    [c.x, c.y, c.z]
                        .iter()
                        .enumerate()
                        .all(|(axis, &v)| v >= child[axis] && v < child[axis] + side)
                }) {
                    mask |= 1 << octant;
                    assert_eq!(node.child_index(octant as u8), Some(next.len()));
                    next.push(child);
                } else {
                    assert_eq!(node.child_index(octant as u8), None);
                }
            }
            assert_eq!(node.child_mask(), mask);
            assert_eq!(node.first_child(), first);
            assert_eq!(node.child_count(), next.len() - first);
        }
        assert_eq!(next.len(), tree.levels()[level + 1].len());
        origins = next;
    }
}

#[test]
fn full_integer_domain_and_invalid_roots() {
    let root = OccupiedOctreeRoot::new([i32::MIN; 3], 32).unwrap();
    let cells = [
        VoxelCoord::new(i32::MIN, 0, i32::MAX),
        VoxelCoord::new(i32::MAX, i32::MIN, 0),
    ];
    let tree = OccupiedOctree::from_cells(root, cells).unwrap();
    assert_eq!(
        tree.cells().collect::<HashSet<_>>(),
        cells.into_iter().collect()
    );
    assert!(cells.into_iter().all(|c| tree.contains(c)));
    assert!(!tree.contains(VoxelCoord::new(0, 0, 0)));
    for (origin, depth) in [([0; 3], 32), ([i32::MIN; 3], 33), ([i32::MAX; 3], 1)] {
        assert_eq!(
            OccupiedOctreeRoot::new(origin, depth),
            Err(OccupiedOctreeError::InvalidRoot)
        );
    }
    let root = OccupiedOctreeRoot::new([-2; 3], 2).unwrap();
    for cell in [VoxelCoord::new(-3, 0, 0), VoxelCoord::new(0, 2, 0)] {
        assert_eq!(
            OccupiedOctree::from_cells(root, [cell]),
            Err(OccupiedOctreeError::OutsideRoot(cell))
        );
    }
}
