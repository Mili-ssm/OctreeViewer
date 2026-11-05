//! Task 6: instance packing, page split, flags, frame, precision and the WGSL decode.

use std::collections::HashSet;

use glam::{DMat4, DVec3, dcamera};

use viewer_core::{HierarchySemantics, NodeId, PointBlock, SpatialBounds};
use wgpu::util::DeviceExt;

use crate::{
    CELL_PAGES_WGSL, CellGrid, CellInstance, FLAG_SPANS_OCTANTS, MAX_PAGE_EXTENT, NodeCell,
    NodeCellLevel, NodeCellSource, OrbitCamera, PAGE_HAS_CLASS, PAGE_HAS_INTENSITY, PAGE_SLOTS,
    PackedLevel, PageEntry, PageFrame, PointNodeCellSource, decode_cell,
    node_cells::node_range,
    pack_level, representative_depth,
    scene_digest_tests::{SIDE, fixture},
    srgb_to_linear_table,
};

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 11
    }
}

fn cell(coord: [u64; 3]) -> NodeCell {
    NodeCell {
        coord,
        rgb_sum: [10, 20, 30],
        intensity_sum: 0.0,
        intensity_count: 0,
        count: 1,
        classification: -1,
        z_min: 0.0,
        z_max: 0.0,
    }
}

fn level(depth: u8, cells: Vec<NodeCell>) -> NodeCellLevel {
    NodeCellLevel {
        depth,
        cells,
        octant_offsets: None,
    }
}

/// A node of side 1/16 at (3, 5, 2)/16 of a unit root: representative depth 7, node depth 4.
fn small_node() -> (CellGrid, SpatialBounds, u8) {
    let grid = CellGrid {
        root_min: [0.0; 3],
        root_side: 1.0,
    };
    let min = [3.0 / 16.0, 5.0 / 16.0, 2.0 / 16.0];
    let bounds = SpatialBounds::new(min, min.map(|v| v + 1.0 / 16.0)).unwrap();
    let node_depth = representative_depth(&grid, bounds) - 3;
    assert_eq!(node_depth, 4);
    (grid, bounds, node_depth)
}

/// The whole unit root (node depth 0), so every coordinate below 2^depth lies inside the node.
fn origin_node() -> (CellGrid, SpatialBounds) {
    let (grid, _, _) = small_node();
    (grid, root_bounds(&grid))
}

fn root_bounds(grid: &CellGrid) -> SpatialBounds {
    SpatialBounds::new(grid.root_min, grid.root_min.map(|v| v + grid.root_side)).unwrap()
}

fn pack(level: &NodeCellLevel) -> PackedLevel {
    let (grid, bounds) = origin_node();
    pack_level(&grid, bounds, NodeId::new(7), level)
}

/// Slot of cell `index` of the packed level.
fn slot_of(packed: &PackedLevel, index: usize) -> usize {
    let page = packed.pages.partition_point(|p| p.first_cell <= index) - 1;
    page * PAGE_SLOTS + (index - packed.pages[page].first_cell)
}

fn line(depth: u8, count: u64, axis: usize, start: u64) -> NodeCellLevel {
    level(
        depth,
        (0..count)
            .map(|i| {
                let mut coord = [0; 3];
                coord[axis] = start + i;
                cell(coord)
            })
            .collect(),
    )
}

/// Origin and box of every page from the cells themselves: per-axis minimum, and maximum minus
/// minimum, over the cells of `level.cells[first_cell..first_cell + cells]`.
fn assert_pages_match_their_cells(packed: &PackedLevel, level: &NodeCellLevel) {
    let mut next = 0;
    for page in &packed.pages {
        assert_eq!(page.first_cell, next, "pages tile the level in order");
        next += page.cells as usize;
        let cells = &level.cells[page.first_cell..next];
        for a in 0..3 {
            let min = cells.iter().map(|c| c.coord[a]).min().unwrap();
            let max = cells.iter().map(|c| c.coord[a]).max().unwrap();
            assert_eq!(page.origin[a], min, "page origin, axis {a}");
            assert_eq!(u64::from(page.box_max[a]), max - min, "page box, axis {a}");
        }
    }
    assert_eq!(next, level.cells.len());
}

