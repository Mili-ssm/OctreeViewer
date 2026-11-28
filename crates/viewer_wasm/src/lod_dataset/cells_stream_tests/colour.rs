//! Colour generations (R3-Q33), plus the far-pose and p4 border probes that shared their section.

use super::*;

/// The synthetic dataset with blocks painted like an attribute mode paints them: while `paint`
/// is set, the red channel follows x and the others are fixed.
type PageCells = HashMap<(NodeId, u8, usize), u64>;

/// Frames until the ladder is at the target and the driver has nothing left, calling `check`
/// after every frame. Returns the nodes of the cut at rest.
fn run_to_rest(
    h: &mut Harness,
    view: &ViewProjection,
    mut check: impl FnMut(&Harness),
) -> Vec<NodeId> {
    let mut calm = 0;
    for _ in 0..2_000 {
        h.frame(view);
        check(h);
        if !h.dirty && h.stage == Some(2.0) && !h.driver.needs_frame() {
            calm += 1;
            if calm == 3 {
                return h.dataset.cells.pool_drawn().to_vec();
            }
        } else {
            calm = 0;
        }
    }
    panic!("no rest: {:?}", h.totals.stages);
}

#[test]
fn switching_colour_rebuilds_every_drawn_node_once_without_holes_or_mixed_generations() {
    let paint = Rc::new(std::cell::Cell::new(false));
    let mut h = painting_harness(Rc::clone(&paint));
    let view = synthetic_views().0;
    let mut phases: Vec<(u64, PageCells)> = Vec::new();
    let snapshot = |h: &Harness| (h.target.color_digest(), h.target.active_cells());
    let mut refs: Vec<PageCells> = Vec::new();
    // rgb -> attribute -> rgb -> attribute, each switch with `paint` and the colour generation
    // changed together (the binding does both).
    for (phase, on) in [false, true, false, true].into_iter().enumerate() {
        if phase > 0 {
            paint.set(on);
            assert!(h.dataset.set_cell_paint(on.then_some((0, 0))));
            h.dirty = true;
        }
        let (old, new) = match phase {
            0 | 1 => (None, None),
            _ => (Some(refs[phase - 1].clone()), Some(refs[phase - 2].clone())),
        };
        let cut = run_to_rest(&mut h, &view, |h| {
            // Holes are checked by the harness on every frame. A node never shows cells of two
            // generations: its pages are all the old ones or all the new ones.
            let (Some(old), Some(new)) = (&old, &new) else {
                return;
            };
            let mut seen: HashMap<NodeId, (bool, bool)> = HashMap::new();
            for (key, hash) in h.target.active_cells() {
                let entry = seen.entry(key.0).or_default();
                let (is_old, is_new) = (old.get(&key) == Some(&hash), new.get(&key) == Some(&hash));
                if is_old != is_new {
                    entry.0 |= is_old;
                    entry.1 |= is_new;
                }
            }
            for (node, (from_old, from_new)) in seen {
                assert!(
                    !(from_old && from_new),
                    "{node:?} draws old and new colours together"
                );
            }
        });
        assert!(!cut.is_empty());
        for node in &cut {
            // Built once at the start and once per switch.
            assert_eq!(
                h.totals.builds.get(node).copied(),
                Some(phase + 1),
                "phase {phase}: {node:?}"
            );
        }
        let (digest, cells) = snapshot(&h);
        phases.push((digest, cells.clone()));
        refs.push(cells);
    }
    assert_ne!(
        phases[0].0, phases[1].0,
        "attribute colours differ from rgb"
    );
    assert_eq!(phases[0].0, phases[2].0, "back to rgb gives the rgb image");
    assert_eq!(phases[1].0, phases[3].0, "the attribute image repeats");
}

