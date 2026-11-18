//! Task 8: picking on the cell pool.
//!
//! The oracle is the picking of the CPU path (`build_pick_chunks` + `resolve_pick_instances`) over
//! the cells the pool draws, as `InstanceRaw`. The filters, the cutoff and the stored eye are also
//! checked on crafted bytes, without a GPU, and `ray_box_interval` against a brute-force sampling.

use glam::DVec3;

use super::*;
use crate::EyeFrame;
use crate::cell_pages::FLAG_SPANS_OCTANTS;
use crate::cell_pick::{
    PICK_MAX_PAGES, PoolCopy, PoolPickPlan, candidates, prepare_pool_pick, resolve_pool_pick,
    select_pages,
};
use crate::cell_render::PageFrameStamp;
use crate::gpu::{
    PickCopy, PickHit, PickRay, build_pick_chunks, ray_box_interval, resolve_pick_instances,
};
use crate::test_support::mask_record;

// ---------------------------------------------------------------------------------------------
// Crafted pages (no GPU)
// ---------------------------------------------------------------------------------------------

fn crafted_page(origin: [f32; 3], octants: u32) -> PageEntry {
    PageEntry([
        origin[0].to_bits(),
        origin[1].to_bits(),
        origin[2].to_bits(),
        1.0_f32.to_bits(),
        0,
        octants,
        20 | 20 << 16,
        20,
        0,
        0,
        0,
        0,
    ])
}

fn crafted_cell(rel: [u16; 3], mask_index: u32, spans: bool) -> CellInstance {
    let flags = mask_index | if spans { FLAG_SPANS_OCTANTS } else { 0 };
    CellInstance([
        u32::from(rel[0]) | u32::from(rel[1]) << 16,
        u32::from(rel[2]) | flags << 16,
        0,
        0,
    ])
}

fn page_bytes(cells: &[CellInstance]) -> Vec<u8> {
    let mut all = cells.to_vec();
    all.resize(PAGE_SLOTS, CellInstance::EMPTY);
    bytemuck::cast_slice(&all).to_vec()
}

fn ray_down(x: f64, y: f64, from_z: f64) -> PickRay {
    PickRay {
        origin: DVec3::new(x, y, from_z),
        direction: DVec3::NEG_Z,
        near_distance: 0.1,
        // Far beyond any crafted geometry.
        far_distance: 1.0e9,
    }
}

fn plan(ray: PickRay, copies: Vec<PoolCopy>, cutoff: Option<f64>) -> PoolPickPlan {
    PoolPickPlan {
        ray,
        eye: [0.0; 3],
        visual_scale: 1.0,
        coordinate_origin: [0.0; 3],
        copies,
        cutoff,
        lod: None,
    }
}

fn mask_with(bits: &[usize]) -> [u32; MASK_WORDS] {
    let mut mask = [0; MASK_WORDS];
    for bit in bits {
        mask[bit / 32] |= 1 << (bit % 32);
    }
    mask
}

/// Resolves one page whose cells sit at x = y = 0, z = their `rel`, against a ray down x = y = 0.5.
fn local_hit(octants: u32, mask: &[usize], cells: &[CellInstance]) -> Option<usize> {
    let copy = PoolCopy {
        slot: 0,
        page: crafted_page([0.0; 3], octants),
        mask: mask_with(mask),
    };
    resolve_pool_pick(
        &plan(ray_down(0.5, 0.5, 100.0), vec![copy], None),
        &page_bytes(cells),
    )
    .unwrap()
    .map(|hit| hit.instance_index)
}

#[test]
fn resolution_applies_the_slot_pass_filters() {
    let low = crafted_cell([0, 0, 1], 0, false);
    // Mask cell (0, 0, z) is in octant 0 for z < 4 and in octant 1 from there.
    let other_octant = crafted_cell([0, 0, 5], 64 * 5, false);
    let masked = crafted_cell([0, 0, 3], 64 * 3, false);
    let spanning = crafted_cell([0, 0, 6], 64 * 6, true);
    // Octant 1 is not in the page bits and the mask bit of the spanning cell is set: only the
    // bypass keeps it.
    let mask = [64 * 3, 64 * 6];
    assert_eq!(local_hit(0b01, &mask, &[low]), Some(0));
    assert_eq!(
        local_hit(0b01, &mask, &[other_octant, low]),
        Some(1),
        "a cell of another octant must not win"
    );
    assert_eq!(
        local_hit(0b01, &mask, &[masked, low]),
        Some(1),
        "a masked cell must not win"
    );
    assert_eq!(
        local_hit(0b01, &mask, &[spanning, low]),
        Some(0),
        "a cell that spans octants ignores octant bits and mask"
    );
    assert_eq!(local_hit(0b11, &mask, &[other_octant, low]), Some(0));
}

#[test]
fn resolution_skips_empty_slots() {
    // An empty slot decodes to relative coordinate 65535 on every axis; aim at the middle of its
    // unit cell.
    let far = f64::from(u16::MAX) + 0.5;
    let copy = PoolCopy {
        slot: 0,
        page: crafted_page([0.0; 3], 0xff),
        mask: [0; MASK_WORDS],
    };
    let hit = resolve_pool_pick(
        &plan(ray_down(far, far, 1.0e5), vec![copy], None),
        &page_bytes(&[]),
    )
    .unwrap();
    assert_eq!(hit, None);
}

#[test]
fn resolution_ties_go_to_the_lower_global_slot() {
    let cell = crafted_cell([0, 0, 1], 0, false);
    let copy = |slot| PoolCopy {
        slot,
        page: crafted_page([0.0; 3], 0xff),
        mask: [0; MASK_WORDS],
    };
    let ray = ray_down(0.5, 0.5, 100.0);
    let mut bytes = page_bytes(&[cell]);
    bytes.extend(page_bytes(&[cell]));
    let hit = |copies| {
        resolve_pool_pick(&plan(ray, copies, None), &bytes)
            .unwrap()
            .unwrap()
            .instance_index
    };
    assert_eq!(hit(vec![copy(5), copy(3)]), 3 * PAGE_SLOTS);
    assert_eq!(hit(vec![copy(3), copy(5)]), 3 * PAGE_SLOTS);
    // Within a page the lower local slot wins.
    let bytes = page_bytes(&[CellInstance::EMPTY, cell, cell]);
    let hit = resolve_pool_pick(&plan(ray, vec![copy(7)], None), &bytes)
        .unwrap()
        .unwrap();
    assert_eq!(hit.instance_index, 7 * PAGE_SLOTS + 1);
}