fn assert_round_trip(packed: &PackedLevel, level: &NodeCellLevel) {
    assert_pages_match_their_cells(packed, level);
    assert_eq!(packed.instances.len(), packed.pages.len() * PAGE_SLOTS);
    let mut used = HashSet::new();
    for (index, cell) in level.cells.iter().enumerate() {
        let slot = slot_of(packed, index);
        assert!(used.insert(slot), "slot {slot} used twice");
        let instance = packed.instances[slot];
        assert!(!instance.is_empty());
        let page = &packed.pages[slot / PAGE_SLOTS];
        let rel = instance.rel();
        assert_eq!(
            std::array::from_fn(|a| page.origin[a] + u64::from(rel[a])),
            cell.coord
        );
        let n = u64::from(cell.count);
        let channel = |a: usize| ((cell.rgb_sum[a] + n / 2) / n) as u32;
        assert_eq!(
            instance.rgba(),
            channel(0) | channel(1) << 8 | channel(2) << 16 | 255 << 24
        );
        assert_eq!(instance.class(), cell.classification);
        let intensity = if cell.intensity_count == 0 {
            0xFFFF
        } else {
            let mean = cell.intensity_sum / f64::from(cell.intensity_count);
            if mean < 0.0 {
                0xFFFF
            } else {
                mean.round().min(65534.0) as u16
            }
        };
        assert_eq!(instance.intensity(), intensity);
        assert_eq!(instance.flags() & 0x8000, 0);
    }
    assert_eq!(used.len(), level.cells.len());
    for (slot, instance) in packed.instances.iter().enumerate() {
        if !used.contains(&slot) {
            assert_eq!(*instance, CellInstance::EMPTY);
        }
    }
}

#[test]
fn round_trip_keeps_every_cell_and_its_attributes() {
    let mut rng = Lcg(1);
    let intensities = [
        (0.0, 1),
        (7.0, 2),
        (70_000.0, 1),
        (-3.0, 1),
        (65_534.4, 1),
        (65_535.0, 1),
    ];
    let cells: Vec<NodeCell> = (0..1500_u64)
        .map(|i| {
            let mut c = cell([i % 40, rng.next() % 40, i / 40]);
            c.count = 1 + (rng.next() % 5) as u32;
            c.rgb_sum = [0, 1, 2].map(|_| rng.next() % (255 * u64::from(c.count) + 1));
            if i % 3 == 0 {
                let (sum, count) = intensities[(i / 3) as usize % intensities.len()];
                (c.intensity_sum, c.intensity_count) = (sum, count);
            }
            c.classification = [-1, 0, 5, i16::MIN, i16::MAX][(i % 5) as usize];
            c
        })
        .collect();
    let level = level(12, cells);
    let packed = pack(&level);
    assert_round_trip(&packed, &level);
    assert!(
        packed
            .pages
            .iter()
            .all(|p| p.flags == PAGE_HAS_INTENSITY | PAGE_HAS_CLASS)
    );
}

#[test]
fn real_fixture_levels_round_trip_with_octants_from_the_index() {
    let fixture = fixture(HierarchySemantics::Replacement);
    let grid = CellGrid {
        root_min: [0.0; 3],
        root_side: SIDE,
    };
    let (mut levels, mut octant_cells, mut spanning_levels) = (0, 0, 0);
    for id in (1..=9).map(NodeId::new) {
        let node = fixture.hierarchy.node(id).unwrap();
        let bounds = node.bounds();
        let local = SpatialBounds::new(
            std::array::from_fn(|a| bounds.min()[a] - crate::scene_digest_tests::ORIGIN[a]),
            std::array::from_fn(|a| bounds.max()[a] - crate::scene_digest_tests::ORIGIN[a]),
        )
        .unwrap();
        let blocks: Vec<PointBlock> = node
            .blocks()
            .iter()
            .map(|b| fixture.cache.peek(*b).unwrap().clone())
            .collect();
        let cells = PointNodeCellSource::default()
            .node_cells(
                &grid,
                id,
                local,
                node.spacing(),
                fixture.hierarchy.is_leaf(id),
                &blocks,
            )
            .unwrap();
        for level in &cells.levels {
            levels += 1;
            let packed = pack_level(&grid, local, id, level);
            assert_round_trip(&packed, level);
            // The octant derived from the mask index equals the library's octant offsets.
            for octant in 0..8 {
                for index in level.octant_range(octant) {
                    let instance = packed.instances[slot_of(&packed, index)];
                    assert_eq!(instance.spans_octants(), level.octant_offsets.is_none());
                    if level.octant_offsets.is_some() {
                        assert_eq!(
                            instance.octant() as usize,
                            octant,
                            "node {id:?} depth {}",
                            level.depth
                        );
                        octant_cells += 1;
                    }
                }
            }
            spanning_levels += usize::from(level.octant_offsets.is_none());
        }
    }
    assert!(
        levels >= 9 && octant_cells > 1000,
        "{levels} levels {octant_cells} cells"
    );
    println!(
        "# real fixture: {levels} levels, {spanning_levels} spanning, {octant_cells} octant cells"
    );
}

#[test]
fn pages_split_at_512_cells() {
    for (count, pages) in [(1, 1), (512, 1), (513, 2), (1025, 3)] {
        let level = line(12, count, 0, 0);
        let packed = pack(&level);
        assert_eq!(packed.pages.len(), pages, "{count} cells");
        assert_eq!(packed.empty_slots(), pages * PAGE_SLOTS - count as usize);
        assert_round_trip(&packed, &level);
    }
}