#[test]
fn a_late_node_of_an_older_colour_generation_is_dropped_by_the_driver() {
    let mut h = synthetic_harness(1 << 20, 2.0);
    let view = synthetic_views().0;
    let config = h.config;
    let plan = |h: &mut Harness| {
        let mut nodes = Vec::new();
        for _ in 0..600 {
            let update = h
                .dataset
                .plan_cell_update(&view, 2.0, config, None, &mut h.blocks)
                .unwrap();
            nodes.extend(update.new_nodes);
            for anchor in update.stats.pending_page_anchors {
                let page = (h.pages)(anchor).unwrap();
                let _ = h.dataset.graft_page(page);
            }
            if nodes.len() >= 3 {
                break;
            }
        }
        nodes
    };
    let old = plan(&mut h);
    assert!(old.len() >= 3);
    h.dataset.set_cell_paint(Some((0, 0)));
    let new = plan(&mut h);
    assert!(new.len() >= 3);
    assert!(
        new[0].0 > old[0].0,
        "a colour change gives a newer generation"
    );
    let mut driver = driver_for(&h.dataset, u64::MAX);
    add_cell_nodes(&mut driver, new.iter().cloned()).unwrap();
    let bytes = driver.cpu_bytes();
    // The nodes of the old generation arrive late, for nodes the driver holds and for nodes it
    // does not: nothing changes.
    add_cell_nodes(&mut driver, old.iter().cloned()).unwrap();
    assert_eq!(driver.cpu_bytes(), bytes);
    for (generation, node) in &old {
        assert_eq!(
            driver.has_node(node.id),
            new.iter().any(|(_, n)| n.id == node.id),
            "{:?}",
            node.id
        );
        assert!(*generation < new[0].0);
    }
}

/// The built-in sample at a close pose: the leaves are tall cubes whose octants cross the eye plane
/// while the data lies in front of it. No node may draw its finest depth (cells of ~0.003 px)
/// there: the probe measures the data, not the node box.
#[test]
fn the_close_pose_of_the_sample_never_draws_sub_pixel_cells_in_cell_mode() {
    const MAX_DEPTH: u8 = 14;
    let (mut h, origin, scale) = sample_harness();
    let viewport = [870, 750];
    // Measured at this pose: probing the data wants depth 11-13 in these leaves (cells near the
    // px target), while probing the node box wants the finest depth, 19 (cells of ~0.003 px).
    // 14 is one level of margin over the former and far from the latter.
    let local = |p: [f64; 3]| -> [f64; 3] { std::array::from_fn(|a| (p[a] - origin[a]) * scale) };
    let mk = |eye: [f64; 3], target: [f64; 3]| {
        let root = h.dataset.metadata.hierarchy().stats().root_bounds;
        let mut camera = OrbitCamera::default();
        assert!(camera.fit(
            SceneBounds {
                min: local(root.min()),
                max: local(root.max())
            },
            viewport
        ));
        assert!(camera.set_pose(local(eye), local(target)));
        ViewProjection::new(camera, viewport, scale, VoxelBuildLimits::default())
    };
    let target = [31.958_496_093_75, 31.958_496_093_75, 1.999_511_718_75];
    let v0 = mk(
        [
            31.958_496_093_75,
            75.781_277_364_260_28,
            77.902_795_408_251_59,
        ],
        target,
    );
    let v4 = mk(
        [
            31.958_496_093_75,
            38.759_519_607_345_27,
            6.118_357_964_164_779,
        ],
        target,
    );
    h.settle(&v0, 3, 5000);
    h.camera_moved();
    h.settle(&v4, 3, 5000);
    let mut seen = std::collections::BTreeSet::new();
    for (node, wanted, drawn) in &h.last_entries {
        if [13, 14, 17, 18].contains(&node.get()) {
            for o in 0..8 {
                let (Some(w), Some(d)) = (wanted[o], drawn[o]) else {
                    continue;
                };
                seen.insert(node.get());
                assert!(
                    w <= MAX_DEPTH && d <= MAX_DEPTH,
                    "node {} octant {o} wants {w} and draws {d}",
                    node.get()
                );
            }
        }
    }
    assert_eq!(
        seen.into_iter().collect::<Vec<_>>(),
        [13, 14, 17, 18],
        "every leaf must have an octant with a wanted and drawn depth, or the check is vacuous"
    );
}