#[test]
fn resolution_reports_dataset_units() {
    let cell = crafted_cell([0, 0, 2], 0, false);
    let copy = PoolCopy {
        slot: 2,
        page: crafted_page([0.0; 3], 0xff),
        mask: [0; MASK_WORDS],
    };
    // The page is relative to an eye at (10, 20, 30); the camera ray starts 100 above it.
    let mut shifted = plan(
        PickRay {
            origin: DVec3::new(0.5, 0.5, 100.0),
            ..ray_down(0.0, 0.0, 0.0)
        },
        vec![copy],
        None,
    );
    shifted.eye = [10.0, 20.0, 30.0];
    shifted.visual_scale = 2.0;
    shifted.coordinate_origin = [1000.0, 2000.0, 3000.0];
    let hit = resolve_pool_pick(&shifted, &page_bytes(&[cell]))
        .unwrap()
        .unwrap();
    // Eye-relative cell [0,1]x[0,1]x[2,3]; the ray hits its top face at z = 3, 97 below the start.
    assert_eq!(hit.instance_index, 2 * PAGE_SLOTS);
    assert_eq!(hit.distance, 97.0 / 2.0);
    assert_eq!(hit.voxel_size, 0.5);
    assert_eq!(hit.voxel_center, [1005.25, 2010.25, 3016.25]);
    assert_eq!(hit.position, [1005.25, 2010.25, 3016.5]);
}

#[test]
fn a_hit_must_come_before_the_first_page_left_out() {
    let cell = crafted_cell([0, 0, 1], 0, false);
    let copy = PoolCopy {
        slot: 0,
        page: crafted_page([0.0; 3], 0xff),
        mask: [0; MASK_WORDS],
    };
    let ray = ray_down(0.5, 0.5, 100.0);
    let bytes = page_bytes(&[cell]);
    // The cell is hit at distance 98.
    let resolve = |cutoff| resolve_pool_pick(&plan(ray, vec![copy], cutoff), &bytes);
    assert!(resolve(None).unwrap().is_some());
    assert!(resolve(Some(98.5)).unwrap().is_some());
    assert!(
        resolve(Some(98.0)).is_err(),
        "a hit at the cutoff is not safe"
    );
    assert!(
        resolve(Some(50.0)).is_err(),
        "a hit past the last copied page is not safe"
    );
    // No hit at all with pages left out: the ray may hit one of them.
    let miss = ray_down(5.0, 5.0, 100.0);
    assert!(
        resolve_pool_pick(&plan(miss, vec![copy], Some(50.0)), &bytes).is_err(),
        "a miss with uncopied pages is not a miss"
    );
    assert_eq!(
        resolve_pool_pick(&plan(miss, vec![copy], None), &bytes).unwrap(),
        None
    );
}

// ---------------------------------------------------------------------------------------------
// Pool against the CPU path
// ---------------------------------------------------------------------------------------------

fn camera_ray(camera: &OrbitCamera, pixel: [f64; 2]) -> Option<PickRay> {
    let (origin, direction, near_distance, far_distance) = camera.ray(pixel, [WIDTH, HEIGHT])?;
    Some(PickRay {
        origin,
        direction,
        near_distance,
        far_distance,
    })
}

fn stamp(eye: [f64; 3]) -> PageFrameStamp {
    PageFrameStamp {
        eye_scaled: eye,
        visual_scale: 1.0,
        coordinate_origin: [0.0; 3],
    }
}

fn pool_pick(h: &Harness, pool: &CellRenderer, ray: PickRay) -> Result<Option<PickHit>, String> {
    let Some(pick) = prepare_pool_pick(&h.device, &h.queue, pool, ray, 1.0, [0.0; 3], u64::MAX)?
    else {
        return Ok(None);
    };
    let bytes = read_buffer(h, pick.buffer());
    pick.resolve(&bytes)
}

/// Distance at which the ray hits the box, with the clamps of the pick.
fn box_distance(ray: &PickRay, min: [f64; 3], max: [f64; 3]) -> Option<f64> {
    let (entry, exit) = ray_box_interval(ray.origin, ray.direction, min, max)?;
    let distance = entry.max(ray.near_distance);
    (distance <= exit && distance <= ray.far_distance).then_some(distance)
}

/// The pick of the CPU path and the two best distances over all the cells.
struct Oracle {
    instances: Vec<InstanceRaw>,
    chunks: Vec<crate::gpu::PickChunk>,
}

impl Oracle {
    fn new(instances: Vec<InstanceRaw>) -> Self {
        let chunks = build_pick_chunks(&instances, 1.0);
        Self { instances, chunks }
    }

    fn distance(ray: &PickRay, instance: &InstanceRaw) -> Option<f64> {
        let half = f64::from(instance.size) * 0.5;
        let center = instance.center.map(f64::from);
        box_distance(
            ray,
            center.map(|value| value - half),
            center.map(|value| value + half),
        )
    }

    fn pick(&self, ray: PickRay) -> (Option<PickHit>, Option<f64>) {
        let size = std::mem::size_of::<InstanceRaw>();
        let (mut copies, mut bytes) = (Vec::new(), Vec::new());
        for chunk in &self.chunks {
            if ray_box_interval(
                ray.origin,
                ray.direction,
                chunk.bounds.min,
                chunk.bounds.max,
            )
            .is_some()
            {
                copies.push(PickCopy {
                    staging_offset: bytes.len() as u64,
                    first_instance: chunk.first_instance,
                    instance_count: chunk.instance_count,
                });
                bytes.extend_from_slice(bytemuck::cast_slice(
                    &self.instances
                        [chunk.first_instance..chunk.first_instance + chunk.instance_count],
                ));
            }
        }
        assert_eq!(bytes.len() % size, 0);
        let hit = resolve_pick_instances(ray, &copies, &bytes, 1.0, [0.0; 3]).unwrap();
        let mut distances: Vec<f64> = self
            .instances
            .iter()
            .filter_map(|instance| Self::distance(&ray, instance))
            .collect();
        distances.sort_by(f64::total_cmp);
        (hit, distances.get(1).copied())
    }
}