#[test]
fn pages_split_when_an_axis_range_passes_2_pow_16() {
    for axis in 0..3 {
        // Extent exactly 2^16: one page, relative 65535.
        let fits = line(20, 1, axis, 0);
        let mut cells = fits.cells;
        let mut far = cell([0; 3]);
        far.coord[axis] = MAX_PAGE_EXTENT - 1;
        cells.push(far);
        let ok = level(20, cells.clone());
        let packed = pack(&ok);
        assert_eq!(packed.pages.len(), 1);
        assert_eq!(packed.pages[0].box_max[axis], 65_535);
        assert_eq!(packed.instances[1].rel()[axis], 65_535);
        // One more: a second page whose origin is its own minimum.
        let mut past = cell([5; 3]);
        past.coord[axis] = MAX_PAGE_EXTENT;
        cells.push(past);
        let level = level(20, cells);
        let packed = pack(&level);
        assert_eq!(packed.pages.len(), 2);
        assert_eq!(packed.pages[1].origin[axis], MAX_PAGE_EXTENT);
        assert_eq!(packed.pages[1].first_cell, 2);
    }
}

#[test]
fn page_flags_are_per_level_and_never_split() {
    let mut cells = line(12, 1100, 1, 0).cells;
    cells[3].intensity_sum = 5.0;
    cells[3].intensity_count = 1;
    cells[900].classification = 2;
    let level = level(12, cells);
    let packed = pack(&level);
    assert_eq!(packed.pages.len(), 3);
    assert!(
        packed
            .pages
            .iter()
            .all(|p| p.flags == PAGE_HAS_INTENSITY | PAGE_HAS_CLASS)
    );
    // A cell without the attribute keeps its sentinel.
    assert_eq!(packed.instances[0].intensity(), 0xFFFF);
    assert_eq!(packed.instances[0].class(), -1);
    let none = pack(&line(12, 600, 1, 0));
    assert!(none.pages.iter().all(|p| p.flags == 0));
}

#[test]
fn flags_match_an_oracle_from_absolute_coordinates() {
    let (grid, bounds, node_depth) = small_node();
    let mut rng = Lcg(99);
    for extra in [1_u8, 2, 3, 8, 36] {
        let depth = node_depth + extra;
        let lo: [u64; 3] = std::array::from_fn(|a| node_range(&grid, bounds, depth, a).0);
        let width = 1_u64 << extra;
        let mut cells = vec![cell(lo), cell(lo.map(|v| v + width - 1))];
        for _ in 0..400 {
            cells.push(cell(std::array::from_fn(|a| lo[a] + rng.next() % width)));
        }
        let level = level(depth, cells);
        let packed = pack_level(&grid, bounds, NodeId::new(1), &level);
        for (index, cell) in level.cells.iter().enumerate() {
            let instance = packed.instances[slot_of(&packed, index)];
            // Mask cell = floor(8 * relative / node width), clamped to 7.
            let mask: [u32; 3] = std::array::from_fn(|a| {
                ((u128::from(cell.coord[a] - lo[a]) * 8) >> extra).min(7) as u32
            });
            assert_eq!(
                instance.mask_index(),
                mask[0] + 8 * mask[1] + 64 * mask[2],
                "+{extra}"
            );
            assert_eq!(
                instance.octant(),
                4 * (mask[0] >> 2) + 2 * (mask[1] >> 2) + (mask[2] >> 2)
            );
            assert!(!instance.spans_octants());
            assert_eq!(instance.flags() & 0xFC00, 0);
        }
    }
    // Levels at or above the node depth span octants; their mask index is 0.
    for depth in [node_depth, node_depth - 2, 0] {
        let lo: [u64; 3] = std::array::from_fn(|a| node_range(&grid, bounds, depth, a).0);
        let level = level(depth, vec![cell(lo)]);
        let packed = pack_level(&grid, bounds, NodeId::new(1), &level);
        let instance = packed.instances[0];
        assert!(instance.spans_octants());
        assert_eq!(
            u32::from(instance.flags()) & FLAG_SPANS_OCTANTS,
            FLAG_SPANS_OCTANTS
        );
        // The node lies inside one cell: relative 0, mask index 0.
        assert_eq!(instance.mask_index(), 0);
    }
}

