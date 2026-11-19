//! Task 10a1 pass criteria: the codec gives the same GPU bytes as packing directly, and malformed
//! input is an error, never a panic; a repeated node of the same generation is ignored.

use crate::cell_generation;
use viewer_core::NodeId;

use super::*;
use crate::{
    CellDriver, CellGrid, CellPoolError, CellTarget, DriverConfig, NodeAdded, NodeCell,
    NodeCellLevel, NodeCells, NodeInput, NodeInputError, PageEntry, PageFrame, PoolConfig,
    pack_node,
    scene_digest_tests::{ORIGIN, SIDE, ids},
    test_support::*,
};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
}

/// `count` distinct cells per depth (random colours, intensity and class on some), in sorted order.
fn node_cells(node: u64, depths: &[(u8, usize)]) -> NodeCells {
    let mut rng = Rng(node);
    let levels = depths
        .iter()
        .map(|&(depth, count)| {
            let span = 1_u64 << depth;
            let mut coords = std::collections::BTreeSet::new();
            while coords.len() < count {
                coords.insert(std::array::from_fn::<u64, 3, _>(|_| rng.next() % span));
            }
            NodeCellLevel {
                depth,
                cells: coords
                    .into_iter()
                    .map(|coord| {
                        let r = rng.next();
                        NodeCell {
                            coord,
                            rgb_sum: [r & 255, r >> 8 & 255, r >> 16 & 255],
                            intensity_sum: if r & 1 == 0 {
                                (r >> 24 & 1023) as f64
                            } else {
                                0.0
                            },
                            intensity_count: u32::from(r & 1 == 0),
                            count: 1,
                            classification: if r & 2 == 0 {
                                (r >> 40 & 31) as i16
                            } else {
                                -1
                            },
                            z_min: 0.0,
                            z_max: 0.0,
                        }
                    })
                    .collect(),
                octant_offsets: None,
            }
        })
        .collect();
    NodeCells {
        node: NodeId::new(node),
        levels,
        buildable: (
            depths.iter().map(|&(depth, _)| depth).min().unwrap_or(0),
            depths.iter().map(|&(depth, _)| depth).max().unwrap_or(0),
        ),
    }
}

fn local_grid() -> CellGrid {
    CellGrid {
        root_min: [0.0; 3],
        root_side: SIDE,
    }
}

fn pack(node: u64, depths: &[(u8, usize)]) -> PackedNode {
    pack_node(
        &grid(),
        ORIGIN,
        NodeInput {
            id: NodeId::new(node),
            bounds: cube(ORIGIN, SIDE),
            points: 1000 + node,
            local_grid: local_grid(),
            cells: &node_cells(node, depths),
        },
    )
    .expect("pack")
}

fn new_driver() -> CellDriver {
    CellDriver::new(DriverConfig {
        pool: PoolConfig::default(),
        grid: grid(),
        coordinate_origin: ORIGIN,
        cpu_budget_bytes: u64::MAX,
    })
}

/// A target that keeps every byte the driver writes, on top of the strict fake GPU.
#[derive(Default)]
struct Recording {
    gpu: FakeGpu,
    uploads: Vec<(NodeId, u8, Vec<u8>, Vec<u32>)>,
    tables: Vec<Vec<(u32, PageEntry)>>,
}

impl CellTarget for Recording {
    type Error = CellPoolError;

    fn has_pool(&self) -> bool {
        self.gpu.has_pool()
    }
    fn drop_pool(&mut self) {
        self.gpu.drop_pool();
    }
    fn eye(&self) -> [f64; 3] {
        self.gpu.eye()
    }
    fn upload_level(&mut self, level: &PackedLevel, slots: &[u32]) -> Result<(), CellPoolError> {
        let mut bytes = bytemuck::cast_slice::<_, u8>(&level.instances).to_vec();
        for page in &level.pages {
            bytes.extend_from_slice(&page.origin.map(u64::to_le_bytes).concat());
            bytes.extend_from_slice(&page.flags.to_le_bytes());
        }
        self.uploads
            .push((level.node, level.depth, bytes, slots.to_vec()));
        self.gpu.upload_level(level, slots)
    }
    fn free_pages(&mut self, slots: &[u32]) -> Result<(), CellPoolError> {
        self.gpu.free_pages(slots)
    }
    fn alloc_mask_slot(&mut self) -> Result<Option<u32>, CellPoolError> {
        self.gpu.alloc_mask_slot()
    }
    fn set_node_record(
        &mut self,
        slot: u32,
        record: &[u32; crate::NODE_WORDS],
    ) -> Result<(), CellPoolError> {
        self.gpu.set_node_record(slot, record)
    }
    fn set_active_pages(
        &mut self,
        pages: &[(u32, PageEntry)],
        _frame: &PageFrame,
    ) -> Result<(), CellPoolError> {
        // The fake GPU's page table check reads the self-describing fake levels, not real ones.
        self.tables.push(pages.to_vec());
        Ok(())
    }
}

