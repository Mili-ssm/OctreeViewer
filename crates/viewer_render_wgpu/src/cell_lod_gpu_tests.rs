//! Task 7a: the per-cell LOD decision of the slot pass against its Rust twin (`cell_lod`).
//!
//! The scenes put cells of several active depths of one node over the same points, so the
//! all-ancestors rule, the masks and their mips all matter. The GPU result is the list of drawn
//! slot ids of the plain compaction, read back from the `visible` buffer; every cell whose
//! decision does not hang on a float within `THRESHOLD_BAND` of the target must agree with
//! `cell_lod::draws`. The scene, pose and readback helpers are shared with the larger partition
//! test of Task 7b, which is the kill gate of phase 2: the drawn cells of a node must partition its
//! unmasked points (see `cell_mode_draws_one_cell_per_point_on_every_adapter`).

use super::*;
use crate::cell_lod::{
    LodView, NodeLod, cell_lo, draws, edge_px, masked_any, node_record, refines,
};
use crate::representative_depth;

/// A cell is skipped when `edge_px / target_px` of it or of an ancestor is this close to 1.
pub(super) const THRESHOLD_BAND: f32 = 1.0e-4;
/// Frustum margins around the shader's `1e-4` that settle whether a cell is in or out.
const FRUSTUM_MARGINS: [f32; 2] = [0.9e-4, 1.1e-4];

/// One node: its cells at the active depths (offsets below the node depth), mask and corner.
pub(super) struct LodNodeScene {
    pub grid: CellGrid,
    pub bounds: SpatialBounds,
    pub node_depth: u8,
    pub active: u64,
    pub mask: [u64; 8],
    /// Level `i` is the depth `node_depth + offsets[i]`.
    pub offsets: Vec<u8>,
    pub levels: Vec<NodeCellLevel>,
    node_lo: [u64; 3],
}

impl LodNodeScene {
    /// `points` random fine cells; every active depth gets the cell that contains each of them,
    /// so the depths nest. `mask_percent` of the 512 mask bits are set.
    pub(super) fn random(
        rng: &mut Rng,
        grid: CellGrid,
        node_depth: u8,
        node_lo: [u64; 3],
        offsets: &[u8],
        mask_percent: u64,
        points: usize,
    ) -> Self {
        let node_side = grid.root_side / (1_u64 << node_depth) as f64;
        let min: [f64; 3] =
            std::array::from_fn(|a| grid.root_min[a] + node_lo[a] as f64 * node_side);
        let bounds = SpatialBounds::new(min, min.map(|v| v + node_side)).unwrap();
        assert_eq!(representative_depth(&grid, bounds), node_depth + 3);
        let deepest = *offsets.iter().max().unwrap();
        let fine: Vec<[u64; 3]> = (0..points)
            .map(|_| std::array::from_fn(|_| rng.below(1 << deepest)))
            .collect();
        let levels = offsets
            .iter()
            .map(|&offset| {
                let mut seen = HashSet::new();
                let mut coords: Vec<[u64; 3]> = fine
                    .iter()
                    .map(|p| {
                        std::array::from_fn(|a| {
                            (node_lo[a] << offset) + (p[a] >> (deepest - offset))
                        })
                    })
                    .filter(|coord| seen.insert(*coord))
                    .collect();
                coords.sort_by_key(|coord| morton(*coord));
                NodeCellLevel {
                    depth: node_depth + offset,
                    cells: coords.into_iter().map(|coord| cell(coord, 0)).collect(),
                    octant_offsets: None,
                }
            })
            .collect();
        let mut mask = [0_u64; 8];
        for bit in 0..512 {
            if rng.below(100) < mask_percent {
                mask[bit / 64] |= 1 << (bit % 64);
            }
        }
        Self {
            grid,
            bounds,
            node_depth,
            active: offsets.iter().fold(0, |bits, o| bits | 1 << o),
            mask,
            offsets: offsets.to_vec(),
            levels,
            node_lo,
        }
    }

    /// Cell `coord` of the level `index` relative to the node corner.
    fn rel(&self, index: usize, coord: [u64; 3]) -> [u32; 3] {
        let offset = self.offsets[index];
        std::array::from_fn(|a| (coord[a] - (self.node_lo[a] << offset)) as u32)
    }

    fn corner(&self) -> [f64; 3] {
        self.bounds.min()
    }
}

/// A level placed in the pool.
pub(super) struct LodPlaced {
    pub node: usize,
    pub level: usize,
    pub packed: PackedLevel,
    pub slots: Vec<u32>,
}

pub(super) struct LodPool {
    pub placed: Vec<LodPlaced>,
    /// Mask slot (= node record) of each node.
    pub node_slots: Vec<u32>,
}

/// Uploads every level of `nodes` into `pool` and gives each node a record slot.
pub(super) fn place_lod(pool: &mut CellRenderer, nodes: &[LodNodeScene]) -> LodPool {
    let mut next_slot = 0..;
    let mut placed = Vec::new();
    let mut node_slots = Vec::new();
    for (node_index, node) in nodes.iter().enumerate() {
        node_slots.push(pool.alloc_mask_slot().expect("mask slot"));
        for (level_index, level) in node.levels.iter().enumerate() {
            let packed = pack_level(
                &node.grid,
                node.bounds,
                NodeId::new(node_index as u64 + 1),
                level,
            );
            assert!(
                packed.pages.iter().all(|p| p.rel_origin[0] != u32::MAX),
                "pages of cell mode need a relative origin"
            );
            let slots: Vec<u32> = next_slot.by_ref().take(packed.pages.len()).collect();
            assert_eq!(slots.len(), packed.pages.len());
            pool.upload_level(&packed, &slots).unwrap();
            placed.push(LodPlaced {
                node: node_index,
                level: level_index,
                packed,
                slots,
            });
        }
    }
    LodPool { placed, node_slots }
}

