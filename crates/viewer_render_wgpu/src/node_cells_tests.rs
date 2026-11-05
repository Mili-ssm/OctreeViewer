use std::collections::BTreeMap;

use viewer_core::{
    AttributeColumn, AttributeDataType, AttributeDescriptor, AttributeValues, BlockId, ColorRgb,
    NodeId, Point, PointBatch, PointBlock, SpatialBounds,
};

use crate::{
    CHAIN_MIN_GROWTH, CHAIN_SPAN, CellGrid, NodeCellSource, NodeCells, OrbitCamera,
    PointNodeCellSource, SceneBounds, chain_keep_mask, node_cells_digest, representative_depth,
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
    PointNodeCellSource::default()
        .node_cells(
            grid,
            NodeId::new(7),
            bounds,
            None,
            leaf,
            std::slice::from_ref(block),
        )
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
        let mut expected = BTreeMap::<[u64; 3], u32>::new();
        for point in &points {
            let coord = point
                .position
                .map(|v| (f64::from(v) / cell_side).floor() as u64);
            assert!(coord.iter().all(|c| *c < 8));
            *expected.entry(coord).or_default() += 1;
        }
        let level = &PointNodeCellSource { min_growth: 1.0 }
            .node_cells(
                &grid(),
                NodeId::new(7),
                bounds,
                None,
                false,
                &[plain_block(points.clone())],
            )
            .unwrap()
            .levels[0];
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
    let mut reference = BTreeMap::<[u64; 3], Reference>::new();
    for (index, point) in points.iter().enumerate() {
        let coord = point
            .position
            .map(|v| (f64::from(v) / cell_side).floor() as u64);
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
        assert_eq!(cell.classification, i16::from(expected.class));
        assert_eq!((cell.z_min, cell.z_max), expected.z);
    }
    let morton_sorted = level
        .cells
        .windows(2)
        .all(|pair| morton(pair[0].coord) < morton(pair[1].coord));
    assert!(morton_sorted, "cells must be in Morton order");
}