const SHAPES: [&[(u8, usize)]; 4] = [
    &[(3, 5), (5, 40)],
    &[(4, 9), (6, 700), (7, 20)],
    &[(3, 1)],
    &[(2, 3), (5, 60), (8, 530)],
];

fn run(driver: &mut CellDriver, cut: &[u64]) -> Recording {
    let mut target = Recording::default();
    let cam = camera("zoom");
    target.gpu.eye = cam.position();
    for _ in 0..12 {
        let report = driver
            .frame(&mut target, &ids(cut), &view(&cam), 4.0)
            .expect("frame");
        assert_eq!(report.counters().pool_overflow, 0);
    }
    target
}

#[test]
fn encoded_nodes_decode_to_the_same_levels_and_the_same_gpu_bytes() {
    let mut direct = new_driver();
    let mut coded = new_driver();
    let mut cut = Vec::new();
    for (index, shape) in SHAPES.iter().enumerate() {
        let id = index as u64 + 2;
        cut.push(id);
        let node = pack(id, shape);
        assert!(node.levels.iter().all(|level| !level.pages.is_empty()));
        let bytes = encode_node_levels(&node).unwrap();
        let decoded = decode_node_levels(&bytes).unwrap();
        assert_eq!(decoded, node);
        assert_eq!(encode_node_levels(&decoded).unwrap(), bytes);

        let cells = node_cells(id, shape);
        let stored = direct
            .add_node(NodeInput {
                id: NodeId::new(id),
                bounds: cube(ORIGIN, SIDE),
                points: 1000 + id,
                local_grid: local_grid(),
                cells: &cells,
            })
            .unwrap();
        assert_eq!(
            coded.add_packed_generation(7, decoded),
            Ok(stored),
            "same result as the direct path"
        );
        assert_eq!(direct.cpu_bytes(), coded.cpu_bytes());
    }
    let a = run(&mut direct, &cut);
    let b = run(&mut coded, &cut);
    assert!(a.uploads.len() >= SHAPES.len());
    assert_eq!(a.uploads, b.uploads);
    assert_eq!(a.tables, b.tables);
}

#[test]
fn a_repeated_node_of_the_same_generation_is_ignored() {
    let mut driver = new_driver();
    let node = pack(2, SHAPES[0]);
    assert_eq!(
        driver.add_packed_generation(cell_generation(1, 0), node.clone()),
        Ok(NodeAdded::Stored { levels: 2 })
    );
    let bytes = driver.cpu_bytes();
    assert_eq!(
        driver.add_packed_generation(cell_generation(1, 0), node.clone()),
        Ok(NodeAdded::AlreadyStored)
    );
    assert_eq!(driver.cpu_bytes(), bytes);
    // Another generation, or a node stored without one, is a Duplicate error and changes nothing.
    assert_eq!(
        driver.add_packed_generation(cell_generation(2, 0), node.clone()),
        Err(NodeInputError::Duplicate(NodeId::new(2)))
    );
    let level = node.levels[0].clone();
    assert_eq!(
        driver.add_packed(
            NodeId::new(2),
            node.bounds,
            node.points,
            vec![level.clone()]
        ),
        Err(NodeInputError::Duplicate(NodeId::new(2)))
    );
    let mut other = new_driver();
    other
        .add_packed(NodeId::new(2), node.bounds, node.points, vec![level])
        .unwrap();
    assert_eq!(
        other.add_packed_generation(cell_generation(1, 0), node),
        Err(NodeInputError::Duplicate(NodeId::new(2)))
    );
    assert_eq!(driver.cpu_bytes(), bytes);
}

#[test]
fn pack_node_keeps_the_grid_and_node_contracts() {
    let cells = node_cells(2, SHAPES[0]);
    let input = |local_grid, id| NodeInput {
        id: NodeId::new(id),
        bounds: cube(ORIGIN, SIDE),
        points: 1,
        local_grid,
        cells: &cells,
    };
    assert!(matches!(
        pack_node(&grid(), ORIGIN, input(grid(), 2)),
        Err(NodeInputError::GridMismatch { .. })
    ));
    assert_eq!(
        pack_node(&grid(), ORIGIN, input(local_grid(), 3)),
        Err(NodeInputError::InvalidNode(NodeId::new(3)))
    );
}