/// Global slot (`page slot * 512 + local`) of every cell of `Scene::drawn_cells`.
fn global_slots(scene: &Scene, placed: &[Placed]) -> Vec<usize> {
    scene
        .drawn_cells()
        .iter()
        .map(|(level, cell, _)| {
            let placed = &placed[*level];
            let page = placed
                .packed
                .pages
                .iter()
                .position(|page| {
                    *cell >= page.first_cell && *cell < page.first_cell + page.cells as usize
                })
                .unwrap();
            let first = placed.packed.pages[page].first_cell;
            placed.slots[page] as usize * PAGE_SLOTS + (cell - first)
        })
        .collect()
}

/// Boxes of the cells the shader filters out (masked or of another octant), in world units.
fn filtered_boxes(scene: &Scene) -> Vec<([f64; 3], [f64; 3])> {
    let mut out = Vec::new();
    for level in &scene.levels {
        let grid = scene.nodes[level.node].grid;
        let side = grid.root_side / (1_u64 << level.level.depth) as f64;
        for cell in &level.level.cells {
            if !scene.drawn(level, cell) {
                let min = std::array::from_fn(|axis| {
                    grid.root_min[axis] + cell.coord[axis] as f64 * side
                });
                out.push((min, min.map(|value| value + side)));
            }
        }
    }
    out
}

fn to_pixel(frame: &EyeFrame, point: [f64; 3]) -> Option<[f64; 2]> {
    let relative = [
        point[0] - frame.eye[0],
        point[1] - frame.eye[1],
        point[2] - frame.eye[2],
        1.0,
    ];
    let clip = |row: usize| -> f64 {
        (0..4)
            .map(|col| f64::from(frame.view_projection[col][row]) * relative[col])
            .sum()
    };
    let w = clip(3);
    if w <= 1.0e-9 {
        return None;
    }
    let (x, y) = (clip(0) / w, clip(1) / w);
    Some([
        (x + 1.0) * f64::from(WIDTH) * 0.5 - 0.5,
        (1.0 - y) * f64::from(HEIGHT) * 0.5 - 0.5,
    ])
}

/// A regular grid plus the points 0.1 px to each side of the middle of projected cell edges.
fn sample_pixels(scene: &Scene, camera: &OrbitCamera) -> (Vec<[f64; 2]>, usize) {
    let mut pixels = Vec::new();
    for j in 0..18 {
        for i in 0..32 {
            pixels.push([
                (f64::from(i) + 0.5) * f64::from(WIDTH) / 32.0,
                (f64::from(j) + 0.5) * f64::from(HEIGHT) / 18.0,
            ]);
        }
    }
    let grid = pixels.len();
    let frame = camera.eye_frame([WIDTH, HEIGHT]).unwrap();
    let drawn = scene.drawn_cells();
    for (_, _, instance) in drawn.iter().step_by((drawn.len() / 40).max(1)) {
        let half = f64::from(instance.size) * 0.5;
        let corner = |bits: usize| -> [f64; 3] {
            std::array::from_fn(|axis| {
                f64::from(instance.center[axis]) + if bits >> axis & 1 == 1 { half } else { -half }
            })
        };
        for a in 0..8_usize {
            for axis in 0..3 {
                let b = a | 1 << axis;
                if a == b {
                    continue;
                }
                let (Some(pa), Some(pb)) =
                    (to_pixel(&frame, corner(a)), to_pixel(&frame, corner(b)))
                else {
                    continue;
                };
                let (dx, dy) = (pb[0] - pa[0], pb[1] - pa[1]);
                let length = dx.hypot(dy);
                if length < 1.0 {
                    continue;
                }
                let mid = [(pa[0] + pb[0]) * 0.5, (pa[1] + pb[1]) * 0.5];
                for side in [-0.1, 0.1] {
                    pixels.push([mid[0] - dy / length * side, mid[1] + dx / length * side]);
                }
            }
        }
    }
    (pixels, grid)
}

#[derive(Debug, Default)]
struct Agreement {
    samples: usize,
    grid_samples: usize,
    grid_hits: usize,
    hits: usize,
    excluded: usize,
    compared: usize,
    /// Pixels where the nearest cell ignoring the mask and octant filters is not the winner.
    filter_mattered: usize,
    max_center_error: f64,
    max_distance_error: f64,
    mismatches: Vec<String>,
}

fn compare_with_oracle(
    h: &Harness,
    pool: &CellRenderer,
    scene: &Scene,
    placed: &[Placed],
    camera: &OrbitCamera,
) -> Agreement {
    let oracle = Oracle::new(scene.cpu_instances());
    let slots = global_slots(scene, placed);
    let hidden = filtered_boxes(scene);
    let (pixels, grid) = sample_pixels(scene, camera);
    let mut out = Agreement {
        samples: pixels.len(),
        grid_samples: grid,
        ..Agreement::default()
    };
    for (index, pixel) in pixels.iter().enumerate() {
        let Some(ray) = camera_ray(camera, *pixel) else {
            out.samples -= 1;
            out.grid_samples -= usize::from(index < grid);
            continue;
        };
        let (expected, second) = oracle.pick(ray);
        let actual = pool_pick(h, pool, ray).unwrap();
        out.hits += usize::from(actual.is_some());
        out.grid_hits += usize::from(index < grid && actual.is_some());
        if let (Some(expected), Some(second)) = (&expected, second)
            && second - expected.distance < 1.0e-5 * expected.distance
        {
            out.excluded += 1;
            continue;
        }
        out.compared += 1;
        match (expected, actual) {
            (None, None) => {}
            (Some(expected), Some(actual)) => {
                let side = expected.voxel_size;
                let center_error = (0..3)
                    .map(|axis| (expected.voxel_center[axis] - actual.voxel_center[axis]).powi(2))
                    .sum::<f64>()
                    .sqrt();
                let magnitude = expected
                    .voxel_center
                    .iter()
                    .map(|v| v * v)
                    .sum::<f64>()
                    .sqrt();
                let allowed = (expected.distance + magnitude) * 2.0_f64.powi(-20);
                let distance_error = (expected.distance - actual.distance).abs();
                out.max_center_error = out.max_center_error.max(center_error / side);
                out.max_distance_error = out.max_distance_error.max(distance_error / allowed);
                let same = slots[expected.instance_index] == actual.instance_index
                    && (expected.voxel_size - actual.voxel_size).abs() <= 1.0e-6 * side
                    && center_error <= 1.0e-3 * side
                    && distance_error <= allowed;
                if same {
                    // Without the filters a hidden cell would win here.
                    let hidden_wins = hidden.iter().any(|(min, max)| {
                        box_distance(&ray, *min, *max)
                            .is_some_and(|distance| distance < actual.distance - 1.0e-9)
                    });
                    out.filter_mattered += usize::from(hidden_wins);
                } else {
                    out.mismatches.push(format!(
                        "pixel {pixel:?}: oracle {expected:?} (slot {}), pool {actual:?}",
                        slots[expected.instance_index]
                    ));
                }
            }
            (expected, actual) => out.mismatches.push(format!(
                "pixel {pixel:?}: oracle {expected:?}, pool {actual:?}"
            )),
        }
    }
    out
}