/// The node of the twin for a pose of the eye at `eye`.
pub(super) fn node_lods(nodes: &[LodNodeScene], eye: [f64; 3]) -> Vec<NodeLod> {
    nodes
        .iter()
        .map(|node| NodeLod {
            anchor: std::array::from_fn(|a| (node.corner()[a] - eye[a]) as f32),
            node_depth: node.node_depth,
            active: node.active,
            mask: node.mask,
        })
        .collect()
}

/// The page entry of page `page` of `placed` for an eye at `eye`.
fn page_entry(
    nodes: &[LodNodeScene],
    pool: &LodPool,
    placed: &LodPlaced,
    page: usize,
    eye: [f64; 3],
) -> PageEntry {
    let node = &nodes[placed.node];
    let frame = PageFrame {
        grid: node.grid,
        coordinate_origin: [0.0; 3],
        visual_scale: 1.0,
        eye_scaled: eye,
    };
    placed.packed.pages[page].entry(&frame, pool.node_slots[placed.node], 0xff)
}

/// Writes the node records and the page entries for `camera`, switches the pool to cell mode with
/// `view`, and returns the twin nodes.
pub(super) fn set_pose(
    pool: &mut CellRenderer,
    nodes: &[LodNodeScene],
    placed: &LodPool,
    camera: &OrbitCamera,
    view: LodView,
) -> Vec<NodeLod> {
    let eye = camera.position();
    let lods = node_lods(nodes, eye);
    for (slot, lod) in placed.node_slots.iter().zip(&lods) {
        pool.set_node_record(*slot, &node_record(lod)).unwrap();
    }
    let mut entries = Vec::new();
    for level in &placed.placed {
        for page in 0..level.packed.pages.len() {
            entries.push((
                level.slots[page],
                page_entry(nodes, placed, level, page, eye),
            ));
        }
    }
    pool.set_active_pages(&entries).unwrap();
    pool.set_lod_view(Some(view));
    lods
}

/// Slot ids (`page slot * PAGE_SLOTS + local`) the last compaction drew; `count_word` is word 1
/// of the draw arguments for the plain pass or phase 1, word 6 for phase 2 (the list is shared,
/// so with occlusion only the last phase is left).
pub(super) fn read_drawn_slots(h: &Harness, pool: &CellRenderer, count_word: usize) -> Vec<u32> {
    let staging = |size| {
        h.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("lod-test-staging"),
            size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        })
    };
    let count_out = staging(28);
    let mut encoder = h.device.create_command_encoder(&Default::default());
    encoder.copy_buffer_to_buffer(pool.draw_args_buffer(), 0, &count_out, 0, 28);
    h.queue.submit([encoder.finish()]);
    let words: Vec<u32> = bytemuck::pod_collect_to_vec(&read_buffer(h, &count_out));
    let count = u64::from(words[count_word]);
    if count == 0 {
        return Vec::new();
    }
    let slots_out = staging(count * 4);
    let mut encoder = h.device.create_command_encoder(&Default::default());
    encoder.copy_buffer_to_buffer(pool.visible_buffer(), 0, &slots_out, 0, count * 4);
    h.queue.submit([encoder.finish()]);
    bytemuck::pod_collect_to_vec(&read_buffer(h, &slots_out))
}

/// A random pose around the scene `[lo, hi]`, with a random target of the cell edge in pixels.
pub(super) fn random_pose(
    rng: &mut Rng,
    bounds: ([f64; 3], [f64; 3]),
    target_px: (f32, f32),
) -> Option<(OrbitCamera, LodView)> {
    let unit = |rng: &mut Rng| rng.below(1 << 24) as f64 / (1_u64 << 24) as f64;
    let target: [f64; 3] =
        std::array::from_fn(|a| bounds.0[a] + (bounds.1[a] - bounds.0[a]) * unit(rng));
    let direction: [f64; 3] = std::array::from_fn(|_| unit(rng) * 2.0 - 1.0);
    let length = direction.iter().map(|v| v * v).sum::<f64>().sqrt();
    if length < 0.1 {
        return None;
    }
    let distance = 0.02 * 150.0_f64.powf(unit(rng));
    let position = std::array::from_fn(|a| target[a] + direction[a] / length * distance);
    let mut camera = test_camera();
    if !camera.set_pose(position, target) {
        return None;
    }
    camera.eye_frame([WIDTH, HEIGHT])?;
    let target_px = target_px.0 + (target_px.1 - target_px.0) * unit(rng) as f32;
    Some((
        camera,
        LodView::from_camera(&camera, [WIDTH as f32, HEIGHT as f32], target_px, 0.0),
    ))
}

#[derive(Debug, Default)]
pub(super) struct TwinReport {
    pub compared: usize,
    pub drawn: usize,
    pub near_threshold: usize,
    pub near_frustum: usize,
    pub mismatch_count: usize,
    /// The first few, for the failure message.
    pub mismatches: Vec<String>,
}