fn small() -> Vec<u8> {
    encode_node_levels(&pack(2, &[(3, 5)])).unwrap()
}

const LEVEL_AT: usize = HEADER_BYTES;
const PAGE_AT: usize = LEVEL_AT + LEVEL_BYTES;
const INSTANCES_AT: usize = PAGE_AT + PAGE_RECORD_BYTES;

fn patched(offset: usize, bytes: &[u8]) -> Vec<u8> {
    let mut data = small();
    data[offset..offset + bytes.len()].copy_from_slice(bytes);
    data
}

#[test]
fn truncation_at_every_offset_is_an_error() {
    let data = small();
    assert_eq!(data.len(), INSTANCES_AT + PAGE_SLOTS * 16);
    for len in 0..data.len() {
        assert!(decode_node_levels(&data[..len]).is_err(), "length {len}");
    }
    assert!(decode_node_levels(&data).is_ok());
    let mut longer = data;
    longer.push(0);
    assert_eq!(decode_node_levels(&longer), Err(CodecError::TrailingBytes));
}

#[test]
fn header_and_count_errors() {
    assert_eq!(
        decode_node_levels(&patched(0, b"XXXX")),
        Err(CodecError::BadMagic)
    );
    assert_eq!(
        decode_node_levels(&patched(4, &1_u32.to_le_bytes())),
        Err(CodecError::UnsupportedVersion(1))
    );
    // Bounds that are not a box (NaN).
    assert!(decode_node_levels(&patched(24, &f64::NAN.to_le_bytes())).is_err());
    // Counts that would reserve gigabytes: errors, from the bytes left, not from the allocator.
    for count in [u32::MAX, 1 << 24, 2, 65] {
        assert!(decode_node_levels(&patched(HEADER_BYTES - 4, &count.to_le_bytes())).is_err());
    }
    for pages in [0_u32, 2, u32::MAX, 1 << 28] {
        assert!(decode_node_levels(&patched(LEVEL_AT + 4, &pages.to_le_bytes())).is_err());
    }
    for depth in [64_u32, u32::MAX, 2] {
        // Depth 2 holds a page at coordinates up to 7: its box is outside the level.
        assert!(decode_node_levels(&patched(LEVEL_AT, &depth.to_le_bytes())).is_err());
    }
}

#[test]
fn inconsistent_pages_are_errors() {
    let page = |offset: usize, bytes: &[u8]| decode_node_levels(&patched(PAGE_AT + offset, bytes));
    assert!(page(24, &1_u64.to_le_bytes()).is_err(), "first_cell");
    assert!(page(32, &0_u32.to_le_bytes()).is_err(), "no cells");
    assert!(page(32, &513_u32.to_le_bytes()).is_err(), "too many cells");
    assert!(page(32, &4_u32.to_le_bytes()).is_err(), "cells vs padding");
    assert!(page(0, &u64::MAX.to_le_bytes()).is_err(), "origin overflow");
    assert!(page(36, &u16::MAX.to_le_bytes()).is_err(), "box");
    assert!(page(42, &u32::MAX.to_le_bytes()).is_err(), "flags");
    // A cell's relative coordinate beyond the box, and a padding slot that is not the sentinel.
    let beyond = patched(INSTANCES_AT, &0x7fff_u32.to_le_bytes());
    assert!(decode_node_levels(&beyond).is_err());
    let slot = INSTANCES_AT + 5 * 16 + 8;
    assert!(decode_node_levels(&patched(slot, &1_u32.to_le_bytes())).is_err());
    let hole = patched(INSTANCES_AT, &u32::MAX.to_le_bytes());
    assert!(decode_node_levels(&hole).is_err(), "empty slot among cells");
}

#[test]
fn encode_refuses_levels_that_do_not_fit_the_node() {
    let mut node = pack(2, SHAPES[0]);
    node.levels[1].node = NodeId::new(9);
    assert!(encode_node_levels(&node).is_err());
    let mut node = pack(2, SHAPES[0]);
    node.levels[0].instances.pop();
    assert!(encode_node_levels(&node).is_err());
    let mut node = pack(2, SHAPES[0]);
    node.levels.reverse();
    assert!(encode_node_levels(&node).is_err());
}