/// One node, two levels: a coarse level and a fine level that draws one octant only. Cells of
/// both levels also sit in masked regions and in the other octants.
fn pick_scene() -> Scene {
    let mut rng = Rng(21);
    let mut next_id = 0;
    let mut scene = Scene {
        nodes: Vec::new(),
        levels: Vec::new(),
    };
    scene.add_cut_node(
        &mut rng,
        &mut next_id,
        CellGrid {
            root_min: [-1.5; 3],
            root_side: 4.0,
        },
        &[(3, 260, 0xff), (4, 700, 0b0000_0100)],
    );
    scene
}

fn placed_pool(h: &Harness, scene: &Scene, eye: [f64; 3]) -> (CellRenderer, Vec<Placed>) {
    let mut pool = new_pool(h);
    let placed = place(&mut pool, scene, 0..);
    pool.set_active_pages(&entries(scene, &placed, eye))
        .unwrap();
    pool.set_page_frame(stamp(eye));
    (pool, placed)
}

#[test]
fn pick_on_pool_hits_same_cell_as_cpu_path() {
    for_each_adapter(CellLimitMode::Auto, |h| {
        if !h.runs_test_pool {
            return false;
        }
        let scene = pick_scene();
        let camera = big_camera();
        let (pool, placed) = placed_pool(h, &scene, camera.position());
        let agreement = compare_with_oracle(h, &pool, &scene, &placed, &camera);
        println!("# adapter {}: pick agreement {agreement:?}", h.label);
        assert!(agreement.grid_samples >= 32 * 18);
        assert!(
            agreement.mismatches.is_empty(),
            "{:#?}",
            agreement.mismatches
        );
        assert!(
            agreement.excluded * 50 <= agreement.samples,
            "too many ambiguous pixels: {agreement:?}"
        );
        assert!(
            agreement.grid_hits * 2 >= agreement.grid_samples,
            "the grid is mostly empty: {agreement:?}"
        );
        assert!(
            agreement.filter_mattered > 0,
            "the mask and octant filters never changed the winner: {agreement:?}"
        );
        true
    });
}

#[test]
fn moving_the_camera_without_rewriting_pages_keeps_the_picks() {
    for_each_adapter(CellLimitMode::Auto, |h| {
        if !h.runs_test_pool {
            return false;
        }
        let scene = pick_scene();
        let camera = big_camera();
        let (pool, placed) = placed_pool(h, &scene, camera.position());
        let mut moved = camera;
        // Radians and zoom step big enough to change which cells are in front.
        assert!(moved.orbit(0.2, 0.05));
        assert!(moved.zoom(0.1));
        let gap = (0..3)
            .map(|axis| (moved.position()[axis] - camera.position()[axis]).powi(2))
            .sum::<f64>()
            .sqrt();
        // Well above rounding, so the eye of the pages is really stale.
        assert!(gap > 0.05, "the camera barely moved: {gap}");
        let agreement = compare_with_oracle(h, &pool, &scene, &placed, &moved);
        println!("# adapter {}: moved pick agreement {agreement:?}", h.label);
        assert!(
            agreement.mismatches.is_empty(),
            "{:#?}",
            agreement.mismatches
        );
        assert!(
            agreement.grid_hits * 2 >= agreement.grid_samples,
            "the grid is mostly empty: {agreement:?}"
        );
        // The pages keep the frame they were built with: another scale or origin is refused.
        let ray = camera_ray(&moved, [f64::from(WIDTH) / 2.0, f64::from(HEIGHT) / 2.0]).unwrap();
        let pick = |pool: &CellRenderer, scale, origin| {
            prepare_pool_pick(&h.device, &h.queue, pool, ray, scale, origin, u64::MAX)
                .map(|p| p.is_some())
        };
        assert!(pick(&pool, 2.0, [0.0; 3]).is_err());
        assert!(pick(&pool, 1.0, [1.0, 0.0, 0.0]).is_err());
        // No active pages yet: nothing to pick.
        assert_eq!(pick(&new_pool(h), 2.0, [0.0; 3]), Ok(false));
        true
    });
}

#[test]
fn pick_copies_at_most_the_page_limit_and_keeps_the_nearest_hit_safe() {
    for_each_adapter(CellLimitMode::Auto, |h| {
        if !h.runs_test_pool {
            return false;
        }
        // More pages than the limit, so some stay uncopied.
        let pages = PICK_MAX_PAGES + 22;
        let mut pool = CellRenderer::new(&h.device, &h.queue, COLOR_FORMAT, pages as u32);
        // One cell per node, the nodes stacked along z; the ray down x = y = 0.5 crosses all.
        let eye = [0.25, 0.0, 500.0];
        let mask_slot = pool.alloc_mask_slot().unwrap();
        let mut active = Vec::new();
        for index in 0..pages {
            let grid = CellGrid {
                root_min: [0.0, 0.0, 2.0 * index as f64],
                root_side: 8.0,
            };
            let level = NodeCellLevel {
                depth: 3,
                cells: vec![cell([0, 0, 0], index as u32 + 1)],
                octant_offsets: None,
            };
            let packed = pack_level(
                &grid,
                unit_bounds(&grid),
                NodeId::new(index as u64 + 1),
                &level,
            );
            pool.upload_level(&packed, &[index as u32]).unwrap();
            let frame = PageFrame {
                grid,
                coordinate_origin: [0.0; 3],
                visual_scale: 1.0,
                eye_scaled: eye,
            };
            active.push((index as u32, packed.pages[0].entry(&frame, mask_slot, 0xff)));
        }
        pool.set_active_pages(&active).unwrap();
        pool.set_page_frame(stamp(eye));
        let ray = PickRay {
            origin: DVec3::new(0.5, 0.5, 1000.0),
            direction: DVec3::NEG_Z,
            near_distance: 0.1,
            far_distance: 1.0e6,
        };
        // Cells are 1 high and 2 apart, so the nearest is the last page.
        let nearest = (pages - 1) * PAGE_SLOTS;
        let hit = pool_pick(h, &pool, ray).unwrap().unwrap();
        assert_eq!(hit.instance_index, nearest);
        assert_eq!(hit.voxel_size, 1.0);
        // Hiding the cell of every copied page leaves hits only in pages that were not copied.
        pool.set_node_record(mask_slot, &mask_record(&mask_with(&[0])))
            .unwrap();
        assert!(pool_pick(h, &pool, ray).is_err());
        // The budget counts the staging buffer.
        // One byte short of one page: the smallest staging buffer that cannot fit.
        let short = PAGE_BYTES - 1;
        assert!(prepare_pool_pick(&h.device, &h.queue, &pool, ray, 1.0, [0.0; 3], short).is_err());
        true
    });
}

