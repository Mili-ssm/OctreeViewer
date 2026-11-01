use std::collections::BTreeMap;

use viewer_core::{
    AttributeColumn, AttributeDataType, AttributeDescriptor, AttributeValues, BlockId, ColorRgb,
    NodeId, Point, PointBatch, PointBlock, SpatialBounds,
};

use crate::{
    CHAIN_SPAN, CellGrid, NodeCellSource, NodeCells, OrbitCamera, PointNodeCellSource, SceneBounds,
    node_cells_digest, representative_depth,
};

const SIDE: f64 = 64.0;
const VIEWPORT: [u32; 2] = [800, 600];

struct Lcg(u64);

impl Lcg {
    fn next_unit(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 40) as f32) / ((1_u64 << 24) as f32)
    }
}

fn grid() -> CellGrid {
    CellGrid {
        root_min: [0.0; 3],
        root_side: SIDE,
    }
}

fn cube(min: [f64; 3], side: f64) -> SpatialBounds {
    SpatialBounds::new(min, min.map(|value| value + side)).unwrap()
}

fn plain_block(points: Vec<Point>) -> PointBlock {
    PointBlock::new(BlockId::new(1), PointBatch::new(points), Vec::new())
}

fn random_points(rng: &mut Lcg, lo: [f32; 3], size: f32, count: usize) -> Vec<Point> {
    (0..count)
        .map(|_| {
            let position = std::array::from_fn(|axis| lo[axis] + rng.next_unit() * size);
            let color = ColorRgb::new(
                (rng.next_unit() * 255.0) as u8,
                (rng.next_unit() * 255.0) as u8,
                (rng.next_unit() * 255.0) as u8,
            );
            Point::new(position, color)
        })
        .collect()
}

fn attributed_block(points: Vec<Point>, intensity: Vec<u16>, class: Vec<u8>) -> PointBlock {
    let count = points.len();
    PointBlock::new(
        BlockId::new(1),
        PointBatch::new(points),
        vec![
            AttributeColumn::new(
                AttributeDescriptor::new("intensity", AttributeDataType::U16, 1).unwrap(),
                AttributeValues::U16(intensity),
                count,
            )
            .unwrap(),
            AttributeColumn::new(
                AttributeDescriptor::new("classification", AttributeDataType::U8, 1).unwrap(),
                AttributeValues::U8(class),
                count,
            )
            .unwrap(),
        ],
    )
}

fn cells_of(grid: &CellGrid, bounds: SpatialBounds, leaf: bool, block: &PointBlock) -> NodeCells {
    PointNodeCellSource
        .node_cells(grid, NodeId::new(7), bounds, leaf, block)
        .unwrap()
}

#[test]
fn internal_node_cells_are_its_representatives() {
    // Node side is root / divisions^level; every division count yields 8 cells per axis.
    for (divisions, expected_depth) in [(2_u32, 4_u8), (4, 5), (8, 6)] {
        let node_side = SIDE / f64::from(divisions);
        let bounds = cube([0.0; 3], node_side);
        assert_eq!(representative_depth(&grid(), bounds), expected_depth);
        let mut rng = Lcg(0x1000 + u64::from(divisions));
        let points = random_points(&mut rng, [0.0; 3], node_side as f32, 3000);
        let cell_side = node_side / 8.0;
        let mut expected = BTreeMap::<[u32; 3], u32>::new();
        for point in &points {
            let coord = point
                .position
                .map(|v| (f64::from(v) / cell_side).floor() as u32);
            assert!(coord.iter().all(|c| *c < 8));
            *expected.entry(coord).or_default() += 1;
        }
        let cells = cells_of(&grid(), bounds, false, &plain_block(points));
        assert_eq!(cells.levels.len(), 1, "divisions {divisions}");
        let level = &cells.levels[0];
        assert_eq!(level.depth, expected_depth);
        let actual: BTreeMap<_, _> = level.cells.iter().map(|c| (c.coord, c.count)).collect();
        assert_eq!(actual, expected, "divisions {divisions}");
        assert!(level.cells.len() > 400, "fixture should fill most subcells");
    }
}

#[test]
fn node_cells_do_not_depend_on_camera() {
    let bounds = cube([0.0; 3], 32.0);
    let mut rng = Lcg(5);
    let block = plain_block(random_points(&mut rng, [0.0; 3], 32.0, 500));
    let scene = SceneBounds {
        min: [0.0; 3],
        max: [SIDE; 3],
    };
    let reference = node_cells_digest(&cells_of(&grid(), bounds, true, &block));
    for (yaw, zoom) in [(0.0, 0.0), (0.4, 0.0), (0.0, -0.7), (-0.3, 0.5)] {
        let mut camera = OrbitCamera::default();
        assert!(camera.fit(scene, VIEWPORT));
        camera.orbit(yaw, 0.1);
        camera.zoom(zoom);
        // The projection is evaluated in the same context but is not an input of the cells.
        let _ = camera.projected_voxel_edge_pixels(bounds, [0.0; 3], VIEWPORT);
        let digest = node_cells_digest(&cells_of(&grid(), bounds, true, &block));
        assert_eq!(digest, reference);
    }
    let other = plain_block(random_points(&mut Lcg(6), [0.0; 3], 32.0, 500));
    assert_ne!(
        node_cells_digest(&cells_of(&grid(), bounds, true, &other)),
        reference
    );
}