#[test]
fn flipped_bytes_never_panic_and_accepted_input_is_canonical() {
    let data = encode_node_levels(&pack(3, SHAPES[3])).unwrap();
    let mut rng = Rng(0x5eed);
    let mut rejected = 0;
    for _ in 0..3000 {
        let mut mutated = data.clone();
        for _ in 0..=rng.next() % 3 {
            let at = if rng.next() % 2 == 0 {
                (rng.next() % (HEADER_BYTES as u64 + 200)) as usize
            } else {
                (rng.next() % data.len() as u64) as usize
            };
            mutated[at] ^= 1 << (rng.next() % 8);
        }
        match decode_node_levels(&mutated) {
            Ok(node) => assert_eq!(encode_node_levels(&node).unwrap(), mutated),
            Err(_) => rejected += 1,
        }
    }
    assert!(rejected > 1000, "{rejected}");
}

#[test]
fn encode_refuses_what_decode_would_refuse() {
    let base = pack(2, &[(3, 5)]);
    type Break = fn(&mut PackedNode);
    let cases: [(&str, Break); 8] = [
        ("first_cell", |n| n.levels[0].pages[0].first_cell = 1),
        ("flags", |n| n.levels[0].pages[0].flags = 0x1_0000),
        ("no cells", |n| n.levels[0].pages[0].cells = 0),
        ("too many cells", |n| n.levels[0].pages[0].cells = 513),
        ("padding", |n| {
            n.levels[0].instances[PAGE_SLOTS - 1].0[3] = 1;
        }),
        ("coordinate beyond box", |n| {
            n.levels[0].instances[0].0[0] = 0x7fff;
        }),
        ("box outside the grid", |n| {
            n.levels[0].pages[0].origin[0] = 8;
        }),
        ("depth", |n| {
            n.levels[0].depth = 64;
            n.levels[0].pages[0].depth = 64;
        }),
    ];
    for (name, break_it) in cases {
        let mut node = base.clone();
        break_it(&mut node);
        assert!(encode_node_levels(&node).is_err(), "{name}");
    }
    assert!(encode_node_levels(&base).is_ok());
}

#[test]
fn depth_63_boxes_are_checked_against_the_grid() {
    let level = |origin: u64| {
        let mut level = pack(2, &[(3, 1)]).levels.remove(0);
        level.depth = 63;
        level.pages[0].depth = 63;
        level.pages[0].origin = [origin, 0, 0];
        level.pages[0].box_max = [0, 0, 0];
        level.instances[0].0[0] = 0;
        level.instances[0].0[1] = 0;
        level
    };
    let node = |origin| PackedNode {
        levels: vec![level(origin)],
        ..pack(2, &[(3, 1)])
    };
    let last = (1_u64 << 63) - 1;
    assert_eq!(
        decode_node_levels(&encode_node_levels(&node(last)).unwrap()).unwrap(),
        node(last)
    );
    assert!(encode_node_levels(&node(last + 1)).is_err());
    assert!(encode_node_levels(&node(u64::MAX)).is_err());
}

#[test]
fn node_levels_round_trip_with_the_buildable_range() {
    let node = PackedNode {
        buildable: (1, 9),
        ..pack(2, &[(3, 5)])
    };
    let decoded = decode_node_levels(&encode_node_levels(&node).unwrap()).unwrap();
    assert_eq!(decoded, node);
    assert_eq!(decoded.buildable, (1, 9));
    for range in [(5, 4), (0, 64)] {
        let bad = PackedNode {
            buildable: range,
            ..node.clone()
        };
        assert!(encode_node_levels(&bad).is_err(), "{range:?}");
    }
}

#[test]
fn a_version_1_buffer_is_refused() {
    let mut old = small();
    // Version 1 had no buildable range between the bounds and the level count.
    old.drain(HEADER_BYTES - 6..HEADER_BYTES - 4);
    old[4..8].copy_from_slice(&1_u32.to_le_bytes());
    assert_eq!(
        decode_node_levels(&old),
        Err(CodecError::UnsupportedVersion(1))
    );
}

#[test]
fn a_version_2_buffer_with_46_byte_page_records_is_refused() {
    let mut old = small();
    // Version 2 pages had no `rel_origin`: 12 bytes less per page record.
    old.drain(PAGE_AT + 46..PAGE_AT + PAGE_RECORD_BYTES);
    old[4..8].copy_from_slice(&2_u32.to_le_bytes());
    assert_eq!(
        decode_node_levels(&old),
        Err(CodecError::UnsupportedVersion(2))
    );
}