/// Compares the GPU slots with the twin for every cell of every placed level.
pub(super) fn compare_with_twin(
    nodes: &[LodNodeScene],
    pool: &LodPool,
    lods: &[NodeLod],
    camera: &OrbitCamera,
    view: &LodView,
    gpu_slots: &[u32],
    report: &mut TwinReport,
) {
    let table = srgb_to_linear_table();
    let eye = camera.position();
    let matrix = camera.eye_frame([WIDTH, HEIGHT]).unwrap().view_projection;
    let gpu: HashSet<u32> = gpu_slots.iter().copied().collect();
    assert_eq!(gpu.len(), gpu_slots.len(), "a slot was compacted twice");
    for placed in &pool.placed {
        let node = &nodes[placed.node];
        let lod = &lods[placed.node];
        let depth = node.node_depth + node.offsets[placed.level];
        let level = u32::from(node.offsets[placed.level]);
        for (page_index, page) in placed.packed.pages.iter().enumerate() {
            let entry = page_entry(nodes, pool, placed, page_index, eye);
            let side = entry.side();
            for local in 0..page.cells as usize {
                let coord = node.levels[placed.level].cells[page.first_cell + local].coord;
                let rel = node.rel(placed.level, coord);
                let decoded = decode_cell(
                    &entry,
                    &placed.packed.instances[page_index * PAGE_SLOTS + local],
                    &table,
                );
                // `outside` grows with a smaller margin: out with the larger one is out for the
                // shader (margin 1e-4), not out with the smaller one is in; between them is noise.
                let [out_small, out_large] =
                    FRUSTUM_MARGINS.map(|margin| outside(&matrix, decoded.lo, decoded.hi, margin));
                let drawn_by_gpu =
                    gpu.contains(&(placed.slots[page_index] * PAGE_SLOTS as u32 + local as u32));
                if out_small && !out_large {
                    report.near_frustum += 1;
                    continue;
                }
                let expected = if out_large {
                    false
                } else {
                    // The ratio of the cell (if a finer depth exists) and of every ancestor.
                    let ratio = |lo_level: u32| {
                        let up = level - lo_level;
                        let r = rel.map(|v| v >> up);
                        let s = side * f32::from_bits((127 + up) << 23);
                        edge_px(view, cell_lo(lod, r, s), s) / view.target_px
                    };
                    let finer = lod.active >> level >> 1 != 0;
                    let mut ratios = Vec::new();
                    if finer {
                        ratios.push(ratio(level));
                    }
                    ratios.extend((0..level).filter(|l| lod.active >> l & 1 != 0).map(ratio));
                    if ratios.iter().any(|r| (r - 1.0).abs() <= THRESHOLD_BAND) {
                        report.near_threshold += 1;
                        continue;
                    }
                    draws(view, lod, depth, rel, side)
                };
                report.compared += 1;
                report.drawn += usize::from(expected);
                if expected != drawn_by_gpu {
                    report.mismatch_count += 1;
                    if report.mismatches.len() < 8 {
                        report.mismatches.push(format!(
                            "node {} depth {depth} rel {rel:?}: twin {expected}, gpu {drawn_by_gpu}",
                            placed.node
                        ));
                    }
                }
            }
        }
    }
}

fn lod_scenes(seed: u64) -> Vec<Vec<LodNodeScene>> {
    let mut rng = Rng(seed);
    let unit = CellGrid {
        root_min: [0.0; 3],
        root_side: 1.0,
    };
    let offset_grid = CellGrid {
        root_min: [0.137, -0.093, 0.211],
        root_side: 0.9,
    };
    // Non-contiguous depths and an active node-depth cell.
    let plain = LodNodeScene::random(&mut rng, unit, 1, [1, 0, 1], &[0, 2, 3, 6], 0, 300);
    // A partially masked node: the 8^3 bit decides from the D5 level, the mips above it.
    let masked = LodNodeScene::random(&mut rng, unit, 2, [1, 2, 0], &[1, 3, 4, 7], 20, 300);
    let first = LodNodeScene::random(&mut rng, unit, 1, [1, 0, 1], &[0, 2, 3, 6], 0, 200);
    let second = LodNodeScene::random(&mut rng, offset_grid, 1, [0, 1, 0], &[1, 2, 5], 10, 200);
    vec![vec![plain], vec![masked], vec![first, second]]
}

#[test]
fn cell_mode_matches_the_twin_away_from_the_threshold() {
    for_each_adapter(CellLimitMode::Auto, |h| {
        if !h.runs_test_pool {
            return false;
        }
        let mut total = TwinReport::default();
        for (scene_index, nodes) in lod_scenes(11).iter().enumerate() {
            let mut pool = new_pool(h);
            let placed = place_lod(&mut pool, nodes);
            let mut rng = Rng(100 + scene_index as u64);
            let mut poses = 0;
            while poses < 50 {
                let Some((camera, view)) = random_pose(&mut rng, ([0.0; 3], [1.0; 3]), (1.0, 30.0))
                else {
                    continue;
                };
                poses += 1;
                let lods = set_pose(&mut pool, nodes, &placed, &camera, view);
                render_cells(h, &mut pool, &camera, false);
                let slots = read_drawn_slots(h, &pool, 1);
                compare_with_twin(nodes, &placed, &lods, &camera, &view, &slots, &mut total);
            }
        }
        println!(
            "# adapter {}: {} cells compared ({} drawn), {} near the threshold, {} near the frustum",
            h.label, total.compared, total.drawn, total.near_threshold, total.near_frustum
        );
        assert!(
            total.mismatch_count == 0,
            "{}: {} cells disagree with the twin: {:#?}",
            h.label,
            total.mismatch_count,
            total.mismatches
        );
        assert!(
            total.drawn > 100 && total.compared > total.drawn * 2,
            "{total:?}"
        );
        true
    });
}