/// The sample from a close pose whose leaves hold the eye (nodes 14 and 17): their octants that
/// straddle the eye plane want a far finer depth than the ones wholly in front. The node's active
/// depths must stay contiguous up to that wish, or the per-cell rule jumps from the coarse range
/// to a depth whose cells measure a fraction of a pixel (black holes).
#[test]
fn a_node_holding_the_eye_keeps_contiguous_active_depths_and_no_sub_pixel_cells() {
    let (mut h, origin, scale) = sample_harness();
    let viewport = [870, 750];
    let local = |p: [f64; 3]| -> [f64; 3] { std::array::from_fn(|a| (p[a] - origin[a]) * scale) };
    let mk = |eye: [f64; 3], target: [f64; 3]| {
        let root = h.dataset.metadata.hierarchy().stats().root_bounds;
        let mut camera = OrbitCamera::default();
        assert!(camera.fit(
            SceneBounds {
                min: local(root.min()),
                max: local(root.max())
            },
            viewport
        ));
        assert!(camera.set_pose(eye, target));
        ViewProjection::new(camera, viewport, scale, VoxelBuildLimits::default())
    };
    // Local-frame poses: the eye 9 units from the target, 1.4 above it.
    let target = [50.0, 50.0, 3.22];
    let far = mk([50.0, 90.0, 40.0], target);
    let p2 = mk([50.0, 59.11, 4.60], target);
    h.settle(&far, 3, 5000);
    h.camera_moved();
    h.settle(&p2, 3, 5000);
    let ctx = h.cell_ctx.as_ref().expect("cell mode context");
    let grid = h.dataset.cell_grid();
    let hierarchy = h.dataset.metadata.hierarchy();
    let mut seen = 0;
    let mut checked = 0;
    for (node, depths, range) in &h.last_levels {
        let mut depths = depths.clone();
        depths.sort_unstable();
        if ![14, 17].contains(&node.get()) {
            continue;
        }
        let (Some(&first), Some(&last)) = (depths.first(), depths.last()) else {
            continue;
        };
        seen += 1;
        let lo = range.map_or(first, |(lo, _)| lo.max(first));
        let missing: Vec<u8> = (lo..=last).filter(|d| !depths.contains(d)).collect();
        assert!(
            missing.is_empty(),
            "node {} active {depths:?} range {range:?}: {missing:?} missing",
            node.get()
        );
        // No cell of the node's lattice is drawn below half the pixel target, unless it is the
        // node's coarsest active depth (nothing coarser to fall back on).
        let bounds = hierarchy.node(*node).unwrap().bounds();
        let node_depth = representative_depth(&grid, bounds).saturating_sub(3);
        let rel_lo: [f64; 3] =
            std::array::from_fn(|a| (bounds.min()[a] - origin[a]) * scale - ctx.eye[a]);
        let lod = NodeLod {
            anchor: rel_lo.map(|v| v as f32),
            node_depth,
            active: depths.iter().fold(0_u64, |b, d| b | 1 << (d - node_depth)),
            mask: [0; 8],
        };
        let side = ctx.root_side / (1_u64 << node_depth) as f64 * ctx.scale;
        for n in 0..343_u16 {
            let at = [n % 7, n / 7 % 7, n / 49];
            let rel: [f64; 3] = std::array::from_fn(|a| {
                let t = 0.01 + 0.98 * (f64::from(at[a]) + 0.37) / 6.37;
                rel_lo[a] + (bounds.max()[a] - bounds.min()[a]) * scale * t
            });
            let Some(depth) = drawn_depth_at(&ctx.lod, &lod, rel, rel_lo, side) else {
                continue;
            };
            if depth == first {
                continue;
            }
            let cells = (1_u64 << (depth - node_depth)) as f64;
            let cell_side = side / cells;
            let cell: [f64; 3] = std::array::from_fn(|a| {
                ((rel[a] - rel_lo[a]) / cell_side)
                    .floor()
                    .clamp(0.0, cells - 1.0)
            });
            let cell_min: [f32; 3] =
                std::array::from_fn(|a| (rel_lo[a] + cell[a] * cell_side) as f32);
            let px = edge_px(&ctx.lod, cell_min, cell_side as f32);
            checked += 1;
            assert!(
                px >= 2.0,
                "node {} draws depth {depth} at {px} px (active {depths:?})",
                node.get()
            );
        }
    }
    assert_eq!(seen, 2, "nodes 14 and 17 are drawn");
    assert!(checked > 100, "too few lattice samples measured: {checked}");
}