#[test]
fn cell_sums_match_accumulate_block() {
    let bounds = cube([16.0; 3], 16.0);
    let mut rng = Lcg(77);
    let points = random_points(&mut rng, [16.0; 3], 16.0, 2000);
    let intensity: Vec<u16> = (0..points.len()).map(|i| (i * 37 % 1000) as u16).collect();
    let class: Vec<u8> = (0..points.len()).map(|i| (i % 11) as u8).collect();
    let block = attributed_block(points.clone(), intensity.clone(), class.clone());
    let cells = cells_of(&grid(), bounds, false, &block);
    let level = &cells.levels[0];
    let cell_side = SIDE / f64::from(1_u32 << level.depth);

    // Same sums, same point order: u64 colour, f64 intensity, classification of the first point.
    #[derive(Default)]
    struct Reference {
        rgb: [u64; 3],
        intensity: f64,
        count: u32,
        class: u8,
        z: (f32, f32),
    }
    let mut reference = BTreeMap::<[u32; 3], Reference>::new();
    for (index, point) in points.iter().enumerate() {
        let coord = point
            .position
            .map(|v| (f64::from(v) / cell_side).floor() as u32);
        let entry = reference.entry(coord).or_default();
        if entry.count == 0 {
            entry.class = class[index];
            entry.z = (point.position[2], point.position[2]);
        }
        entry.rgb[0] += u64::from(point.color.r);
        entry.rgb[1] += u64::from(point.color.g);
        entry.rgb[2] += u64::from(point.color.b);
        entry.intensity += f64::from(intensity[index]);
        entry.count += 1;
        entry.z.0 = entry.z.0.min(point.position[2]);
        entry.z.1 = entry.z.1.max(point.position[2]);
    }
    assert_eq!(level.cells.len(), reference.len());
    for cell in &level.cells {
        let expected = &reference[&cell.coord];
        assert_eq!(cell.rgb_sum, expected.rgb);
        assert_eq!(cell.intensity_sum.to_bits(), expected.intensity.to_bits());
        assert_eq!(cell.count, expected.count);
        assert_eq!(cell.classification, expected.class);
        assert_eq!((cell.z_min, cell.z_max), expected.z);
    }
    let morton_sorted = level
        .cells
        .windows(2)
        .all(|pair| morton(pair[0].coord) < morton(pair[1].coord));
    assert!(morton_sorted, "cells must be in Morton order");
}

fn morton(coord: [u32; 3]) -> u128 {
    (0..32).fold(0_u128, |acc, bit| {
        (0..3).fold(acc, |acc, axis| {
            acc | (u128::from((coord[axis] >> bit) & 1) << (3 * bit + (2 - axis)))
        })
    })
}

#[test]
fn points_on_shared_and_max_faces_count_once() {
    // Node [8, 16]^3 of a 64 m root: depth 6, cells 8..=15 per axis.
    let bounds = cube([8.0; 3], 8.0);
    let mut points = Vec::new();
    for x in [8.0_f32, 12.0, 16.0] {
        for y in [8.0_f32, 12.0, 16.0] {
            for z in [8.0_f32, 12.0, 16.0] {
                points.push(Point::new([x, y, z], ColorRgb::new(1, 2, 3)));
            }
        }
    }
    // Float noise just outside the node and non-finite points.
    points.push(Point::new([7.999_99; 3], ColorRgb::new(1, 2, 3)));
    points.push(Point::new([16.000_1; 3], ColorRgb::new(1, 2, 3)));
    let finite = points.len() as u32;
    points.push(Point::new([f32::NAN, 9.0, 9.0], ColorRgb::new(1, 2, 3)));
    points.push(Point::new(
        [9.0, f32::INFINITY, 9.0],
        ColorRgb::new(1, 2, 3),
    ));
    for leaf in [false, true] {
        let cells = cells_of(&grid(), bounds, leaf, &plain_block(points.clone()));
        for level in &cells.levels {
            let total: u32 = level.cells.iter().map(|cell| cell.count).sum();
            assert_eq!(total, finite, "leaf {leaf}, depth {}", level.depth);
            let n = 1_u32 << level.depth;
            let cells_per_node = n / 8;
            let lo = n / 8;
            for cell in &level.cells {
                for c in cell.coord {
                    assert!(c >= lo && c < lo + cells_per_node, "{:?}", cell.coord);
                }
            }
        }
    }
    // The max-face point lands in the last cell, the shared plane in the upper cell.
    let cells = cells_of(&grid(), bounds, false, &plain_block(points));
    let coords: Vec<_> = cells.levels[0].cells.iter().map(|c| c.coord).collect();
    assert!(coords.contains(&[15, 15, 15]));
    assert!(coords.contains(&[12, 12, 12]));
}