// ---------------------------------------------------------------------------------------------
// Task 7b: the drawn cells partition the points of a node
// ---------------------------------------------------------------------------------------------

type CellKey = (usize, usize, [u32; 3]);

/// A scene of the partition test and how its poses are drawn.
struct PartitionScene {
    name: &'static str,
    nodes: Vec<LodNodeScene>,
    /// Log-uniform range of the pose's `target_px`.
    target_px: (f32, f32),
    /// Log-uniform range of the camera distance to its target, in root units.
    distance: (f64, f64),
    /// `min_side` of the view: the side of this level of node 0 (cells of that side never refine).
    min_side_level: Option<usize>,
    /// Some level of node 0 has two pages with the children of one ancestor on both.
    split_family: bool,
}

/// A point of the data: it lies in the finest cell `fine` of `node` (integer coordinates relative
/// to the node corner), so which cell of each level contains it never depends on a float.
struct Sample {
    node: usize,
    fine: [u64; 3],
    position: [f64; 3],
    masked: bool,
}

#[derive(Debug, Default)]
struct SceneStats {
    poses: usize,
    /// Drawn cells per (node, depth), plain frames.
    drawn: std::collections::BTreeMap<(usize, u8), usize>,
    /// Most depths of one node drawn in one pose.
    max_depths: usize,
    /// Drawn cells with an active ancestor two or more levels up.
    far_ancestor: usize,
    eye_inside: usize,
}

#[derive(Debug, Default)]
struct PartitionReport {
    frames: usize,
    occlusion_frames: usize,
    threshold_cases: usize,
    points: usize,
    masked_points: usize,
    holes: usize,
    doubles: usize,
    masked_drawn: usize,
    scenes: Vec<SceneStats>,
    failures: Vec<String>,
}

fn node_side(node: &LodNodeScene) -> f64 {
    node.grid.root_side / (1_u64 << node.node_depth) as f64
}

fn deepest(node: &LodNodeScene) -> u8 {
    *node.offsets.iter().max().unwrap()
}

/// Index of the finest level of `node`.
fn finest_level(node: &LodNodeScene) -> usize {
    node.offsets
        .iter()
        .position(|&o| o == deepest(node))
        .unwrap()
}

fn sample_at(node_index: usize, node: &LodNodeScene, fine: [u64; 3], frac: [f64; 3]) -> Sample {
    let fine_side = node_side(node) / (1_u64 << deepest(node)) as f64;
    let position =
        std::array::from_fn(|a| node.corner()[a] + (fine[a] as f64 + frac[a]) * fine_side);
    // Every scene's finest level is at least 3 below the node: its cells sit inside one mask bit.
    let bit = fine.map(|f| (f >> (deepest(node) - 3)) as usize);
    let masked = node.mask[bit[2]] >> (bit[0] + 8 * bit[1]) & 1 != 0;
    Sample {
        node: node_index,
        fine,
        position,
        masked,
    }
}

fn level_sides(nodes: &[LodNodeScene], pool: &LodPool) -> Vec<Vec<f32>> {
    let mut sides: Vec<Vec<f32>> = nodes.iter().map(|n| vec![0.0; n.levels.len()]).collect();
    for placed in &pool.placed {
        sides[placed.node][placed.level] = page_entry(nodes, pool, placed, 0, [0.0; 3]).side();
    }
    sides
}

/// Camera at `distance` from `target` (or at a random point of `inside`), looking at it.
fn pose_toward(
    rng: &mut Rng,
    target: [f64; 3],
    inside: Option<([f64; 3], [f64; 3])>,
    distance: (f64, f64),
    target_px: (f32, f32),
) -> Option<(OrbitCamera, LodView)> {
    let unit = |rng: &mut Rng| rng.below(1 << 24) as f64 / (1_u64 << 24) as f64;
    let position: [f64; 3] = if let Some((lo, hi)) = inside {
        std::array::from_fn(|a| lo[a] + (hi[a] - lo[a]) * unit(rng))
    } else {
        let direction: [f64; 3] = std::array::from_fn(|_| unit(rng) * 2.0 - 1.0);
        let length = direction.iter().map(|v| v * v).sum::<f64>().sqrt();
        if length < 0.1 {
            return None;
        }
        let range = distance.0 * (distance.1 / distance.0).powf(unit(rng));
        std::array::from_fn(|a| target[a] + direction[a] / length * range)
    };
    let mut camera = test_camera();
    if !camera.set_pose(position, target) {
        return None;
    }
    camera.eye_frame([WIDTH, HEIGHT])?;
    let px = target_px.0 * (target_px.1 / target_px.0).powf(unit(rng) as f32);
    Some((
        camera,
        LodView::from_camera(&camera, [WIDTH as f32, HEIGHT as f32], px, 0.0),
    ))
}

/// At least 2 % of the viewport inside every side of the frustum.
fn well_inside(matrix: &[[f32; 4]; 4], position: [f64; 3], eye: [f64; 3]) -> bool {
    let p: [f64; 3] = std::array::from_fn(|a| position[a] - eye[a]);
    let q: [f64; 4] = std::array::from_fn(|row| {
        f64::from(matrix[0][row]) * p[0]
            + f64::from(matrix[1][row]) * p[1]
            + f64::from(matrix[2][row]) * p[2]
            + f64::from(matrix[3][row])
    });
    q[3] > 0.0
        && q[0].abs() <= 0.96 * q[3]
        && q[1].abs() <= 0.96 * q[3]
        && q[2] >= 1.0e-6 * q[3]
        && q[2] <= (1.0 - 1.0e-6) * q[3]
}