fn morton(coord: [u64; 3]) -> u128 {
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
    for leaf in [false] {
        let cells = cells_of(&grid(), bounds, leaf, &plain_block(points.clone()));
        for level in &cells.levels {
            let total: u32 = level.cells.iter().map(|cell| cell.count).sum();
            assert_eq!(total, finite, "leaf {leaf}, depth {}", level.depth);
            let n = 1_u64 << level.depth;
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
    let cells = PointNodeCellSource { min_growth: 1.0 }
        .node_cells(
            &grid(),
            NodeId::new(7),
            bounds,
            None,
            false,
            &[plain_block(points)],
        )
        .unwrap();
    let coords: Vec<_> = cells.levels[0].cells.iter().map(|c| c.coord).collect();
    assert!(coords.contains(&[15, 15, 15]));
    assert!(coords.contains(&[12, 12, 12]));
}

#[test]
fn adaptive_leaf_counts_shared_and_max_faces_and_clamps_to_node() {
    let bounds = cube([8.0; 3], 8.0);
    let points = vec![
        Point::new([8.0; 3], ColorRgb::new(1, 2, 3)),
        Point::new([12.0; 3], ColorRgb::new(4, 5, 6)),
        Point::new([16.0; 3], ColorRgb::new(7, 8, 9)),
        Point::new([16.0; 3], ColorRgb::new(10, 11, 12)),
        Point::new([f32::NAN, 8.0, 8.0], ColorRgb::new(0, 0, 0)),
    ];
    let cells = cells_of(&grid(), bounds, true, &plain_block(points));
    let level = cells.levels.last().unwrap();
    assert_eq!(level.depth, 6);
    assert_eq!(level.cells.iter().map(|cell| cell.count).sum::<u32>(), 4);
    let max = level
        .cells
        .iter()
        .find(|cell| cell.coord == [15, 15, 15])
        .unwrap();
    assert_eq!(max.count, 2);
    assert_eq!(max.rgb_sum, [17, 19, 21]);
    assert!(level.cells.iter().any(|cell| cell.coord == [12, 12, 12]));
    assert!(level.cells.iter().any(|cell| cell.coord == [8, 8, 8]));
}

#[test]
fn adaptive_leaf_rejects_distinct_points_clamped_to_same_face() {
    let points = vec![
        Point::new([16.0; 3], ColorRgb::new(1, 2, 3)),
        Point::new([16.000_1; 3], ColorRgb::new(4, 5, 6)),
    ];
    let result = PointNodeCellSource::default().node_cells(
        &grid(),
        NodeId::new(1),
        cube([8.0; 3], 8.0),
        None,
        true,
        &[plain_block(points)],
    );
    assert!(matches!(
        result,
        Err(crate::NodeCellError::AbsoluteDepthExceeded)
    ));
}

#[test]
fn leaf_chain_stops_when_all_positions_are_duplicates() {
    let bounds = cube([0.0; 3], 16.0);
    let points = vec![Point::new([3.0, 4.0, 5.0], ColorRgb::new(9, 9, 9)); 40];
    let cells = cells_of(&grid(), bounds, true, &plain_block(points));
    assert_eq!(cells.levels.len(), 1);
    assert_eq!(cells.levels[0].depth, representative_depth(&grid(), bounds));
    assert_eq!(cells.levels[0].cells.len(), 1);
    assert_eq!(cells.levels[0].cells[0].count, 40);
}

#[test]
fn leaf_chain_extends_past_previous_fixed_span() {
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
    // Growth 1.0 keeps every level; the default growth prunes this slowly growing chain.
    let cells = PointNodeCellSource { min_growth: 1.0 }
        .node_cells(
            &grid(),
            NodeId::new(7),
            bounds,
            None,
            true,
            &[plain_block(points)],
        )
        .unwrap();
    let depths: Vec<_> = cells.levels.iter().map(|level| level.depth).collect();
    let expected: Vec<_> = (d0..=d0 + CHAIN_SPAN + 2).collect();
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
            std::array::from_fn(|a| far.root_min[a] + (cell.coord[a] as f64 + 0.5) * cell_side);
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

#[test]
fn chain_keep_mask_keeps_finest_and_geometric_levels() {
    // 100 is kept; 90 and 80 are within 1.5x of it; 60 is not; 50 is dropped against 60; 20 is kept.
    let counts = [20, 50, 60, 80, 90, 100];
    assert_eq!(
        chain_keep_mask(&counts, 1.5),
        [true, false, true, false, false, true]
    );
    assert_eq!(chain_keep_mask(&counts, 1.0), [true; 6]);
    assert!(chain_keep_mask(&[], 1.5).is_empty());
}

#[test]
fn leaf_chain_has_at_most_three_cells_per_point() {
    let mut rng = Lcg(5);
    let bounds = cube([0.0; 3], 16.0);
    for count in [3_usize, 40, 300, 2000] {
        let points = random_points(&mut rng, [0.0; 3], 16.0, count);
        let cells = cells_of(&grid(), bounds, true, &plain_block(points));
        let total: usize = cells.levels.iter().map(|level| level.cells.len()).sum();
        assert!(
            (total as f64) <= count as f64 * CHAIN_MIN_GROWTH / (CHAIN_MIN_GROWTH - 1.0),
            "{count} points gave {total} cells"
        );
        let finest = cells.levels.last().unwrap();
        let counts: usize = finest.cells.iter().map(|cell| cell.count as usize).sum();
        assert_eq!(counts, count, "the finest level keeps every point");
    }
}

#[test]
fn leaf_chain_keeps_finer_detail_after_a_plateau() {
    let bounds = cube([0.0; 3], SIDE);
    let points = vec![
        Point::new([1.0, 0.0, 0.0], ColorRgb::new(1, 2, 3)),
        Point::new([1.5, 0.0, 0.0], ColorRgb::new(4, 5, 6)),
    ];
    let cells = cells_of(&grid(), bounds, true, &plain_block(points));
    assert!(cells.levels.last().unwrap().cells.len() == 2);
}

#[test]
fn blocks_preserve_missing_attributes_and_point_order() {
    let p = Point::new([1.0; 3], ColorRgb::new(2, 3, 4));
    let blocks = [
        plain_block(vec![p]),
        attributed_block(vec![p], vec![12], vec![7]),
    ];
    let cells = PointNodeCellSource::default()
        .node_cells(
            &grid(),
            NodeId::new(1),
            cube([0.0; 3], SIDE),
            None,
            false,
            &blocks,
        )
        .unwrap();
    for level in cells.levels {
        let cell = &level.cells[0];
        assert_eq!(
            (
                cell.count,
                cell.intensity_count,
                cell.intensity_sum,
                cell.classification
            ),
            (2, 1, 12.0, -1)
        );
        assert_eq!(cell.rgb_sum, [4, 6, 8]);
    }
}

#[test]
fn absolute_depth_above_24_and_subregions() {
    let deep = CellGrid {
        root_min: [0.0; 3],
        root_side: 2.0_f64.powi(26),
    };
    let block = plain_block(vec![Point::new([0.25; 3], ColorRgb::new(1, 2, 3))]);
    let cells = PointNodeCellSource::default()
        .node_cells(
            &deep,
            NodeId::new(1),
            cube([0.0; 3], 2.0),
            None,
            false,
            std::slice::from_ref(&block),
        )
        .unwrap();
    assert!(cells.levels.iter().all(|level| level.depth > 24));
    let result = PointNodeCellSource::default().node_cells(
        &grid(),
        NodeId::new(1),
        cube([0.0; 3], SIDE),
        Some(SIDE / 2.0_f64.powi(22)),
        true,
        &[block],
    );
    assert!(result.is_ok());
}

#[test]
fn node_cells_digest_covers_multiple_blocks_and_missing_attributes() {
    let p = Point::new([1.0; 3], ColorRgb::new(2, 3, 4));
    let blocks = [
        plain_block(vec![p]),
        attributed_block(vec![p], vec![12], vec![7]),
    ];
    let cells = PointNodeCellSource::default()
        .node_cells(
            &grid(),
            NodeId::new(1),
            cube([0.0; 3], SIDE),
            None,
            false,
            &blocks,
        )
        .unwrap();
    assert_eq!(node_cells_digest(&cells), 11_489_581_987_825_140_401);
}

#[test]
fn node_cell_color_and_attributes_match_cpu_voxel_builder() {
    let p1 = Point::new([1.0; 3], ColorRgb::new(1, 4, 12));
    let p2 = Point::new([2.0; 3], ColorRgb::new(2, 9, 25));
    let blocks = [
        plain_block(vec![p1]),
        attributed_block(vec![p2], vec![20], vec![7]),
    ];
    let bounds = cube([0.0; 3], SIDE);
    let project = |b: SpatialBounds| b.max()[0] - b.min()[0];
    let mut cpu = crate::voxel_grid::VoxelSceneBuilder::new(bounds, [0.0; 3], 8.0, 100);
    for block in &blocks {
        cpu.add_block(block, &project).unwrap();
    }
    let scene = cpu.finish(&project);
    let actual = &scene.scene.instances()[0];
    let cells = PointNodeCellSource { min_growth: 1.0 }
        .node_cells(&grid(), NodeId::new(1), bounds, None, false, &blocks)
        .unwrap();
    let cell = &cells.levels[0].cells[0];
    let rgb = cell
        .rgb_sum
        .map(|sum| ((sum + u64::from(cell.count) / 2) / u64::from(cell.count)) as u8);
    assert_eq!(
        actual.color,
        ColorRgb::new(rgb[0], rgb[1], rgb[2]).to_linear_rgba()
    );
    assert_eq!(
        actual.intensity,
        (cell.intensity_sum / f64::from(cell.intensity_count)) as f32
    );
    assert_eq!(actual.classification, f32::from(cell.classification));
    assert_eq!(cell.count, 2);
}

#[test]
fn leaf_stops_when_each_finite_point_has_its_own_cell() {
    let bounds = cube([0.0; 3], SIDE);
    let block = plain_block(vec![Point::new([1.0; 3], ColorRgb::new(1, 2, 3))]);
    let cells = PointNodeCellSource::default()
        .node_cells(
            &grid(),
            NodeId::new(1),
            bounds,
            Some(SIDE / 2.0_f64.powi(15)),
            true,
            &[block],
        )
        .unwrap();
    assert_eq!(cells.levels.len(), 1);
    assert_eq!(cells.levels[0].depth, 15);
}

#[test]
fn cpu_voxel_builder_matches_multiple_cells_and_attribute_presence() {
    let p1 = Point::new([1.0; 3], ColorRgb::new(1, 4, 12));
    let p2 = Point::new([2.0; 3], ColorRgb::new(2, 9, 25));
    let p3 = Point::new([10.0; 3], ColorRgb::new(6, 8, 10));
    for blocks in [
        vec![
            attributed_block(vec![p1, p3], vec![0, 80], vec![0, 8]),
            plain_block(vec![p2]),
        ],
        vec![plain_block(vec![p1, p3]), plain_block(vec![p2])],
    ] {
        let bounds = cube([0.0; 3], SIDE);
        let project = |b: SpatialBounds| b.max()[0] - b.min()[0];
        let mut cpu = crate::voxel_grid::VoxelSceneBuilder::new(bounds, [0.0; 3], 8.0, 100);
        for block in &blocks {
            cpu.add_block(block, &project).unwrap();
        }
        let scene = cpu.finish(&project);
        let cells = PointNodeCellSource { min_growth: 1.0 }
            .node_cells(&grid(), NodeId::new(1), bounds, None, false, &blocks)
            .unwrap();
        let level = &cells.levels[0];
        assert_eq!(level.cells.len(), scene.scene.instances().len());
        for cell in &level.cells {
            let center = cell.coord.map(|coord| (coord as f32 + 0.5) * 8.0);
            let instance = scene
                .scene
                .instances()
                .iter()
                .find(|instance| instance.center == center)
                .unwrap();
            let rgb = cell
                .rgb_sum
                .map(|sum| ((sum + u64::from(cell.count) / 2) / u64::from(cell.count)) as u8);
            assert_eq!(
                instance.color,
                ColorRgb::new(rgb[0], rgb[1], rgb[2]).to_linear_rgba()
            );
            let intensity = if cell.intensity_count == 0 {
                -1.0
            } else {
                (cell.intensity_sum / f64::from(cell.intensity_count)) as f32
            };
            assert_eq!(instance.intensity, intensity);
            assert_eq!(instance.classification, f32::from(cell.classification));
        }
    }
}

#[test]
fn adaptive_leaf_separates_distinct_positions_after_old_limit_with_duplicates() {
    let blocks = [
        plain_block(vec![Point::new([0.0; 3], ColorRgb::new(1, 2, 3))]),
        attributed_block(
            vec![
                Point::new([-0.0, 0.0, 0.0], ColorRgb::new(4, 5, 6)),
                Point::new([0.03125, 0.0, 0.0], ColorRgb::new(7, 8, 9)),
            ],
            vec![10, 20],
            vec![3, 4],
        ),
    ];
    let cells = PointNodeCellSource::default()
        .node_cells(
            &grid(),
            NodeId::new(1),
            cube([0.0; 3], SIDE),
            None,
            true,
            &blocks,
        )
        .unwrap();
    let last = cells.levels.last().unwrap();
    assert_eq!(last.depth, 11);
    assert_eq!(last.cells.len(), 2);
    assert_eq!(last.cells[0].count, 2);
    assert_eq!(last.cells[0].rgb_sum, [5, 7, 9]);
    assert_eq!(last.cells[0].intensity_sum, 10.0);
}

#[test]
fn adaptive_leaf_crosses_region_limit_and_rejects_clamped_collisions() {
    let source = PointNodeCellSource::default();
    let bounds = cube([0.0; 3], SIDE);
    let point = |x| Point::new([x, 0.0, 0.0], ColorRgb::new(1, 2, 3));
    let build = |points| {
        source.node_cells(
            &grid(),
            NodeId::new(1),
            bounds,
            Some(SIDE / 2.0_f64.powi(21)),
            true,
            &[plain_block(points)],
        )
    };
    let cells = build(vec![point(0.0), point((SIDE / 2.0_f64.powi(21)) as f32)]).unwrap();
    assert_eq!(cells.levels[0].depth, 21);
    assert_eq!(cells.levels[0].cells.len(), 2);
    assert_eq!(
        build(vec![point(0.0), point((SIDE / 2.0_f64.powi(22)) as f32)])
            .unwrap()
            .levels
            .last()
            .unwrap()
            .depth,
        22
    );
    assert!(matches!(
        build(vec![point(64.0), point(65.0)]),
        Err(crate::NodeCellError::AbsoluteDepthExceeded)
    ));
    let max = build(vec![point(63.0), point(64.0)]).unwrap();
    assert_eq!(max.levels[0].cells.len(), 2);
}

#[test]
fn subregions_preserve_global_morton_order_and_aggregate_duplicates() {
    let unit = SIDE / 2.0_f64.powi(23);
    let coords = [
        [2097152, 0, 0],
        [0, 2097152, 0],
        [0, 0, 2097152],
        [2097151, 2097151, 2097151],
        [2097152, 2097152, 2097152],
        [4194304, 0, 0],
        [0, 4194304, 0],
        [0, 0, 4194304],
    ];
    let points: Vec<_> = coords
        .iter()
        .map(|coord| {
            Point::new(
                coord.map(|v| (f64::from(v) * unit) as f32),
                ColorRgb::new(1, 2, 3),
            )
        })
        .collect();
    let blocks = [
        plain_block(points.clone()),
        attributed_block(vec![points[0]], vec![7], vec![9]),
    ];
    let cells = PointNodeCellSource::default()
        .node_cells(
            &grid(),
            NodeId::new(1),
            cube([0.0; 3], SIDE),
            Some(unit),
            true,
            &blocks,
        )
        .unwrap();
    let level = &cells.levels[0];
    let mut expected: Vec<[u64; 3]> = coords.iter().map(|coord| coord.map(|v| v as u64)).collect();
    expected.sort_by_key(|coord| morton(*coord));
    assert_eq!(
        level
            .cells
            .iter()
            .map(|cell| cell.coord)
            .collect::<Vec<_>>(),
        expected
    );
    let duplicate = level
        .cells
        .iter()
        .find(|cell| cell.coord == [2097152, 0, 0])
        .unwrap();
    assert_eq!(
        (
            duplicate.count,
            duplicate.rgb_sum,
            duplicate.intensity_sum,
            duplicate.classification
        ),
        (2, [2, 4, 6], 7.0, -1)
    );
}

#[test]
fn subregions_keep_all_high_bits_at_maximum_depth() {
    let grid = CellGrid {
        root_min: [0.0; 3],
        root_side: 2.0_f64.powi(63),
    };
    let positions = [
        [2.0_f32.powi(62), 0.0, 0.0],
        [0.0, 2.0_f32.powi(62), 0.0],
        [0.0, 0.0, 2.0_f32.powi(62)],
        [2.0_f32.powi(61), 0.0, 0.0],
    ];
    let block = plain_block(
        positions
            .into_iter()
            .map(|p| Point::new(p, ColorRgb::new(1, 2, 3)))
            .collect(),
    );
    let cells = PointNodeCellSource::default()
        .node_cells(
            &grid,
            NodeId::new(1),
            cube([0.0; 3], grid.root_side),
            Some(1.0),
            true,
            &[block],
        )
        .unwrap();
    let coords: Vec<_> = cells.levels[0]
        .cells
        .iter()
        .map(|cell| cell.coord)
        .collect();
    assert_eq!(cells.levels[0].depth, 63);
    assert_eq!(
        coords,
        vec![
            [1_u64 << 61, 0, 0],
            [0, 0, 1_u64 << 62],
            [0, 1_u64 << 62, 0],
            [1_u64 << 62, 0, 0]
        ]
    );
}

#[test]
fn single_region_matches_generic_at_nonzero_node_origin() {
    let grid = CellGrid {
        root_min: [0.0; 3],
        root_side: 2.0_f64.powi(23),
    };
    let origin = 2_097_152_f32;
    let positions = [
        [origin, origin, origin],
        [origin + 2_097_151.0, origin, origin],
        [origin, origin + 2_097_151.0, origin],
        [origin, origin, origin + 2_097_151.0],
    ];
    let points: Vec<_> = positions
        .into_iter()
        .map(|p| Point::new(p, ColorRgb::new(1, 2, 3)))
        .collect();
    let shared = [
        plain_block(points.clone()),
        attributed_block(vec![points[0]], vec![7], vec![9]),
    ];
    let source = PointNodeCellSource::default();
    let build = |bounds, blocks: &[PointBlock]| {
        source
            .node_cells(&grid, NodeId::new(1), bounds, Some(1.0), true, blocks)
            .unwrap()
    };
    let narrow = build(cube([f64::from(origin); 3], 2.0_f64.powi(21)), &shared);
    let wide = build(cube([f64::from(origin); 3], 2.0_f64.powi(22)), &shared);
    assert_eq!(narrow, wide);
    let boundary = Point::new(
        [origin + 2_097_152.0, origin, origin],
        ColorRgb::new(4, 5, 6),
    );
    let mut with_boundary = shared.to_vec();
    with_boundary.push(plain_block(vec![boundary]));
    let mut wide_with_boundary = build(
        cube([f64::from(origin); 3], 2.0_f64.powi(22)),
        &with_boundary,
    )
    .levels
    .remove(0)
    .cells;
    wide_with_boundary.retain(|cell| cell.coord != [4_194_304, 2_097_152, 2_097_152]);
    assert_eq!(narrow.levels[0].cells, wide_with_boundary);
}