#[test]
fn layout_literals_are_fixed() {
    assert_eq!(CellInstance::EMPTY.0, [0xFFFF_FFFF, 0xFFFF_FFFF, 0, 0]);
    assert_eq!((PAGE_HAS_INTENSITY, PAGE_HAS_CLASS), (1, 2));
    assert_eq!(size_of::<CellInstance>(), 16);
    assert_eq!(size_of::<PageEntry>(), 32);
    let mut cells = line(12, 3, 0, 0).cells;
    cells[1].intensity_sum = 9.0;
    cells[1].intensity_count = 1;
    cells[2].classification = 6;
    let packed = pack(&level(12, cells));
    let (grid, _) = origin_node();
    let frame = PageFrame {
        grid,
        coordinate_origin: [0.0; 3],
        visual_scale: 1.0,
        eye_scaled: [0.0; 3],
    };
    let entry = packed.pages[0].entry(&frame, 11, 5);
    assert_eq!(entry.0[7] >> 16, 3, "page flags in the high half of word 7");
    assert_eq!(entry.0[7] & 0xFFFF, 0, "box z");
    assert_eq!(entry.0[6], 2, "box x | y << 16");
    assert_eq!((entry.0[4], entry.0[5]), (11, 5));
}

#[test]
fn intensity_and_color_edge_cases() {
    let mut cells = line(12, 8, 0, 0).cells;
    let means = [
        (f64::NAN, 1),
        (f64::INFINITY, 1),
        (f64::NEG_INFINITY, 1),
        (1.0e30, 1),
        (-0.4, 1),
        (0.4, 1),
        (65_534.5, 1),
        (3.0, 0),
    ];
    for (cell, (sum, count)) in cells.iter_mut().zip(means) {
        (cell.intensity_sum, cell.intensity_count) = (sum, count);
    }
    cells[0].rgb_sum = [1000, 255, 0];
    let packed = pack(&level(12, cells));
    let intensities: Vec<u16> = packed.instances[..8]
        .iter()
        .map(|i| i.intensity())
        .collect();
    // NaN, +-inf and a negative mean are missing; a huge finite mean clamps to 65534.
    assert_eq!(
        intensities,
        [0xFFFF, 0xFFFF, 0xFFFF, 65_534, 0xFFFF, 0, 65_534, 0xFFFF]
    );
    // A channel sum above 255 per point (malformed input) clamps instead of spilling into g.
    assert_eq!(packed.instances[0].rgba(), 255 | 255 << 8 | 255 << 24);
}

// ---- frame and precision -------------------------------------------------------------------

fn ulp(value: f32) -> f64 {
    let v = value.abs();
    f64::from(f32::from_bits(v.to_bits() + 1) - v)
}

/// The box that `scene.rs`'s vertex shader draws for an `InstanceRaw` of this cell with
/// `style.y = 1` and `render_origin.w = 1`: `center - anchor +- size / 2`, in f64 (`anchor` is the
/// `render_origin`: the target for the scene, the eye for pages).
fn scene_box(
    frame: &PageFrame,
    anchor: [f64; 3],
    depth: u8,
    coord: [u64; 3],
) -> ([f64; 3], [f64; 3]) {
    let side = frame.grid.root_side / (1_u64 << depth) as f64;
    let size = side * frame.visual_scale;
    let lo = std::array::from_fn(|a| {
        let center = (frame.grid.root_min[a] + (coord[a] as f64 + 0.5) * side
            - frame.coordinate_origin[a])
            * frame.visual_scale;
        center - anchor[a] - size * 0.5
    });
    (lo, std::array::from_fn(|a| lo[a] + size))
}

/// Decodes every cell of the level and returns (lo, hi) as f32 pairs next to the exact box and the
/// ulp bound `4 ulp(max(|origin|, (box + 1) side))`.
fn check_against_scene_box(
    frame: &PageFrame,
    target: [f64; 3],
    level: &NodeCellLevel,
    packed: &PackedLevel,
) -> f64 {
    let shift: [f64; 3] = std::array::from_fn(|a| frame.eye_scaled[a] - target[a]);
    let table = srgb_to_linear_table();
    let mut worst = 0.0_f64;
    for (index, cell) in level.cells.iter().enumerate() {
        let slot = slot_of(packed, index);
        let page = &packed.pages[slot / PAGE_SLOTS];
        let entry = page.entry(frame, 0, 0);
        let decoded = decode_cell(&entry, &packed.instances[slot], &table);
        let (lo, hi) = scene_box(frame, target, level.depth, cell.coord);
        let side = entry.side();
        for a in 0..3 {
            let reach = (f32::from(page.box_max[a]) + 1.0) * side;
            let bound = 4.0 * ulp(entry.origin()[a].abs().max(reach));
            for (got, want) in [(decoded.lo[a], lo[a]), (decoded.hi[a], hi[a])] {
                // Pages are eye-relative; the scene box is relative to the target.
                let error = (f64::from(got) + shift[a] - want).abs();
                assert!(error <= bound, "axis {a} error {error} > {bound}");
                worst = worst.max(error / bound);
            }
        }
    }
    worst
}