// ---------------------------------------------------------------------------------------------
// Pure rules (no GPU)
// ---------------------------------------------------------------------------------------------

#[test]
fn the_page_frame_must_use_the_units_of_the_renderer() {
    let frame = PageFrameStamp {
        eye_scaled: [1.0, 2.0, 3.0],
        visual_scale: 2.0,
        coordinate_origin: [10.0, 20.0, 30.0],
    };
    assert!(frame.same_units(2.0, [10.0, 20.0, 30.0]));
    assert!(!frame.same_units(2.0, [10.0, 20.0, 30.5]), "origin");
    assert!(!frame.same_units(2.5, [10.0, 20.0, 30.0]), "scale");
}

/// `count` pages stacked along z, 30 apart (their boxes are 21 high), so the one with the highest
/// z is the nearest to a ray going down.
fn stacked_pages(count: usize) -> Vec<(u32, PageEntry)> {
    (0..count)
        .map(|index| {
            (
                index as u32,
                crafted_page([0.0, 0.0, 30.0 * index as f32], 0xff),
            )
        })
        .collect()
}

#[test]
fn the_cutoff_is_the_entry_of_the_first_candidate_left_out() {
    let ray = ray_down(0.5, 0.5, 10_000.0);
    for count in [PICK_MAX_PAGES - 1, PICK_MAX_PAGES] {
        let found = candidates(stacked_pages(count).into_iter(), &ray);
        let (copied, cutoff) = select_pages(&found);
        assert_eq!((copied.len(), cutoff), (count, None), "{count} candidates");
    }
    let found = candidates(stacked_pages(PICK_MAX_PAGES + 1).into_iter(), &ray);
    assert_eq!(found.len(), PICK_MAX_PAGES + 1);
    let (copied, cutoff) = select_pages(&found);
    assert_eq!(copied.len(), PICK_MAX_PAGES);
    // The page at z = 0 is the farthest; its box top is at z = 21 (widened one ulp).
    assert_eq!(cutoff, Some(found[PICK_MAX_PAGES].distance));
    assert!((cutoff.unwrap() - (10_000.0 - 21.0)).abs() < 1.0e-3);
    assert!(
        copied.iter().all(|c| c.slot != 0),
        "the farthest is left out"
    );
}

#[test]
fn an_uncopied_page_nearer_than_the_hit_makes_the_pick_fail() {
    // 127 fillers on top, then page A (copied, box top at z = 121, cell top at z = 102), then page
    // B (left out, box top at z = 111): B starts before A's hit, so the hit is not safe.
    let ray = ray_down(0.5, 0.5, 10_000.0);
    let mut pages: Vec<(u32, PageEntry)> = (0..PICK_MAX_PAGES - 1)
        .map(|index| {
            let z = 200.0 + 30.0 * index as f32;
            (index as u32, crafted_page([0.0, 0.0, z], 0xff))
        })
        .collect();
    let a = PICK_MAX_PAGES as u32 - 1;
    pages.push((a, crafted_page([0.0, 0.0, 100.0], 0xff)));
    pages.push((a + 1, crafted_page([0.0, 0.0, 90.0], 0xff)));
    let found = candidates(pages.into_iter(), &ray);
    let (copied, cutoff) = select_pages(&found);
    assert_eq!(copied.last().map(|c| c.slot), Some(a));
    assert!((cutoff.unwrap() - (10_000.0 - 111.0)).abs() < 1.0e-3);
    let mut bytes = Vec::new();
    let copies = copied
        .iter()
        .map(|candidate| {
            let cells = if candidate.slot == a {
                vec![crafted_cell([0, 0, 2], 0, false)]
            } else {
                Vec::new()
            };
            bytes.extend(page_bytes(&cells));
            PoolCopy {
                slot: candidate.slot,
                page: candidate.page,
                mask: [0; MASK_WORDS],
            }
        })
        .collect();
    let result = resolve_pool_pick(&plan(ray, copies, cutoff), &bytes);
    assert!(result.is_err(), "{result:?}");
}

/// Interval of the ray inside the box by dense sampling: first and last sample inside, and the
/// step. Independent of the slab formula of `ray_box_interval`.
fn sampled_interval(
    origin: DVec3,
    direction: DVec3,
    min: [f64; 3],
    max: [f64; 3],
) -> (Option<(f64, f64)>, f64) {
    const STEPS: usize = 40_000;
    const LENGTH: f64 = 80.0;
    let step = LENGTH / STEPS as f64;
    let mut found: Option<(f64, f64)> = None;
    for index in 0..=STEPS {
        let t = index as f64 * step;
        let point = origin + direction * t;
        if (0..3).all(|axis| point[axis] >= min[axis] && point[axis] <= max[axis]) {
            found = Some((found.map_or(t, |(first, _)| first), t));
        }
    }
    (found, step)
}