#[test]
fn leaf_chain_stops_when_cells_stop_growing() {
    let bounds = cube([0.0; 3], 16.0);
    let points = vec![Point::new([3.0, 4.0, 5.0], ColorRgb::new(9, 9, 9)); 40];
    let cells = cells_of(&grid(), bounds, true, &plain_block(points));
    assert_eq!(cells.levels.len(), 1);
    assert_eq!(cells.levels[0].depth, representative_depth(&grid(), bounds));
    assert_eq!(cells.levels[0].cells.len(), 1);
    assert_eq!(cells.levels[0].cells[0].count, 40);
}

#[test]
fn leaf_chain_respects_ten_bit_span() {
    // Root-sized node: representative depth 3. Point `b_j` shares the level-(2 + j) cell of `a`
    // and separates at level 3 + j, so the cell count grows at every level.
    let bounds = cube([0.0; 3], SIDE);
    let d0 = representative_depth(&grid(), bounds);
    assert_eq!(d0, 3);
    let mut points = vec![Point::new([0.0; 3], ColorRgb::new(1, 1, 1))];
    for j in 0..=u32::from(CHAIN_SPAN) + 2 {
        let x = (SIDE * 1.5 / f64::from(1_u32 << (u32::from(d0) + j))) as f32;
        points.push(Point::new([x, 0.0, 0.0], ColorRgb::new(2, 2, 2)));
    }
    let cells = cells_of(&grid(), bounds, true, &plain_block(points));
    let depths: Vec<_> = cells.levels.iter().map(|level| level.depth).collect();
    let expected: Vec<_> = (d0..=d0 + CHAIN_SPAN).collect();
    assert_eq!(depths, expected);
    assert!(
        cells
            .levels
            .windows(2)
            .all(|pair| pair[0].cells.len() < pair[1].cells.len())
    );
}

#[test]
fn page_origin_relative_precision() {
    // A root 10^6 m from the origin: centres relative to the page origin keep f32 precision.
    let far = CellGrid {
        root_min: [1_000_000.1; 3],
        root_side: SIDE,
    };
    let bounds = cube([1_000_000.1; 3], SIDE);
    let mut points = Vec::new();
    for i in 0..8_u32 {
        let v = (1_000_000.1 + 4.0 + 8.0 * f64::from(i)) as f32;
        points.push(Point::new([v; 3], ColorRgb::new(5, 5, 5)));
    }
    let cells = cells_of(&far, bounds, false, &plain_block(points));
    let level = &cells.levels[0];
    assert_eq!(level.cells.len(), 8);
    let cell_side = SIDE / f64::from(1_u32 << level.depth);
    let page_origin = bounds.min();
    let camera: [f64; 3] = page_origin.map(|v| v - 20.0);
    let mut naive_worst = 0.0_f64;
    for cell in &level.cells {
        let center: [f64; 3] =
            std::array::from_fn(|a| far.root_min[a] + (f64::from(cell.coord[a]) + 0.5) * cell_side);
        let distance = (0..3)
            .map(|a| (center[a] - camera[a]).powi(2))
            .sum::<f64>()
            .sqrt();
        let tolerance = 1.0e-6 * distance;
        let relative: [f32; 3] = std::array::from_fn(|a| (center[a] - page_origin[a]) as f32);
        for a in 0..3 {
            let rebuilt = page_origin[a] + f64::from(relative[a]);
            assert!((rebuilt - center[a]).abs() <= tolerance);
            naive_worst =
                naive_worst.max((f64::from(center[a] as f32) - center[a]).abs() / tolerance);
        }
    }
    assert!(
        naive_worst > 1.0,
        "absolute f32 centres must be visibly worse"
    );
}

#[test]
fn representative_depth_snaps_float_noise_to_the_intended_level() {
    let grid = grid();
    // Majadas has nodes whose side ratio gives log2 = 13.9999904 instead of 14 (f32 bounds).
    let exact = grid.root_side / f64::from(1u32 << 11);
    let noisy = cube(grid.root_min, exact * (1.0 + 1e-5));
    assert_eq!(representative_depth(&grid, noisy), 14);
    let coarser = cube(grid.root_min, exact * 1.5);
    assert_eq!(representative_depth(&grid, coarser), 13);
}