/// Cells (node, level, rel) drawn by `lists`, with how many times each appears.
fn drawn_cells(
    nodes: &[LodNodeScene],
    pool: &LodPool,
    lists: &[Vec<u32>],
) -> std::collections::HashMap<CellKey, u32> {
    let mut by_page = std::collections::HashMap::new();
    for (index, placed) in pool.placed.iter().enumerate() {
        for (page, slot) in placed.slots.iter().enumerate() {
            by_page.insert(*slot, (index, page));
        }
    }
    let mut cells = std::collections::HashMap::new();
    for slot in lists.iter().flatten() {
        let (index, page) = by_page[&(slot / PAGE_SLOTS as u32)];
        let placed = &pool.placed[index];
        let node = &nodes[placed.node];
        let local = *slot as usize % PAGE_SLOTS;
        let coord =
            node.levels[placed.level].cells[placed.packed.pages[page].first_cell + local].coord;
        *cells
            .entry((placed.node, placed.level, node.rel(placed.level, coord)))
            .or_insert(0) += 1;
    }
    cells
}

/// `rel` of the cell of level `level` of `node` that contains `fine`.
fn rel_at(node: &LodNodeScene, level: usize, fine: [u64; 3]) -> [u32; 3] {
    let shift = deepest(node) - node.offsets[level];
    fine.map(|f| (f >> shift) as u32)
}

struct FrameCheck<'a> {
    adapter: &'a str,
    scene: &'a PartitionScene,
    scene_index: usize,
    seed: u64,
    label: &'static str,
    pool: &'a LodPool,
    lods: Vec<NodeLod>,
    sides: &'a [Vec<f32>],
    view: LodView,
    samples: &'a [Sample],
}

impl FrameCheck<'_> {
    /// Every sample point is covered by one drawn cell (`exact`: plain compaction) or by at most
    /// one (occlusion, over the union of both phases); masked points by none.
    fn run(&self, lists: &[Vec<u32>], exact: bool, report: &mut PartitionReport) {
        let nodes = &self.scene.nodes;
        let cells = drawn_cells(nodes, self.pool, lists);
        report.frames += 1;
        for sample in self.samples {
            let node = &nodes[sample.node];
            let covering: Vec<(usize, u32)> = (0..node.levels.len())
                .filter_map(|level| {
                    let key = (sample.node, level, rel_at(node, level, sample.fine));
                    cells.get(&key).map(|n| (level, *n))
                })
                .collect();
            let hits: u32 = covering.iter().map(|(_, n)| n).sum();
            report.points += 1;
            report.masked_points += usize::from(sample.masked);
            let problem = if sample.masked && hits > 0 {
                report.masked_drawn += 1;
                "a masked point is drawn"
            } else if hits >= 2 {
                report.doubles += 1;
                "double"
            } else if exact && !sample.masked && hits == 0 {
                report.holes += 1;
                "hole"
            } else {
                continue;
            };
            if report.failures.len() < 6 {
                report
                    .failures
                    .push(self.describe(problem, sample, &covering));
            }
        }
        if exact {
            self.count_drawn(&cells, report);
        }
    }

    fn count_drawn(
        &self,
        cells: &std::collections::HashMap<CellKey, u32>,
        report: &mut PartitionReport,
    ) {
        let stats = &mut report.scenes[self.scene_index];
        let mut depths = std::collections::HashSet::new();
        for &(node_index, level, _) in cells.keys() {
            let node = &self.scene.nodes[node_index];
            let depth = node.node_depth + node.offsets[level];
            *stats.drawn.entry((node_index, depth)).or_insert(0) += 1;
            depths.insert((node_index, depth));
            stats.far_ancestor +=
                usize::from(node.offsets.iter().any(|o| o + 2 <= node.offsets[level]));
        }
        for node_index in 0..self.scene.nodes.len() {
            let count = depths.iter().filter(|(n, _)| *n == node_index).count();
            stats.max_depths = stats.max_depths.max(count);
        }
    }

    fn describe(&self, problem: &str, sample: &Sample, covering: &[(usize, u32)]) -> String {
        let node = &self.scene.nodes[sample.node];
        let lod = &self.lods[sample.node];
        let mut text = format!(
            "{problem}: adapter {}, scene {} (#{}), pose seed {:#x}, frame {}, node {} point {:?} (fine cell {:?}, masked {}), target_px {}\n",
            self.adapter,
            self.scene.name,
            self.scene_index,
            self.seed,
            self.label,
            sample.node,
            sample.position,
            sample.fine,
            sample.masked,
            self.view.target_px,
        );
        for (level, &offset) in node.offsets.iter().enumerate() {
            let rel = rel_at(node, level, sample.fine);
            let side = self.sides[sample.node][level];
            let edge = edge_px(&self.view, cell_lo(lod, rel, side), side);
            let drawn = covering
                .iter()
                .find(|(l, _)| *l == level)
                .map_or(0, |(_, n)| *n);
            text += &format!(
                "    depth {} rel {rel:?}: edge_px {edge} ({}x target), refines {}, masked_any {}, drawn {drawn}\n",
                node.node_depth + offset,
                edge / self.view.target_px,
                refines(&self.view, cell_lo(lod, rel, side), side),
                masked_any(lod, node.node_depth + offset, rel),
            );
        }
        text
    }
}