#[test]
fn ray_box_interval_agrees_with_dense_sampling() {
    let mut rng = Rng(5);
    let mut unit = move || (rng.below(1_000_000) as f64) / 1.0e6;
    let (mut hits, mut misses, mut inside, mut parallel) = (0, 0, 0, 0);
    for case in 0..600 {
        let min: [f64; 3] = std::array::from_fn(|_| unit() * 6.0 - 3.0);
        let max: [f64; 3] = std::array::from_fn(|axis| min[axis] + 0.5 + unit() * 3.0);
        let mut origin = DVec3::from_array(std::array::from_fn(|_| unit() * 16.0 - 8.0));
        if case % 5 == 0 {
            origin = DVec3::from_array(std::array::from_fn(|axis| {
                min[axis] + (max[axis] - min[axis]) * unit()
            }));
            inside += 1;
        }
        let mut direction =
            DVec3::from_array(std::array::from_fn(|_| unit() * 2.0 - 1.0)).normalize();
        if case % 4 == 0 {
            // Parallel to the faces of one axis: the origin is inside or outside that slab.
            let axis = case / 4 % 3;
            direction[axis] = 0.0;
            direction = direction.normalize();
            parallel += 1;
        }
        let (reference, step) = sampled_interval(origin, direction, min, max);
        let interval = ray_box_interval(origin, direction, min, max);
        match (interval, reference) {
            (None, None) => misses += 1,
            (None, Some(_)) => panic!("case {case}: the helper misses a box the samples enter"),
            (Some((near, far)), None) => {
                assert!(
                    far - near <= step,
                    "case {case}: {near}..{far} has no sample"
                )
            }
            (Some((near, far)), Some((first, last))) => {
                hits += 1;
                let tolerance = 1.0e-9;
                assert!(
                    first >= near - tolerance && first - near <= step + tolerance,
                    "case {case}: entry {near} vs first sample {first}"
                );
                assert!(
                    last <= far + tolerance && far - last <= step + tolerance,
                    "case {case}: exit {far} vs last sample {last}"
                );
            }
        }
    }
    assert!(hits > 100 && misses > 100 && inside >= 100 && parallel >= 100);
}

// ---------------------------------------------------------------------------------------------
// Cell mode: the drawn cell is the one the per-cell decision draws
// ---------------------------------------------------------------------------------------------

/// `(nearest drawn (distance, global slot, depth), second best drawn distance, nearest cell of any kind)`.
type TwinPick = (Option<(f64, usize, u8)>, Option<f64>, Option<(f64, usize)>);

/// The nearest cell the twin draws along `ray` and the nearest cell of any kind, as
/// `(distance, global slot)`, plus the second best drawn distance.
fn twin_pick(
    scene: &[super::lod::LodNodeScene],
    pool: &super::lod::LodPool,
    lods: &[crate::cell_lod::NodeLod],
    view: &crate::cell_lod::LodView,
    eye: [f64; 3],
    ray: &PickRay,
) -> TwinPick {
    let table = crate::cell_pages::srgb_to_linear_table();
    let ray = PickRay {
        origin: ray.origin - glam::DVec3::from_array(eye),
        ..*ray
    };
    let (mut best, mut second, mut any): (Option<(f64, usize, u8)>, Option<f64>, _) =
        (None, None, None::<(f64, usize)>);
    for placed in &pool.placed {
        let node = &scene[placed.node];
        let depth = node.node_depth + node.offsets[placed.level];
        let side_node = node.grid.root_side / (1_u64 << node.node_depth) as f64;
        let node_lo: [u64; 3] = std::array::from_fn(|a| {
            ((node.bounds.min()[a] - node.grid.root_min[a]) / side_node).round() as u64
        });
        let frame = PageFrame {
            grid: node.grid,
            coordinate_origin: [0.0; 3],
            visual_scale: 1.0,
            eye_scaled: eye,
        };
        for (page_index, page) in placed.packed.pages.iter().enumerate() {
            let entry = page.entry(&frame, pool.node_slots[placed.node], 0xff);
            for local in 0..page.cells as usize {
                let coord = node.levels[placed.level].cells[page.first_cell + local].coord;
                let rel: [u32; 3] = std::array::from_fn(|a| {
                    (coord[a] - (node_lo[a] << node.offsets[placed.level])) as u32
                });
                let decoded = decode_cell(
                    &entry,
                    &placed.packed.instances[page_index * PAGE_SLOTS + local],
                    &table,
                );
                let Some(distance) =
                    box_distance(&ray, decoded.lo.map(f64::from), decoded.hi.map(f64::from))
                else {
                    continue;
                };
                let global = placed.slots[page_index] as usize * PAGE_SLOTS + local;
                if any.is_none_or(|a| (distance, global) < a) {
                    any = Some((distance, global));
                }
                if !crate::cell_lod::draws(view, &lods[placed.node], depth, rel, entry.side()) {
                    continue;
                }
                if best.is_none_or(|b| (distance, global) < (b.0, b.1)) {
                    second = best.map(|b| b.0);
                    best = Some((distance, global, depth));
                } else if second.is_none_or(|s| distance < s) {
                    second = Some(distance);
                }
            }
        }
    }
    (best, second, any)
}