/// Diagnostic (prints, asserts nothing): the built-in sample at pose P4 of the web demo (close
/// pose: from the initial fit, 8 wheel steps of -0.3 zoom each, then a right drag of 120 px
/// upward on an 870 x 750 canvas). For every face of every node of the cut it takes a grid of
/// point pairs straddling the face, asks the twin of the GPU rule (`drawn_depth_at`) which node
/// draws each side and at which depth, and prints, the nodes' active, resident,
/// wanted and drawn depths and ranges, a histogram of the level differences of the pairs where
/// both samples are on screen, and the worst pairs with their screen position.
/// Run: `cargo test --offline --profile fast -p viewer_wasm p4_border_levels -- --ignored --nocapture`.
#[test]
#[ignore = "diagnostic: prints the level difference across the P4 node borders"]
fn p4_border_levels() {
    let _ = p4_border_levels_under();
}

/// ADR 0055 plan, task 4: at P4 node 14 (beside node 8, which draws level 10 across their border)
/// can draw level 10 because its `lo` comes from `node_view_range`, and no visible border pair
/// differs by more than one level. `node_far_depth` gave node 14 the range 11..13.
#[test]
fn at_p4_node_14_can_draw_the_level_its_border_needs() {
    let (histogram, node_14) = p4_border_levels_under();
    let (lo, _) = node_14.expect("node 14 has a range at P4");
    assert!(lo <= 10, "node 14 range.lo {lo} > 10");
    assert!(
        histogram.keys().all(|diff| *diff <= 1),
        "border pairs by level difference: {histogram:?}"
    );
}