/// The phase 2 list of the frame just rendered, then the phase 1 list, recomputed: the shared
/// `visible` buffer only keeps the last phase, but phase 1 reads the previous half of the
/// visibility bits, which the frame did not touch, so running it again gives the same list.
fn read_both_phases(h: &Harness, pool: &mut CellRenderer, camera: &OrbitCamera) -> Vec<Vec<u32>> {
    let (_, counts) = render_cells_phases(h, pool, camera, false);
    let second = read_drawn_slots(h, pool, 6);
    let mut encoder = h.device.create_command_encoder(&Default::default());
    pool.encode_compaction(&mut encoder, 1);
    h.queue.submit([encoder.finish()]);
    let first = read_drawn_slots(h, pool, 1);
    assert_eq!(
        [first.len() as u32, second.len() as u32],
        counts,
        "phase 1 was not reproduced"
    );
    vec![first, second]
}

/// Does some cell of some level of `nodes` contain `eye`?
fn eye_in_a_cell(nodes: &[LodNodeScene], cells: &HashSet<CellKey>, eye: [f64; 3]) -> bool {
    nodes.iter().enumerate().any(|(index, node)| {
        node.offsets.iter().enumerate().any(|(level, &offset)| {
            let side = node_side(node) / (1_u64 << offset) as f64;
            let rel: [f64; 3] =
                std::array::from_fn(|a| ((eye[a] - node.corner()[a]) / side).floor());
            let limit = (1_u64 << offset) as f64;
            rel.iter().all(|r| (0.0..limit).contains(r))
                && cells.contains(&(index, level, rel.map(|r| r as u32)))
        })
    })
}

/// Siblings of one ancestor (at a coarser level of node 0) on both sides of a page boundary of a
/// level of node 0.
fn split_families(scene: &PartitionScene, pool: &LodPool) -> usize {
    let node = &scene.nodes[0];
    let mut families = 0;
    for placed in pool.placed.iter().filter(|p| p.node == 0) {
        let offset = node.offsets[placed.level];
        let cells = &node.levels[placed.level].cells;
        for page in placed.packed.pages.iter().skip(1) {
            let (a, b) = (&cells[page.first_cell - 1], &cells[page.first_cell]);
            families += node
                .offsets
                .iter()
                .filter(|&&up| up < offset)
                .filter(|&&up| {
                    (0..3).all(|ax| a.coord[ax] >> (offset - up) == b.coord[ax] >> (offset - up))
                })
                .count();
        }
    }
    families
}

fn partition_scenes() -> Vec<PartitionScene> {
    let mut rng = Rng(0x7b);
    let unit = CellGrid {
        root_min: [0.0; 3],
        root_side: 1.0,
    };
    let offset_grid = CellGrid {
        root_min: [0.137, -0.093, 0.211],
        root_side: 0.9,
    };
    let dense = ((0.5, 40.0), (0.02, 3.0));
    let scene = |name, nodes, (target_px, distance), min_side_level, split_family| PartitionScene {
        name,
        nodes,
        target_px,
        distance,
        min_side_level,
        split_family,
    };
    let mut random = |grid, depth, lo, offsets: &[u8], mask, points| {
        LodNodeScene::random(&mut rng, grid, depth, lo, offsets, mask, points)
    };
    vec![
        scene(
            "three depths, non-contiguous, off-origin grid",
            vec![random(offset_grid, 1, [0, 1, 0], &[1, 3, 6], 0, 400)],
            dense,
            None,
            false,
        ),
        scene(
            "masked node at offset 0 with its D5 level (1 % mask)",
            vec![random(unit, 2, [1, 2, 0], &[0, 1, 2, 3, 5], 1, 500)],
            dense,
            None,
            false,
        ),
        scene(
            "a level split into pages",
            vec![random(unit, 1, [0, 1, 0], &[0, 3, 6], 0, 1800)],
            dense,
            None,
            true,
        ),
        scene(
            "min_side above the two finest levels' sides",
            vec![random(unit, 1, [1, 1, 0], &[0, 2, 4, 5], 0, 800)],
            dense,
            Some(2),
            false,
        ),
        scene(
            "span 25: rel above 2^24",
            vec![random(unit, 0, [0, 0, 0], &[0, 12, 25], 2, 30)],
            ((0.02, 6.0), (1.0e-5, 2.0)),
            None,
            false,
        ),
        scene(
            "two nodes side by side",
            vec![
                random(unit, 1, [0, 0, 0], &[0, 2, 4], 0, 300),
                random(unit, 1, [1, 0, 0], &[1, 3, 5], 2, 300),
            ],
            dense,
            None,
            false,
        ),
    ]
}