#[test]
fn pick_in_cell_mode_hits_the_drawn_cell() {
    use super::lod::{LodNodeScene, place_lod, random_pose, set_pose};
    for_each_adapter(CellLimitMode::Auto, |h| {
        if !h.runs_test_pool {
            return false;
        }
        let unit = CellGrid {
            root_min: [0.0; 3],
            root_side: 1.0,
        };
        let mut rng = Rng(0x91c4);
        let nodes = vec![
            LodNodeScene::random(&mut rng, unit, 1, [0, 1, 0], &[0, 2, 4, 6], 0, 1500),
            LodNodeScene::random(&mut rng, unit, 1, [1, 0, 0], &[1, 3, 5], 0, 600),
        ];
        let bounds = ([0.0; 3], [1.0; 3]);
        let (mut compared, mut hits, mut ambiguous, mut mattered) = (0, 0, 0, 0);
        let mut depths = HashSet::new();
        let mut mismatches = Vec::new();
        let mut poses = 0;
        while poses < 24 {
            let Some((camera, view)) = random_pose(&mut rng, bounds, (0.5, 12.0)) else {
                continue;
            };
            poses += 1;
            let mut pool = new_pool(h);
            let placed = place_lod(&mut pool, &nodes);
            let lods = set_pose(&mut pool, &nodes, &placed, &camera, view);
            pool.set_page_frame(stamp(camera.position()));
            for iy in 0..12 {
                for ix in 0..16 {
                    let pixel = [
                        (ix as f64 + 0.5) / 16.0 * f64::from(WIDTH),
                        (iy as f64 + 0.5) / 12.0 * f64::from(HEIGHT),
                    ];
                    let Some(ray) = camera_ray(&camera, pixel) else {
                        continue;
                    };
                    let (expected, second, any) =
                        twin_pick(&nodes, &placed, &lods, &view, camera.position(), &ray);
                    let actual = pool_pick(h, &pool, ray).unwrap();
                    hits += usize::from(actual.is_some());
                    if let (Some(e), Some(second)) = (expected, second)
                        && second - e.0 < 1.0e-5 * e.0
                    {
                        ambiguous += 1;
                        continue;
                    }
                    compared += 1;
                    match (expected, actual) {
                        (None, None) => {}
                        (Some((distance, global, depth)), Some(actual)) => {
                            if actual.instance_index == global
                                && (actual.distance - distance).abs() <= 1.0e-9 * (1.0 + distance)
                            {
                                depths.insert(depth);
                                // Without the per-cell decision another cell would win here.
                                mattered += usize::from(
                                    any.is_some_and(|a| a.1 != global && a.0 < distance),
                                );
                            } else {
                                mismatches.push(format!(
                                    "pose {poses} pixel {pixel:?}: twin ({distance}, {global}), pick {actual:?}"
                                ));
                            }
                        }
                        (expected, actual) => mismatches.push(format!(
                            "pose {poses} pixel {pixel:?}: twin {expected:?}, pick {actual:?}"
                        )),
                    }
                }
            }
        }
        println!(
            "# adapter {}: cell pick {compared} compared, {hits} hits, {ambiguous} ambiguous, \
             {mattered} where the decision mattered, depths hit {depths:?}",
            h.label
        );
        assert!(
            mismatches.is_empty(),
            "{:#?}",
            &mismatches[..mismatches.len().min(8)]
        );
        assert!(
            compared > 200 && hits > 100,
            "too few picks: {compared} {hits}"
        );
        assert!(depths.len() >= 2, "only {depths:?} were hit");
        assert!(mattered > 0, "the cell decision never changed the winner");
        true
    });
}

/// A `CellRenderer` fed by the driver that also remembers the cells of each page slot.
struct PickTarget<'a> {
    pool: &'a mut CellRenderer,
    eye: [f64; 3],
    held: &'a mut std::collections::HashMap<u32, (NodeId, u8, Vec<[u64; 3]>)>,
    /// The page slots of the last active table.
    active: &'a mut HashSet<u32>,
}

impl crate::cell_driver::CellTarget for PickTarget<'_> {
    type Error = crate::CellPoolError;

    fn has_pool(&self) -> bool {
        true
    }

    fn drop_pool(&mut self) {
        unreachable!("the target never fails");
    }

    fn eye(&self) -> [f64; 3] {
        self.eye
    }

    fn upload_level(
        &mut self,
        level: &PackedLevel,
        slots: &[u32],
    ) -> Result<(), crate::CellPoolError> {
        for (index, (info, slot)) in level.pages.iter().zip(slots).enumerate() {
            let cells = (0..info.cells as usize)
                .map(|local| {
                    let rel = level.instances[index * PAGE_SLOTS + local].rel();
                    std::array::from_fn(|axis| info.origin[axis] + u64::from(rel[axis]))
                })
                .collect();
            self.held.insert(*slot, (level.node, level.depth, cells));
        }
        self.pool.upload_level(level, slots)
    }

    fn free_pages(&mut self, slots: &[u32]) -> Result<(), crate::CellPoolError> {
        for slot in slots {
            self.held.remove(slot);
        }
        self.pool.free_pages(slots)
    }

    fn alloc_mask_slot(&mut self) -> Result<Option<u32>, crate::CellPoolError> {
        Ok(self.pool.alloc_mask_slot())
    }

    fn set_node_record(
        &mut self,
        slot: u32,
        record: &[u32; NODE_WORDS],
    ) -> Result<(), crate::CellPoolError> {
        self.pool.set_node_record(slot, record)
    }

    fn set_lod_view(&mut self, view: Option<crate::cell_lod::LodView>) {
        self.pool.set_lod_view(view);
    }

    fn set_active_pages(
        &mut self,
        pages: &[(u32, PageEntry)],
        _frame: &PageFrame,
    ) -> Result<(), crate::CellPoolError> {
        *self.active = pages.iter().map(|(slot, _)| *slot).collect();
        self.pool.set_active_pages(pages)
    }
}