fn frame_at_1e6() -> PageFrame {
    PageFrame {
        grid: CellGrid {
            root_min: [1_000_000.37, 1_000_000.11, 1_000_050.9],
            root_side: 4096.0,
        },
        coordinate_origin: [1_000_000.0, 1_000_000.0, 1_000_000.0],
        visual_scale: 0.0173,
        eye_scaled: [30.123_456_789, 12.5, 7.0625],
    }
}

#[test]
fn frame_matches_the_scene_formula_for_the_same_cells() {
    let frame = PageFrame {
        grid: CellGrid {
            root_min: [10.0, -20.0, 5.0],
            root_side: 64.0,
        },
        coordinate_origin: [4.0, -3.0, 1.0],
        visual_scale: 1.0,
        eye_scaled: [8.0, -1.5, 0.25],
    };
    let mut rng = Lcg(5);
    let cells = (0..800)
        .map(|_| cell(std::array::from_fn(|_| rng.next() % 4000)))
        .collect();
    let level = level(12, cells);
    let packed = pack(&level);
    check_against_scene_box(&frame, [-2.0, 40.5, 3.25], &level, &packed);
}

#[test]
fn precision_at_1e6_with_a_visual_scale_stays_within_4_ulp() {
    let frame = frame_at_1e6();
    let mut rng = Lcg(6);
    // Root at level 0: cells of the level 20 spread over the root, pages as wide as allowed.
    let mut cells: Vec<NodeCell> = (0..3000)
        .map(|_| cell(std::array::from_fn(|_| rng.next() % (1 << 20))))
        .collect();
    cells.extend((0..600).map(|i| cell([65_535 * (i % 2), 65_535 * (i / 2 % 2), 17 * i])));
    cells.sort_by_key(|c| c.coord);
    let level = level(20, cells);
    let packed = pack(&level);
    let target = [
        frame.eye_scaled[0] + 4.43,
        frame.eye_scaled[1],
        frame.eye_scaled[2],
    ];
    let worst = check_against_scene_box(&frame, target, &level, &packed);
    println!("# worst error / 4 ulp bound at 1e6: {worst:.3}");
}

/// Datasets of the Task 4 export: (root side, visual scale, nearest drawn depth in metres).
/// Scales and root sides are those of the `inside-leaf` case. The nearest depth along the view
/// axis is a bit below the smallest eye-to-cell distance of its 4 px export (8.6 m for
/// Belvedere, 0.14 m for Majadas), because the patches also reach the screen edges.
const BELVEDERE: (f64, f64, f64) = (16384.0, 0.016_492_016_214_950_24, 8.0);
const MAJADAS: (f64, f64, f64) = (2048.0, 0.078_826_867_034_050_9, 0.1);

/// The camera's view-projection with the eye at the origin, built in f64 and cast to f32 (as the
/// renderer will upload it): the camera's own projection (recovered from its target-origin
/// matrix) times a `look_at` from the origin towards `target - eye` with the camera's up.
fn eye_view_projection(camera: &OrbitCamera, viewport: [u32; 2]) -> DMat4 {
    let (eye, target) = (DVec3::from(camera.position()), DVec3::from(camera.target()));
    let target_origin = DMat4::from_cols_array_2d(
        &camera
            .frame(viewport)
            .unwrap()
            .view_projection
            .map(|c| c.map(f64::from)),
    );
    let offset = eye - target;
    let view_at_target = dcamera::rh::view::look_at_mat4(offset, DVec3::ZERO, DVec3::Z);
    let projection = target_origin * view_at_target.inverse();
    let view_at_eye = dcamera::rh::view::look_at_mat4(DVec3::ZERO, -offset, DVec3::Z);
    (projection * view_at_eye).as_mat4().as_dmat4()
}

struct PxReport {
    worst: f64,
    at: f64,
    samples: usize,
    skipped: usize,
}