fn pose_count() -> usize {
    std::env::var("CELL_LOD_POSES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(200)
}

fn run_scene(
    h: &Harness,
    scene_index: usize,
    scene: &PartitionScene,
    poses: usize,
    report: &mut PartitionReport,
) {
    let nodes = &scene.nodes;
    let mut plain = new_pool(h);
    let mut occluded = new_occlusion_pool(h);
    let placed = place_lod(&mut plain, nodes);
    let again = place_lod(&mut occluded, nodes);
    assert_eq!(placed.node_slots, again.node_slots);
    let sides = level_sides(nodes, &placed);
    let min_side = scene.min_side_level.map_or(0.0, |level| sides[0][level]);
    if scene.split_family {
        assert!(
            split_families(scene, &placed) > 0,
            "{}: no family split across pages",
            scene.name
        );
    }
    let all_cells: HashSet<CellKey> = nodes
        .iter()
        .enumerate()
        .flat_map(|(index, node)| {
            (0..node.levels.len()).flat_map(move |level| {
                node.levels[level]
                    .cells
                    .iter()
                    .map(move |cell| (index, level, node.rel(level, cell.coord)))
            })
        })
        .collect();
    let finest: Vec<(usize, [u64; 3])> = nodes
        .iter()
        .enumerate()
        .flat_map(|(index, node)| {
            let level = finest_level(node);
            node.levels[level]
                .cells
                .iter()
                .map(move |cell| (index, node.rel(level, cell.coord).map(u64::from)))
        })
        .collect();
    let union = (
        std::array::from_fn(|a| nodes.iter().map(|n| n.corner()[a]).fold(f64::MAX, f64::min)),
        std::array::from_fn(|a| {
            nodes
                .iter()
                .map(|n| n.corner()[a] + node_side(n))
                .fold(f64::MIN, f64::max)
        }),
    );
    report.scenes.push(SceneStats::default());
    let (mut done, mut attempt) = (0, 0_u64);
    while done < poses {
        attempt += 1;
        let seed = 0x7b00_0000 + scene_index as u64 * 1_000_000 + attempt;
        let mut rng = Rng(seed);
        let kind = done % 4;
        let anchor = finest[rng.below(finest.len() as u64) as usize];
        let target = sample_at(anchor.0, &nodes[anchor.0], anchor.1, [0.5; 3]).position;
        let Some((camera, mut view)) = pose_toward(
            &mut rng,
            target,
            (kind == 3).then_some(union),
            scene.distance,
            scene.target_px,
        ) else {
            continue;
        };
        view.min_side = min_side;
        done += 1;
        let eye = camera.position();
        let matrix = camera.eye_frame([WIDTH, HEIGHT]).unwrap().view_projection;
        let mut samples: Vec<Sample> = finest
            .iter()
            .map(|&(node, fine)| sample_at(node, &nodes[node], fine, [0.5; 3]))
            .collect();
        let unit = |rng: &mut Rng| rng.below(1 << 24) as f64 / (1_u64 << 24) as f64;
        for _ in 0..finest.len() {
            let (node, fine) = finest[rng.below(finest.len() as u64) as usize];
            // Some points hug a face of their cell.
            let frac = std::array::from_fn(|_| match rng.below(6) {
                0 => 1.0e-3,
                1 => 1.0 - 1.0e-3,
                _ => unit(&mut rng),
            });
            samples.push(sample_at(node, &nodes[node], fine, frac));
        }
        samples.retain(|s| well_inside(&matrix, s.position, eye));
        let check = |label: &'static str, view: &LodView, lods: &[NodeLod]| FrameCheck {
            adapter: &h.label,
            scene,
            scene_index,
            seed,
            label,
            pool: &placed,
            lods: lods.to_vec(),
            sides: &sides,
            view: *view,
            samples: &samples,
        };
        report.scenes[scene_index].poses += 1;
        report.scenes[scene_index].eye_inside +=
            usize::from(kind == 3 && eye_in_a_cell(nodes, &all_cells, eye));

        let lods = set_pose(&mut plain, nodes, &placed, &camera, view);
        render_cells(h, &mut plain, &camera, false);
        let slots = read_drawn_slots(h, &plain, 1);
        check("plain", &view, &lods).run(&[slots], true, report);

        if kind == 2 {
            // The 1-ULP boundary of the edge of the cell of a random point, or of one of its
            // ancestors: a hair under, on and a hair over the target.
            let unmasked: Vec<&Sample> = samples.iter().filter(|s| !s.masked).collect();
            if !unmasked.is_empty() {
                let s = unmasked[rng.below(unmasked.len() as u64) as usize];
                let node = &nodes[s.node];
                let level = rng.below(node.levels.len() as u64) as usize;
                let side = sides[s.node][level];
                let rel = rel_at(node, level, s.fine);
                let edge = edge_px(&view, cell_lo(&lods[s.node], rel, side), side);
                if edge.is_finite() && edge > 0.0 && edge < 1.0e38 {
                    for bits in [edge.to_bits() - 1, edge.to_bits(), edge.to_bits() + 1] {
                        let mut at = view;
                        at.target_px = f32::from_bits(bits);
                        let lods = set_pose(&mut plain, nodes, &placed, &camera, at);
                        render_cells(h, &mut plain, &camera, false);
                        let slots = read_drawn_slots(h, &plain, 1);
                        report.threshold_cases += 1;
                        check("threshold", &at, &lods).run(&[slots], true, report);
                    }
                }
            }
        }

        // Two frames of the same pose: the second one has the first one's visibility bits.
        let lods = set_pose(&mut occluded, nodes, &placed, &camera, view);
        for _ in 0..2 {
            let lists = read_both_phases(h, &mut occluded, &camera);
            report.occlusion_frames += 1;
            check("occlusion", &view, &lods).run(&lists, false, report);
        }
    }
}

