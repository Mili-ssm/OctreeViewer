//! Task 7a: the per-cell LOD decision of the slot pass against its Rust twin (`cell_lod`).
//!
//! The scenes put cells of several active depths of one node over the same points, so the
//! all-ancestors rule, the masks and their mips all matter. The GPU result is the list of drawn
//! slot ids of the plain compaction, read back from the `visible` buffer; every cell whose
//! decision does not hang on a float within `THRESHOLD_BAND` of the target must agree with
//! `cell_lod::draws`. The scene, pose and readback helpers are shared with the larger partition
//! test of Task 7b.

use super::*;
use crate::cell_lod::{LodView, NodeLod, cell_lo, draws, edge_px, node_record};
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