/// The records the driver writes feed the pick: it equals the twin evaluated on the active depths
/// the frame emits, also on retained frames (same cut, no `MaskUpdate`) whose active set moved.
#[test]
fn pick_on_driver_records_equals_the_twin_after_retained_frames() {
    use super::lod::{LodNodeScene, random_pose};
    use crate::cell_driver::{CellDriver, DriverConfig, NodeInput};
    for_each_adapter(CellLimitMode::Auto, |h| {
        if !h.runs_test_pool {
            return false;
        }
        let unit = CellGrid {
            root_min: [0.0; 3],
            root_side: 1.0,
        };
        // The scene must produce at least one retained frame whose active set moves
        // (`retained_changes > 0` below). Re-chosen from 0x7a11 on 18-11-2026 when `lo` became
        // coarser (ADR 0055): the old scene then gave none, this one gives 3. A seed with 0
        // fails that guard instead of passing vacuously.
        let mut rng = Rng(0x7a12);
        let scene = [
            LodNodeScene::random(&mut rng, unit, 1, [0, 1, 0], &[0, 2, 4, 6], 0, 1200),
            LodNodeScene::random(&mut rng, unit, 1, [1, 0, 0], &[1, 3, 5], 0, 500),
        ];
        let mut driver = CellDriver::new(DriverConfig {
            pool: crate::PoolConfig {
                capacity_pages: POOL_PAGES_FOR_TESTS,
                ..crate::PoolConfig::default()
            },
            grid: unit,
            coordinate_origin: [0.0; 3],
            cpu_budget_bytes: u64::MAX,
        });
        let mut cut = Vec::new();
        for (index, node) in scene.iter().enumerate() {
            let id = NodeId::new(index as u64 + 1);
            let offsets = node.offsets.iter().map(|o| node.node_depth + o);
            let cells = crate::NodeCells {
                node: id,
                levels: node.levels.clone(),
                buildable: (offsets.clone().min().unwrap(), offsets.max().unwrap()),
            };
            driver
                .add_node(NodeInput {
                    id,
                    bounds: node.bounds,
                    points: node.levels.iter().map(|l| l.cells.len() as u64).sum(),
                    local_grid: unit,
                    cells: &cells,
                })
                .unwrap();
            cut.push(id);
        }
        driver.set_lod_mode(crate::LodMode::Cell);

        let mut pool = new_pool(h);
        let mut held = std::collections::HashMap::new();
        let mut table = HashSet::new();
        let (mut compared, mut hits, mut retained_changes) = (0, 0, 0);
        let mut mismatches = Vec::new();
        let mut previous: Option<Vec<(u32, u64)>> = None;
        let mut poses = 0;
        while poses < 60 {
            let Some((camera, target_px)) =
                random_pose(&mut rng, ([0.0; 3], [1.0; 3]), (0.5, 12.0))
            else {
                continue;
            };
            poses += 1;
            let view = crate::ViewProjection::new(
                camera,
                [WIDTH, HEIGHT],
                1.0,
                crate::VoxelBuildLimits::default(),
            );
            // Two thirds of the poses are judged after one or two frames only: retained frames
            // with the previous pose's records and pages and the new pose's active set.
            let frames = [1, 2, 300][poses % 3];
            let mut report = None;
            for _ in 0..frames {
                let mut target = PickTarget {
                    pool: &mut pool,
                    eye: camera.position(),
                    held: &mut held,
                    active: &mut table,
                };
                report = Some(
                    driver
                        .frame(&mut target, &cut, &view, f64::from(target_px.target_px))
                        .unwrap(),
                );
                if !driver.needs_frame() {
                    break;
                }
            }
            let report = report.unwrap();
            pool.set_page_frame(stamp(camera.position()));
            let lod_view = pool.lod_view().expect("cell mode sets a view");

            // The active depths per mask slot, as emitted.
            let mut emitted: std::collections::HashMap<u32, u64> = Default::default();
            for active in &report.plan.active {
                let node_depth = pool.node_record(active.mask_slot)[19];
                *emitted.entry(active.mask_slot).or_default() |=
                    1 << (u32::from(active.key.depth) - node_depth);
            }
            let mut snapshot: Vec<(u32, u64)> = emitted.iter().map(|(k, v)| (*k, *v)).collect();
            snapshot.sort_unstable();
            if report.plan.mask_updates.is_empty()
                && previous.as_ref().is_some_and(|p| *p != snapshot)
            {
                retained_changes += 1;
            }
            previous = Some(snapshot);

            for iy in 0..10 {
                for ix in 0..14 {
                    let pixel = [
                        (ix as f64 + 0.5) / 14.0 * f64::from(WIDTH),
                        (iy as f64 + 0.5) / 10.0 * f64::from(HEIGHT),
                    ];
                    let Some(ray) = camera_ray(&camera, pixel) else {
                        continue;
                    };
                    // Twin: the cells the active pages draw under the emitted depths.
                    let mut best: Option<(f64, usize)> = None;
                    let mut second: Option<f64> = None;
                    for active in &report.plan.active {
                        let record = pool.node_record(active.mask_slot);
                        let mut node = crate::cell_pick::node_of_record(record);
                        node.active = emitted[&active.mask_slot];
                        let side = 1.0 / (1_u64 << active.key.depth) as f64;
                        let (id, _) = scene
                            .iter()
                            .enumerate()
                            .find(|(i, _)| NodeId::new(*i as u64 + 1) == active.key.node)
                            .unwrap();
                        let lo: [u64; 3] = std::array::from_fn(|a| {
                            (scene[id].bounds.min()[a] * f64::from(1u32 << node.node_depth)).round()
                                as u64
                        });
                        for (slot, (held_node, depth, cells)) in &held {
                            if *held_node != active.key.node || *depth != active.key.depth {
                                continue;
                            }
                            // Only the pages of the active table count.
                            if !table.contains(slot) {
                                continue;
                            }
                            for (local, coord) in cells.iter().enumerate() {
                                let min = coord.map(|c| c as f64 * side);
                                let max = coord.map(|c| (c + 1) as f64 * side);
                                let Some(distance) = box_distance(&ray, min, max) else {
                                    continue;
                                };
                                let rel: [u32; 3] = std::array::from_fn(|a| {
                                    (coord[a] - (lo[a] << (*depth - node.node_depth))) as u32
                                });
                                if !crate::cell_lod::draws(
                                    &lod_view,
                                    &node,
                                    *depth,
                                    rel,
                                    side as f32,
                                ) {
                                    continue;
                                }
                                let global = *slot as usize * PAGE_SLOTS + local;
                                if best.is_none_or(|b| (distance, global) < b) {
                                    second = best.map(|b| b.0);
                                    best = Some((distance, global));
                                } else if second.is_none_or(|s| distance < s) {
                                    second = Some(distance);
                                }
                            }
                        }
                    }
                    let actual = pool_pick(h, &pool, ray).unwrap();
                    hits += usize::from(actual.is_some());
                    if let (Some(e), Some(second)) = (best, second)
                        && second - e.0 < 1.0e-5 * e.0
                    {
                        continue;
                    }
                    compared += 1;
                    let same = match (best, &actual) {
                        (None, None) => true,
                        (Some((distance, global)), Some(actual)) => {
                            actual.instance_index == global
                                && (actual.distance - distance).abs() <= 1.0e-6 * (1.0 + distance)
                        }
                        _ => false,
                    };
                    if !same {
                        mismatches.push(format!(
                            "pose {poses} pixel {pixel:?}: twin {best:?}, pick {actual:?}"
                        ));
                    }
                }
            }
        }
        println!(
            "# adapter {}: driver records pick {compared} compared, {hits} hits, \
             {retained_changes} retained frames with a moved active set",
            h.label
        );
        assert!(
            mismatches.is_empty(),
            "{:#?}",
            &mismatches[..mismatches.len().min(8)]
        );
        assert!(
            compared > 150 && hits > 50,
            "too few picks: {compared} {hits}"
        );
        assert!(
            retained_changes > 0,
            "no retained frame moved the active set"
        );
        true
    });
}