/// Returns the histogram of the visible border pairs by level difference, and the range of node 14.
fn p4_border_levels_under() -> (std::collections::BTreeMap<u8, usize>, Option<(u8, u8)>) {
    type Group = (usize, [f64; 2], [f64; 2]);
    const N: usize = 48;
    let (mut h, origin, scale) = sample_harness();
    let viewport = [870, 750];
    let local = |p: [f64; 3]| -> [f64; 3] { std::array::from_fn(|a| (p[a] - origin[a]) * scale) };
    // The host fits the camera to the data extent (`LodDataset::fit_camera`).
    let root = h.dataset.metadata.hierarchy().stats().data_bounds;
    let mut camera = OrbitCamera::default();
    assert!(camera.fit(
        SceneBounds {
            min: local(root.min()),
            max: local(root.max())
        },
        viewport
    ));
    let initial = ViewProjection::new(camera, viewport, scale, VoxelBuildLimits::default());
    // The web host sums the wheel deltas (deltaY * 0.001) and the pointer deltas (normalised by
    // the canvas size) of a frame and applies zoom, then orbit.
    let mut p4_camera = camera;
    assert!(p4_camera.zoom(-0.3 * 8.0));
    assert!(p4_camera.orbit(0.0, -120.0 / 750.0));
    let p4 = ViewProjection::new(p4_camera, viewport, scale, VoxelBuildLimits::default());
    println!(
        "== P4 eye {:?} target {:?} (dataset frame of the close-pose test: eye \
         [31.958, 38.760, 6.118] target [31.958, 31.958, 1.9995])",
        p4_camera.position(),
        p4_camera.target()
    );
    h.settle(&initial, 3, 5000);
    h.camera_moved();
    h.settle(&p4, 3, 5000);
    far_diagnosis(&h, &p4, &[14, 8]);

    let ctx = h.cell_ctx.as_ref().expect("cell mode context");
    let grid = h.dataset.cell_grid();
    let hierarchy = h.dataset.metadata.hierarchy();
    let nodes = &h.cell_nodes;
    let to_eye = |p: [f64; 3]| -> [f64; 3] {
        std::array::from_fn(|a| (p[a] - ctx.origin[a]) * ctx.scale - ctx.eye[a])
    };
    // The unique drawer of a point: (node index, depth).
    let drawer = |p: [f64; 3]| -> Option<(usize, u8)> {
        let rel = to_eye(p);
        let mut found = None;
        for (j, node) in nodes.iter().enumerate() {
            if !(0..3).all(|a| p[a] >= node.min[a] && p[a] <= node.max[a]) {
                continue;
            }
            if let Some(depth) = drawn_depth_at(&ctx.lod, &node.lod, rel, node.lo, node.side) {
                found = Some((j, depth));
            }
        }
        found
    };
    // Projected edge of the cell of node `j` at `depth` that holds `rel`.
    let cell_px = |j: usize, depth: u8, rel: [f64; 3]| -> f32 {
        let node = &nodes[j];
        let level = depth - node.lod.node_depth;
        let cells = (1_u64 << level) as f64;
        let side = node.side / cells;
        let min: [f32; 3] = std::array::from_fn(|a| {
            (node.lo[a]
                + ((rel[a] - node.lo[a]) / side)
                    .floor()
                    .clamp(0.0, cells - 1.0)
                    * side) as f32
        });
        edge_px(&ctx.lod, min, side as f32)
    };
    let screen = |rel: [f64; 3]| -> [f64; 2] {
        let dot = |v: [f32; 3]| (0..3).map(|a| rel[a] * f64::from(v[a])).sum::<f64>();
        let depth = dot(ctx.lod.forward);
        let focal = f64::from(ctx.lod.focal);
        [
            ctx.viewport[0] * 0.5 + dot(ctx.lod.right) * focal / depth,
            ctx.viewport[1] * 0.5 - dot(ctx.lod.up) * focal / depth,
        ]
    };

    println!("-- nodes of the cut ({})", nodes.len());
    for (j, node) in nodes.iter().enumerate() {
        let (_, depths, range) = &h.last_levels[j];
        let (_, wanted, drawn) = &h.last_entries[j];
        let mut depths = depths.clone();
        depths.sort_unstable();
        let resident: Vec<u8> = (node.lod.node_depth..=node.lod.node_depth + MAX_NODE_SPAN)
            .filter(|d| {
                h.driver
                    .pool()
                    .is_resident(h.driver.pool().key(node.id, *d))
            })
            .collect();
        let bounds = hierarchy.node(node.id).unwrap().bounds();
        println!(
            "node {:>3} depth {} bounds {:.1?}..{:.1?} active {depths:?} resident {resident:?} \
             range {range:?}\n          wanted {wanted:?}\n          drawn {drawn:?}",
            node.id.get(),
            representative_depth(&grid, bounds).saturating_sub(3),
            bounds.min(),
            bounds.max(),
        );
    }

    // Only the data matters on screen: the cells above and below the point cloud draw nothing.
    let data = h.dataset.metadata.hierarchy().stats().data_bounds;
    println!("data bounds {:?}..{:?}", data.min(), data.max());
    let mut bands: std::collections::BTreeMap<usize, std::collections::BTreeMap<u8, usize>> =
        std::collections::BTreeMap::default();
    // `P4_DUMP=<dir>` writes every visible pair as `x y z depth_a depth_b node_a node_b` lines.
    let mut dump = String::new();
    let mut histogram: std::collections::BTreeMap<u8, usize> =
        std::collections::BTreeMap::default();
    // (node a, depth a, node b, depth b) -> (pairs, min xy, max xy)
    let mut groups: std::collections::BTreeMap<(u64, u8, u64, u8), Group> =
        std::collections::BTreeMap::default();
    for node in nodes {
        for axis in 0..3 {
            let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
            for upper in [false, true] {
                for i in 0..N {
                    for j in 0..N {
                        let extent: [f64; 3] = std::array::from_fn(|a| node.max[a] - node.min[a]);
                        let eps = extent[axis] * 1e-4;
                        let mut inside = [0.0; 3];
                        inside[u] = node.min[u] + extent[u] * (i as f64 + 0.5) / N as f64;
                        inside[v] = node.min[v] + extent[v] * (j as f64 + 0.5) / N as f64;
                        let mut outside = inside;
                        let face = if upper {
                            node.max[axis]
                        } else {
                            node.min[axis]
                        };
                        let sign = if upper { 1.0 } else { -1.0 };
                        inside[axis] = face - sign * eps;
                        outside[axis] = face + sign * eps;
                        let (Some((ja, da)), Some((jb, db))) = (drawer(inside), drawer(outside))
                        else {
                            continue;
                        };
                        if ja == jb {
                            continue;
                        }
                        if !(0..3).all(|a| inside[a] >= data.min()[a] && inside[a] <= data.max()[a])
                        {
                            continue;
                        }
                        let (ra, rb) = (to_eye(inside), to_eye(outside));
                        if !(ctx.on_screen(ra) && ctx.on_screen(rb)) {
                            continue;
                        }
                        *histogram.entry(da.abs_diff(db)).or_default() += 1;
                        let at = screen(ra);
                        *bands
                            .entry((at[1] / 50.0).floor().max(0.0) as usize * 50)
                            .or_default()
                            .entry(da.abs_diff(db))
                            .or_default() += 1;
                        let _ = writeln!(
                            dump,
                            "{:.1} {:.1} {:.3} {da} {db} {} {} {:.2} {:.2}",
                            at[0],
                            at[1],
                            inside[2],
                            nodes[ja].id.get(),
                            nodes[jb].id.get(),
                            cell_px(ja, da, ra),
                            cell_px(jb, db, rb)
                        );
                        let entry = groups
                            .entry((nodes[ja].id.get(), da, nodes[jb].id.get(), db))
                            .or_insert((0, at, at));
                        entry.0 += 1;
                        for k in 0..2 {
                            entry.1[k] = entry.1[k].min(at[k]);
                            entry.2[k] = entry.2[k].max(at[k]);
                        }
                    }
                }
            }
        }
    }
    println!("-- visible border pairs by level difference: {histogram:?}");
    if let Ok(dir) = std::env::var("P4_DUMP") {
        std::fs::write(format!("{dir}/pairs.txt"), dump).unwrap();
    }
    println!("-- level differences per 50 px screen row band: {bands:?}");
    let mut worst: Vec<_> = groups.iter().collect();
    worst.sort_by_key(|((_, da, _, db), (n, ..))| {
        (std::cmp::Reverse(da.abs_diff(*db)), std::cmp::Reverse(*n))
    });
    for ((a, da, b, db), (n, lo, hi)) in worst.into_iter().take(25) {
        println!(
            "diff {} : node {a} @ depth {da} | node {b} @ depth {db} : {n} pairs, screen x {:.0}..{:.0} y {:.0}..{:.0}",
            da.abs_diff(*db),
            lo[0],
            hi[0],
            lo[1],
            hi[1]
        );
    }
    let node_14 = nodes
        .iter()
        .position(|node| node.id.get() == 14)
        .and_then(|j| h.last_levels[j].2);
    (histogram, node_14)
}