/// Largest pixel error, over cells that project to about 4 px at distances from `nearest` metres
/// up, of the decoded `lo`/`hi` against the exact corner, in the eye-relative frame, with a
/// target at `root_side / 16` from an eye inside the deepest leaf.
fn inside_leaf_error_px((root_side, scale, nearest): (f64, f64, f64)) -> PxReport {
    let grid = CellGrid {
        root_min: [1_000_000.37, 1_000_000.11, 1_000_050.9],
        root_side,
    };
    let node = root_bounds(&grid);
    let frame = PageFrame {
        grid,
        coordinate_origin: [1_000_000.0, 1_000_000.0, 1_000_000.0],
        visual_scale: scale,
        eye_scaled: [0.0; 3],
    };
    let viewport = [1280_u32, 720];
    let root = grid.root_min;
    let eye_dataset = [
        root[0] + root_side * 0.37,
        root[1] + root_side * 0.41,
        root[2] + root_side * 0.05,
    ];
    let eye_scaled: [f64; 3] =
        std::array::from_fn(|a| (eye_dataset[a] - frame.coordinate_origin[a]) * frame.visual_scale);
    let forward = [1.0, 0.0, 0.0_f64];
    // Target at root_side / 16 (scaled) from the eye, like the Task 4 inside-leaf pose.
    let distance = root_side * scale / 16.0;
    let mut camera = OrbitCamera::default();
    assert!(camera.set_pose(
        eye_scaled,
        std::array::from_fn(|a| eye_scaled[a] + distance * forward[a])
    ));
    let eye = camera.position();
    let frame = PageFrame {
        eye_scaled: eye,
        ..frame
    };
    let matrix = eye_view_projection(&camera, viewport);
    let project = |p: [f64; 3]| {
        let clip = matrix * DVec3::from(p).extend(1.0);
        (clip.w > 0.0).then(|| [clip.x / clip.w, clip.y / clip.w])
    };
    let focal = matrix.row(1).truncate().length() * f64::from(viewport[1]) / 2.0;
    let table = srgb_to_linear_table();
    let mut report = PxReport {
        worst: 0.0,
        at: nearest,
        samples: 0,
        skipped: 0,
    };
    // Depth along the view axis, and lateral offsets that reach the screen edges.
    let patches = (0..8).flat_map(|i| {
        [-0.7, 0.0, 0.7].into_iter().flat_map(move |ly| {
            [-0.4, 0.0, 0.4].map(move |lz| (nearest * f64::from(1_u32 << i), ly, lz))
        })
    });
    for (meters, ly, lz) in patches {
        let wanted = 4.0 * meters / focal;
        let depth = (root_side / wanted).log2().floor().clamp(0.0, 24.0) as u8;
        let side = root_side / (1_u64 << depth) as f64;
        let point: [f64; 3] =
            std::array::from_fn(|a| eye_dataset[a] + meters * (forward[a] + [0.0, ly, lz][a]));
        let base: [u64; 3] = std::array::from_fn(|a| ((point[a] - root[a]) / side).floor() as u64);
        let mut cells = Vec::new();
        for (dx, dy, dz) in (0..512).map(|i| (i % 8, i / 8 % 8, i / 64)) {
            cells.push(cell([base[0] + dx - 4, base[1] + dy - 4, base[2] + dz - 4]));
        }
        let level = level(depth, cells);
        let packed = pack_level(&grid, node, NodeId::new(1), &level);
        for (index, cell) in level.cells.iter().enumerate() {
            let slot = slot_of(&packed, index);
            let page = &packed.pages[slot / PAGE_SLOTS];
            let decoded = decode_cell(&page.entry(&frame, 0, 0), &packed.instances[slot], &table);
            let (lo, hi) = scene_box(&frame, eye, depth, cell.coord);
            for (got, exact) in [(decoded.lo, lo), (decoded.hi, hi)] {
                assert!(
                    got.iter().all(|v| v.is_finite()),
                    "non-finite corner {got:?}"
                );
                let (Some(a), Some(b)) = (project(exact), project(got.map(f64::from))) else {
                    report.skipped += 1;
                    continue;
                };
                if a[0].abs() > 1.5 || a[1].abs() > 1.5 {
                    report.skipped += 1;
                    continue;
                }
                let px = ((a[0] - b[0]) * 640.0).hypot((a[1] - b[1]) * 360.0);
                if px > report.worst {
                    (report.worst, report.at) = (px, meters);
                }
                report.samples += 1;
            }
        }
    }
    report
}

fn assert_inside_leaf(name: &str, dataset: (f64, f64, f64)) {
    let r = inside_leaf_error_px(dataset);
    println!(
        "# {name} inside-leaf max error {:.5} px at {:.2} m ({} corners, {} skipped)",
        r.worst, r.at, r.samples, r.skipped
    );
    assert!(r.samples > 1000, "only {} corners projected", r.samples);
    assert!(r.worst <= 0.01, "max error {} px at {} m", r.worst, r.at);
}

#[test]
fn inside_leaf_pose_in_the_eye_frame_stays_within_a_hundredth_of_a_pixel() {
    assert_inside_leaf("belvedere", BELVEDERE);
}

#[test]
fn inside_leaf_pose_of_majadas_in_the_eye_frame_stays_within_a_hundredth_of_a_pixel() {
    assert_inside_leaf("majadas", MAJADAS);
}

// ---- WGSL ----------------------------------------------------------------------------------

#[test]
fn cell_pages_wgsl_validates_with_naga() {
    let module = naga::front::wgsl::parse_str(CELL_PAGES_WGSL).expect("valid WGSL");
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .expect("valid module");
}