#[test]
fn cell_mode_draws_one_cell_per_point_on_every_adapter() {
    let poses = pose_count();
    for_each_adapter(CellLimitMode::Auto, |h| {
        if !h.runs_test_pool {
            return false;
        }
        let start = std::time::Instant::now();
        let mut report = PartitionReport::default();
        let scenes = partition_scenes();
        for (index, scene) in scenes.iter().enumerate() {
            run_scene(h, index, scene, poses, &mut report);
        }
        println!(
            "# adapter {}: {poses} poses per scene x {} scenes, {} frames ({} occlusion), {} points \
             ({} masked), {} holes, {} doubles, {} masked drawn, {} threshold cases, {:.1} s",
            h.label,
            scenes.len(),
            report.frames,
            report.occlusion_frames,
            report.points,
            report.masked_points,
            report.holes,
            report.doubles,
            report.masked_drawn,
            report.threshold_cases,
            start.elapsed().as_secs_f32(),
        );
        for (scene, stats) in scenes.iter().zip(&report.scenes) {
            println!(
                "#   {}: eye inside a cell in {} poses, up to {} depths of a node drawn at once, {} drawn cells with an ancestor 2+ levels up, drawn per (node, depth) {:?}",
                scene.name, stats.eye_inside, stats.max_depths, stats.far_ancestor, stats.drawn
            );
        }
        assert!(
            report.failures.is_empty(),
            "{}: {} holes, {} doubles, {} masked points drawn:\n{}",
            h.label,
            report.holes,
            report.doubles,
            report.masked_drawn,
            report.failures.join("\n")
        );
        for (scene, stats) in scenes.iter().zip(&report.scenes) {
            let levels = scene.nodes.iter().map(|n| n.offsets.len()).max().unwrap();
            if levels >= 2 {
                assert!(
                    stats.max_depths >= 2,
                    "{}: never drew two depths of a node",
                    scene.name
                );
            }
            if scene.nodes.iter().any(|n| n.offsets.len() >= 3) {
                assert!(
                    stats.far_ancestor > 0,
                    "{}: no ancestor 2+ levels up was decisive",
                    scene.name
                );
            }
        }
        assert!(report.scenes.iter().map(|s| s.eye_inside).sum::<usize>() > 0);
        assert!(report.threshold_cases > 0);
        true
    });
}

/// Drawn slots tallied by absolute depth.
fn tally_depths(
    nodes: &[LodNodeScene],
    pool: &LodPool,
    lists: &[Vec<u32>],
) -> [u32; crate::cell_render::COUNTER_DEPTHS] {
    let mut tally = [0; crate::cell_render::COUNTER_DEPTHS];
    for slot in lists.iter().flatten() {
        let page_slot = slot / PAGE_SLOTS as u32;
        let placed = pool
            .placed
            .iter()
            .find(|p| p.slots.contains(&page_slot))
            .expect("drawn slot of a placed page");
        let node = &nodes[placed.node];
        tally[usize::from(node.node_depth + node.offsets[placed.level])] += 1;
    }
    tally
}

#[test]
fn lod_counters_count_drawn_cells_per_depth() {
    for_each_adapter(CellLimitMode::Auto, |h| {
        if !h.runs_test_pool {
            return false;
        }
        let mut drawn_total = 0;
        for (scene_index, nodes) in lod_scenes(11).iter().enumerate() {
            let mut pool = new_pool(h);
            let placed = place_lod(&mut pool, nodes);
            let mut rng = Rng(300 + scene_index as u64);
            let mut poses = 0;
            while poses < 4 {
                let Some((camera, view)) = random_pose(&mut rng, ([0.0; 3], [1.0; 3]), (1.0, 30.0))
                else {
                    continue;
                };
                poses += 1;
                set_pose(&mut pool, nodes, &placed, &camera, view);

                // Counters off: nothing is counted.
                pool.set_occlusion(false);
                pool.set_counters(false);
                render_cells(h, &mut pool, &camera, false);
                let zero = pool.read_lod_counters(&h.device, &h.queue);
                assert_eq!(zero.per_depth, [0; 64], "{}: counters off", h.label);

                // Plain phase.
                pool.set_counters(true);
                render_cells(h, &mut pool, &camera, false);
                let counters = pool.read_lod_counters(&h.device, &h.queue);
                let slots = read_drawn_slots(h, &pool, 1);
                assert_eq!(
                    counters.per_depth,
                    tally_depths(nodes, &placed, std::slice::from_ref(&slots)),
                    "{}: plain frame",
                    h.label
                );
                drawn_total += slots.len();

                // Both occlusion phases: phase 2 is read straight after the frame, then phase 1 is
                // recomputed (the counters are read between them, before the recompute adds to
                // them).
                pool.set_occlusion(true);
                pool.reset_visibility();
                for _ in 0..2 {
                    let (_, counts) = render_cells_phases(h, &mut pool, &camera, false);
                    let second = read_drawn_slots(h, &pool, 6);
                    let counters = pool.read_lod_counters(&h.device, &h.queue);
                    let mut encoder = h.device.create_command_encoder(&Default::default());
                    pool.encode_compaction(&mut encoder, 1);
                    h.queue.submit([encoder.finish()]);
                    let first = read_drawn_slots(h, &pool, 1);
                    assert_eq!([first.len() as u32, second.len() as u32], counts);
                    assert_eq!(
                        counters.per_depth,
                        tally_depths(nodes, &placed, &[first, second]),
                        "{}: occlusion frame",
                        h.label
                    );
                }
                pool.set_counters(false);
            }
        }
        assert!(drawn_total > 0, "{}: nothing was drawn", h.label);
        true
    });
}