const KERNEL: &str = r"
@group(0) @binding(1) var<storage, read> pages: array<CellPage>;
@group(0) @binding(2) var<storage, read> cells: array<vec4<u32>>;
@group(0) @binding(3) var<storage, read_write> decoded: array<u32>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    if i >= arrayLength(&cells) { return; }
    let cell = cells[i];
    let page = pages[i / 512u];
    let rel = cell_rel(cell);
    let lo = cell_lo(page, cell);
    let hi = cell_hi(page, cell);
    let rgb = cell_linear_rgb(cell);
    let base = i * 24u;
    decoded[base] = rel.x;
    decoded[base + 1u] = rel.y;
    decoded[base + 2u] = rel.z;
    decoded[base + 3u] = cell_flags(cell);
    decoded[base + 4u] = cell_mask_index(cell);
    decoded[base + 5u] = cell_octant(cell);
    decoded[base + 6u] = select(0u, 1u, cell_spans_octants(cell));
    decoded[base + 7u] = cell_intensity(cell);
    decoded[base + 8u] = bitcast<u32>(cell_class(cell));
    decoded[base + 9u] = select(0u, 1u, cell_is_empty(cell));
    decoded[base + 10u] = bitcast<u32>(lo.x);
    decoded[base + 11u] = bitcast<u32>(lo.y);
    decoded[base + 12u] = bitcast<u32>(lo.z);
    decoded[base + 13u] = bitcast<u32>(hi.x);
    decoded[base + 14u] = bitcast<u32>(hi.y);
    decoded[base + 15u] = bitcast<u32>(hi.z);
    decoded[base + 16u] = bitcast<u32>(rgb.x);
    decoded[base + 17u] = bitcast<u32>(rgb.y);
    decoded[base + 18u] = bitcast<u32>(rgb.z);
}
";
const WORDS_PER_CELL: usize = 24;

/// Instances with the extremes plus pseudo-random cells, split over many pages.
fn gpu_input() -> (Vec<CellInstance>, Vec<PageEntry>) {
    let (grid, bounds) = origin_node();
    let node_depth = 0;
    let frame = PageFrame {
        grid: CellGrid {
            root_min: [1_000_000.37, 999_999.5, 1_000_050.9],
            root_side: 4096.0,
        },
        coordinate_origin: [1_000_000.0; 3],
        visual_scale: 0.0173,
        eye_scaled: [30.123_456_789, 12.5, 7.0625],
    };
    let mut rng = Lcg(77);
    let mut extremes = vec![
        cell([0, 0, 0]),
        cell([65_535, 65_535, 65_535]),
        cell([65_535, 0, 0]),
        cell([0, 65_535, 0]),
        cell([0, 0, 65_535]),
    ];
    let attributes = [
        (-1, 0.0, 1),
        (i16::MIN, 65_534.0, 1),
        (i16::MAX, 70_000.0, 1),
        (0, -1.0, 1),
        (-1, 0.0, 0),
    ];
    for (i, (class, sum, count)) in attributes.into_iter().enumerate() {
        extremes[i].classification = class;
        extremes[i].intensity_sum = sum;
        extremes[i].intensity_count = count;
        extremes[i].rgb_sum = [255 * (i as u64 % 2), 255, i as u64 * 60];
    }
    let mut instances = Vec::new();
    let mut pages = Vec::new();
    let mut add = |level: NodeCellLevel, instances: &mut Vec<CellInstance>| {
        let packed = pack_level(&grid, bounds, NodeId::new(3), &level);
        for page in &packed.pages {
            let index = pages.len() as u32;
            pages.push(page.entry(&frame, index, index & 7));
        }
        instances.extend(packed.instances);
    };
    add(level(19, extremes), &mut instances);
    for depth in [17, 19, 20, 25, 30] {
        let span = 1_u64 << (depth - node_depth).min(40);
        let cells = (0..2500)
            .map(|_| {
                let mut c = cell(std::array::from_fn(|_| rng.next() % span.min(60_000)));
                c.count = 1 + (rng.next() % 9) as u32;
                c.rgb_sum = [0, 1, 2].map(|_| rng.next() % (255 * u64::from(c.count) + 1));
                c.classification = (rng.next() % 70_000) as i64 as i16;
                if rng.next() % 3 != 0 {
                    c.intensity_count = 1 + (rng.next() % 3) as u32;
                    c.intensity_sum = (rng.next() % 140_000) as f64 / 1.7 - 1000.0;
                }
                c
            })
            .collect();
        add(level(depth, cells), &mut instances);
    }
    assert!(instances.iter().filter(|i| !i.is_empty()).count() >= 10_000);
    (instances, pages)
}

fn rust_decode(instances: &[CellInstance], pages: &[PageEntry]) -> Vec<u32> {
    let table = srgb_to_linear_table();
    let mut out = Vec::with_capacity(instances.len() * WORDS_PER_CELL);
    for (slot, instance) in instances.iter().enumerate() {
        let d = decode_cell(&pages[slot / PAGE_SLOTS], instance, &table);
        let mut words = [0_u32; WORDS_PER_CELL];
        words[..3].copy_from_slice(&d.rel.map(u32::from));
        words[3] = u32::from(d.flags);
        words[4] = d.mask_index;
        words[5] = d.octant;
        words[6] = u32::from(d.spans_octants);
        words[7] = u32::from(d.intensity);
        words[8] = d.class as i32 as u32;
        words[9] = u32::from(instance.is_empty());
        for a in 0..3 {
            words[10 + a] = d.lo[a].to_bits();
            words[13 + a] = d.hi[a].to_bits();
            words[16 + a] = d.linear_rgb[a].to_bits();
        }
        out.extend(words);
    }
    out
}

fn gpu_decode(
    adapter: &wgpu::Adapter,
    instances: &[CellInstance],
    pages: &[PageEntry],
) -> Result<Vec<u32>, String> {
    pollster::block_on(async {
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("cell-pages-test"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_defaults()
                    .using_resolution(adapter.limits()),
                memory_hints: wgpu::MemoryHints::MemoryUsage,
                ..Default::default()
            })
            .await
            .map_err(|e| format!("device: {e}"))?;
        let source = format!("{CELL_PAGES_WGSL}\n{KERNEL}");
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cell-pages-test"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let init = |label, contents: &[u8], usage| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents,
                usage,
            })
        };
        let table = srgb_to_linear_table();
        let table_buffer = init(
            "table",
            bytemuck::bytes_of(&table),
            wgpu::BufferUsages::UNIFORM,
        );
        let pages_buffer = init(
            "pages",
            bytemuck::cast_slice(pages),
            wgpu::BufferUsages::STORAGE,
        );
        let cells_buffer = init(
            "cells",
            bytemuck::cast_slice(instances),
            wgpu::BufferUsages::STORAGE,
        );
        let size = (instances.len() * WORDS_PER_CELL * 4) as u64;
        let out = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("decoded"),
            size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("staging"),
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("cell-pages-test"),
            layout: None,
            module: &module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: table_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: pages_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: cells_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: out.as_entire_binding(),
                },
            ],
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(instances.len().div_ceil(64) as u32, 1, 1);
        }
        encoder.copy_buffer_to_buffer(&out, 0, &staging, 0, size);
        queue.submit([encoder.finish()]);
        let (sender, receiver) = std::sync::mpsc::channel();
        staging.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = sender.send(r);
        });
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| format!("poll: {e}"))?;
        receiver
            .recv()
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        let view = staging
            .slice(..)
            .get_mapped_range()
            .map_err(|e| e.to_string())?;
        let words = bytemuck::cast_slice::<u8, u32>(&view).to_vec();
        Ok(words)
    })
}

#[test]
fn gpu_decode_is_bit_identical_to_rust_on_every_adapter() {
    let (instances, pages) = gpu_input();
    let expected = rust_decode(&instances, &pages);
    let instance = wgpu::Instance::default();
    let mut adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
    if let Ok(fallback) =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: true,
            ..Default::default()
        }))
        && !adapters.iter().any(|a| a.get_info() == fallback.get_info())
    {
        adapters.push(fallback);
    }
    let real = |a: &wgpu::Adapter| a.get_info().device_type != wgpu::DeviceType::Cpu;
    adapters.sort_by_key(|a| !real(a));
    let mut real_ran = 0;
    for adapter in &adapters {
        let info = adapter.get_info();
        let label = format!("{} ({:?}, {:?})", info.name, info.backend, info.device_type);
        match gpu_decode(adapter, &instances, &pages) {
            Ok(got) => {
                assert_eq!(got.len(), expected.len());
                let bad = got.iter().zip(&expected).position(|(g, e)| g != e);
                if let Some(at) = bad {
                    panic!(
                        "{label}: word {} of slot {} differs: gpu {:#x} rust {:#x}",
                        at % WORDS_PER_CELL,
                        at / WORDS_PER_CELL,
                        got[at],
                        expected[at]
                    );
                }
                println!("# adapter {label}: {} slots bit-identical", instances.len());
                real_ran += usize::from(real(adapter));
            }
            Err(error) => println!("# adapter {label}: skipped ({error})"),
        }
    }
    if real_ran == 0 {
        println!("# no real adapter ran the decode test");
        assert!(
            std::env::var("CELL_PAGES_REQUIRE_GPU").as_deref() != Ok("1"),
            "CELL_PAGES_REQUIRE_GPU=1 but no real adapter ran"
        );
    }
}
