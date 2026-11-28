//! R3-Q21: the image only refines.

use super::*;

/// Does the mask of `node` hide `point`?
fn masked_at(
    plan: &FramePlan,
    nodes: &HashMap<NodeId, PoolNode>,
    node: u64,
    point: [f64; 3],
) -> bool {
    let entry = plan
        .entries
        .iter()
        .find(|entry| entry.node == NodeId::new(node))
        .expect("entry");
    let at = cell_of(nodes[&entry.node].bounds, point).expect("point in node");
    entry.mask[at[2]] >> (at[0] + 8 * at[1]) & 1 == 1
}

/// Resident levels of the nodes, as the cut of the next `step` reads them (caps applied).
fn resident_levels(rig: &Rig) -> HashMap<NodeId, Vec<u8>> {
    rig.nodes
        .iter()
        .map(|(id, node)| {
            let levels = capped(&node.stored, rig.pool.caps.get(id))
                .into_iter()
                .filter(|depth| rig.pool.is_resident(rig.pool.key(*id, *depth)))
                .collect();
            (*id, levels)
        })
        .collect()
}

/// Depth the plan really draws at `point`: the unhidden octant of an entry that has a resident
/// level in `active` there (geometry computed here). `resident` is `resident_levels` taken
/// before the step: each drawn depth must be one of them, and no point may be drawn by two
/// entries.
fn drawn_at(
    plan: &FramePlan,
    nodes: &HashMap<NodeId, PoolNode>,
    resident: &HashMap<NodeId, Vec<u8>>,
    point: [f64; 3],
) -> Option<u8> {
    let depths: Vec<u8> = plan
        .entries
        .iter()
        .filter_map(|entry| {
            let at = cell_of(nodes[&entry.node].bounds, point)?;
            if entry.mask[at[2]] >> (at[0] + 8 * at[1]) & 1 == 1 {
                return None;
            }
            let octant = ((at[0] / 4) << 2) | ((at[1] / 4) << 1) | (at[2] / 4);
            plan.active
                .iter()
                .any(|a| a.key.node == entry.node && a.octant_bits >> octant & 1 == 1)
                .then(|| entry.drawn[octant])
                .flatten()
                .inspect(|depth| {
                    // The octant may be stepping (ADR 0053): it draws a resident level, and the
                    // stepping rule has its own tests.
                    assert!(
                        resident[&entry.node].contains(depth),
                        "node {:?} octant {octant}",
                        entry.node
                    );
                })
        })
        .collect();
    assert!(depths.len() <= 1, "{point:?} is drawn by {depths:?}");
    depths.first().copied()
}

/// `rig.run` that also returns the resident levels the last frame planned with.
fn run_with_resident(
    rig: &mut Rig,
    frames: usize,
    cut: &[NodeId],
    camera: &OrbitCamera,
    target: f64,
) -> (FramePlan, HashMap<NodeId, Vec<u8>>) {
    rig.run(frames - 1, cut, camera, target);
    let resident = resident_levels(rig);
    (rig.step(cut, camera, target), resident)
}

/// Region A drew depth 6 (8 px) and refines: in the fallback variant the cut becomes its eight
/// children (node 2 is a fallback ancestor), in the other it keeps node 2 and adds child 10.
fn only_refine(in_cut: bool) {
    let nodes = two_regions();
    let mut rig = Rig::new(nodes.clone(), 512, 3, 1);
    let cam = camera("fit");
    let (before, resident) = run_with_resident(&mut rig, 60, &ids(&[2, 3]), &cam, 8.0);
    assert_eq!(before.apply, CutApply::Applied);
    assert_eq!(depth_of(&before, 2), [Some(6); 8], "setup");
    let points = sample_points(nodes[&NodeId::new(2)].bounds);
    let base: Vec<Option<u8>> = points
        .iter()
        .map(|p| drawn_at(&before, &nodes, &resident, *p))
        .collect();
    assert!(base.iter().all(|d| *d == Some(6)), "setup");
    let (cut, watched): (Vec<NodeId>, Vec<u64>) = if in_cut {
        (ids(&[2, 10, 3]), vec![10])
    } else {
        let mut cut = ids(&region_children(10));
        cut.push(NodeId::new(3));
        (cut, region_children(10))
    };
    let (mut saw_held, mut saw_taken_over) = (false, false);
    for frame in 0..40 {
        let ready: Vec<bool> = watched.iter().map(|c| resident_at(&rig, *c, 6)).collect();
        let coarse: Vec<bool> = watched.iter().map(|c| resident_at(&rig, *c, 5)).collect();
        let resident = resident_levels(&rig);
        let plan = rig.step(&cut, &cam, 8.0);
        assert_eq!(plan.apply, CutApply::Applied, "frame {frame}");
        assert_no_holes(&plan, &nodes);
        assert!(!plan.counters.descendant_too_fine, "frame {frame}");
        assert_eq!(plan.counters.hold_unsettled, 0, "frame {frame}");
        // No zone is ever drawn coarser than before.
        for (point, was) in points.iter().zip(&base) {
            let now = drawn_at(&plan, &nodes, &resident, *point);
            assert!(now >= *was, "frame {frame}: {point:?} {now:?} < {was:?}");
        }
        // A child draws exactly when its level 6 (the ancestor's depth and its wanted one) is
        // resident, and a coarser one is held (it neither draws nor hides node 2).
        let mut missing = 0;
        let mut held = 0;
        for (index, child) in watched.iter().enumerate() {
            let drawn = depth_of(&plan, *child);
            assert_eq!(
                drawn[0].is_some(),
                ready[index],
                "frame {frame} node {child}"
            );
            let centre = sample_points(nodes[&NodeId::new(*child)].bounds)[13];
            if has_entry(&plan, 2) {
                assert_eq!(
                    masked_at(&plan, &nodes, 2, centre),
                    ready[index],
                    "frame {frame} node {child}"
                );
            }
            missing += usize::from(!ready[index]);
            held += usize::from(coarse[index] && !ready[index]);
            saw_taken_over |= ready[index];
        }
        saw_held |= held > 0;
        assert_eq!(plan.counters.held_nodes as usize, held, "frame {frame}");
        assert_eq!(plan.counters.nodes_without_level as usize, missing);
        assert_eq!(has_entry(&plan, 2), in_cut || missing > 0, "frame {frame}");
        if missing == 0 {
            assert_eq!(plan.counters.stages, 0);
            assert_eq!(plan.counters.fallback_pages, 0);
        } else {
            assert!(plan.counters.stages as usize >= 8 * held, "frame {frame}");
            let pages = rig.pages(rig.pool.key(NodeId::new(2), 6)) as u32;
            let covers = if in_cut { held > 0 } else { true };
            assert_eq!(plan.counters.fallback_pages, if covers { pages } else { 0 });
        }
    }
    assert!(saw_held && saw_taken_over);
}

#[test]
fn a_child_whose_first_level_is_coarser_than_the_ancestor_drew_is_held() {
    only_refine(false);
}

#[test]
fn a_child_in_the_cut_with_its_ancestor_is_held_the_same_way() {
    only_refine(true);
}

/// Region A drew depth 6 at 8 px; its children (which also store depth 4) replace it at
/// `target` px with room for `capacity` pages. A child is held when what it would draw is below
/// `min(what node 2 drew, what the child wants)`; the test recomputes that from the resident
/// levels (`resident_depth`) and the plan's wanted depths, and checks the image never gets
/// coarser than the ancestor drew while the children take over within `limit` frames.
fn takes_over_at(capacity: u32, target: f64, expect_held: bool, limit: u64) {
    let nodes = two_regions_with_coarse_children();
    let mut rig = Rig::new(nodes.clone(), capacity, 3, 1);
    let cam = camera("fit");
    let mut previous = rig.run(60, &ids(&[2, 3]), &cam, 8.0);
    assert_eq!(depth_of(&previous, 2), [Some(6); 8], "setup");
    let mut cut = ids(&region_children(10));
    cut.push(NodeId::new(3));
    let (mut gone, mut saw_held) = (None, false);
    for frame in 0..60 {
        let resident = resident_levels(&rig);
        let plan = rig.step(&cut, &cam, target);
        assert_eq!(plan.apply, CutApply::Applied, "frame {frame}");
        assert_no_holes(&plan, &nodes);
        let mut held = 0;
        for child in region_children(10) {
            // Node 2 drew this child's zone at its octant (child bits are x, y, z from the low).
            let index = child - 10;
            let octant = ((index & 1) << 2 | (index >> 1 & 1) << 1 | (index >> 2 & 1)) as usize;
            let ancestor = has_entry(&previous, 2).then(|| depth_of(&previous, 2)[octant]);
            let entry = plan.entries.iter().find(|e| e.node.get() == child).unwrap();
            let id = entry.node;
            let wanted = entry.wanted[0].unwrap();
            let would = crate::resident_depth(wanted, &resident[&id]).map(|(d, _)| d);
            let short = match (ancestor.flatten(), would) {
                (Some(a), Some(d)) => d < a.min(wanted),
                _ => false,
            };
            held += usize::from(short);
            assert_eq!(entry.drawn[0].is_some(), would.is_some() && !short);
        }
        assert_eq!(plan.counters.held_nodes as usize, held, "frame {frame}");
        saw_held |= held > 0 && has_entry(&plan, 2);
        if !has_entry(&plan, 2) && gone.is_none() {
            gone = Some(frame);
            assert_eq!(plan.counters.nodes_without_level, 0);
        }
        previous = plan;
    }
    assert_eq!(saw_held, expect_held, "a child was held while node 2 drew");
    assert!(gone.is_some_and(|frame| frame < limit), "{gone:?}");
}

#[test]
fn a_child_that_wants_less_than_the_ancestor_drew_takes_over_at_its_wanted_level() {
    // At 16 px the children want depth 5 < the 6 the ancestor drew; the ancestor itself wants 4
    // now. While its level 4 is not resident it keeps drawing 6 (R3-Q21: the image only refines,
    // a finer level meets the target), so the children whose only level is 4 are held behind it
    // until their 5 arrives, and then take over at their wanted level.
    takes_over_at(512, 16.0, true, 15);
}

#[test]
fn a_child_whose_fit_cap_is_below_the_ancestor_depth_takes_over_at_the_cap() {
    // The pool cannot hold depth 6 for the farthest children: their cap (5) is the threshold,
    // and the nearest ones still want 6.
    takes_over_at(40, 8.0, true, 30);
}

#[test]
fn the_hold_follows_what_would_be_drawn_and_not_mere_residency() {
    // Node 10 wants 6 (the depth node 2 drew). With only 5 resident (the closest level, below
    // 6) it is held; levels of the node that are not resident do not count.
    let nodes = two_regions();
    let mut rig = Rig::new(nodes.clone(), 512, 3, 1);
    let cam = camera("fit");
    let before = rig.run(60, &ids(&[2, 3]), &cam, 8.0);
    assert_eq!(depth_of(&before, 2), [Some(6); 8], "setup");
    let mut cut = ids(&region_children(10));
    cut.push(NodeId::new(3));
    rig.held.insert(NodeId::new(10));
    seed_resident(&mut rig.pool, 10, &[5]);
    let points = sample_points(nodes[&NodeId::new(10)].bounds);
    for frame in 0..12 {
        let resident = resident_levels(&rig);
        let plan = rig.step(&cut, &cam, 8.0);
        assert_eq!(plan.apply, CutApply::Applied, "frame {frame}");
        assert!(rig.pool.hold.contains(&NodeId::new(10)), "frame {frame}");
        assert_eq!(depth_of(&plan, 10), [None; 8]);
        for point in &points {
            let now = drawn_at(&plan, &nodes, &resident, *point);
            assert!(now >= Some(6), "frame {frame}");
        }
    }
    rig.held.clear();
    let mut taken_over = false;
    for _ in 0..12 {
        let resident = resident_levels(&rig);
        let plan = rig.step(&cut, &cam, 8.0);
        assert!(
            points
                .iter()
                .all(|p| drawn_at(&plan, &nodes, &resident, *p) >= Some(6))
        );
        taken_over |= depth_of(&plan, 10)[0] == Some(6);
    }
    assert!(taken_over);
}

#[test]
fn the_closest_resident_level_decides_whether_a_node_is_held() {
    // Node 2 drew 6 and node 10 wants 6.
    for (resident, held) in [
        (vec![5_u8], true),
        (vec![5, 7], false),
        (vec![4, 5], true),
        (vec![6], false),
        (vec![5, 6], false),
        (vec![7], false),
        (vec![7, 9], false),
    ] {
        let mut rig = rig_with_coarse_children(None);
        let key = rig.pool.key(NodeId::new(10), 5);
        rig.pool.levels.remove(&key);
        seed_resident(&mut rig.pool, 10, &resident);
        let (eval, _) = eval_with(&rig, &ids(&[10]), 6);
        assert_eq!(eval.hold.contains(&NodeId::new(10)), held, "{resident:?}");
    }
}

/// A rig that drew `[2, 3]` (node 2 at depth 6) and whose children 10 and 17 of node 2 hold a
/// resident level 5.
fn rig_with_coarse_children(extra: Option<(u64, PoolNode)>) -> Rig {
    let mut nodes = two_regions();
    nodes.extend(extra.map(|(id, node)| (NodeId::new(id), node)));
    let mut rig = Rig::new(nodes, 512, 3, 1);
    rig.run(60, &ids(&[2, 3]), &camera("fit"), 8.0);
    for child in [10, 17] {
        seed_resident(&mut rig.pool, child, &[5]);
    }
    rig.pool.residency_epoch += 1;
    rig
}

#[test]
fn only_a_node_with_an_ancestor_the_fallback_would_use_can_be_held() {
    let rig = rig_with_coarse_children(None);
    let (eval, fallback) = eval_with(&rig, &ids(&[10]), 7);
    assert!(eval.hold.contains(&NodeId::new(10)));
    assert_eq!(
        fallback,
        ids(&[2]),
        "the held node's ancestor is in the real cut"
    );
    // A resident cut node four levels below node 2 rules it out as a fallback (it would make the
    // real cut too fine), so the node cannot be held on it either: it would draw nothing.
    let deep = (
        50,
        PoolNode {
            bounds: cube([ORIGIN[0] + SIDE / 4.0, ORIGIN[1], ORIGIN[2]], SIDE / 32.0),
            stored: vec![8, 9, 10],
            buildable: (8, 10),
            points: 2000,
            octant_data_bounds: None,
        },
    );
    let mut rig = rig_with_coarse_children(Some(deep));
    seed_resident(&mut rig.pool, 50, &[8]);
    let (eval, fallback) = eval_with(&rig, &ids(&[10, 50]), 7);
    assert!(
        eval.hold.is_empty() && eval.settle.is_empty(),
        "{:?}",
        eval.hold
    );
    assert!(fallback.is_empty());
}

#[test]
fn in_cell_mode_the_hold_threshold_is_the_finest_depth_active_in_the_ancestor() {
    // The GPU picks the depth per cell among the active ones of a node, so node 2 may have drawn
    // its finest active depth (6) over an octant whose nominal depth is 5. Child 10 stores 5 and
    // 6, has only 5 resident and wants 6: short of what was really drawn there, held.
    let nodes = two_regions_with_coarse_children();
    let mut rig = Rig::new(nodes.clone(), 512, 3, 1);
    rig.run(60, &ids(&[2, 3]), &camera("fit"), 8.0);
    let child = nodes[&NodeId::new(10)].clone();
    seed_resident(&mut rig.pool, 10, &[child.stored[0]]);
    rig.pool.residency_epoch += 1;
    let nominal = child.stored[0];
    rig.pool
        .applied_drawn
        .insert(NodeId::new(2), [Some(nominal); 8]);
    rig.pool.applied_finest.insert(NodeId::new(2), nominal + 1);
    let (eval, _) = eval_with(&rig, &ids(&[10]), nominal + 1);
    assert!(eval.hold.contains(&NodeId::new(10)));
}

#[test]
fn the_hold_threshold_counts_the_d5_depth_above_the_run_of_the_ancestor() {
    // Node 2 drew its run at the nominal depth (5) and its D5 depth two levels above (7): the
    // GPU can draw 7 over a cell whose own run refines, so child 10 (5 resident, wants 6) would
    // coarsen it and is held. The run alone (B3) would say 5 drew before and release it; that
    // is only for the anchor of `node_run`, and `before` for the anchor stays without D5.
    let nodes = two_regions_with_coarse_children();
    let mut rig = Rig::new(nodes.clone(), 512, 3, 1);
    rig.run(60, &ids(&[2, 3]), &camera("fit"), 8.0);
    let child = nodes[&NodeId::new(10)].clone();
    seed_resident(&mut rig.pool, 10, &[child.stored[0]]);
    rig.pool.residency_epoch += 1;
    let nominal = child.stored[0];
    rig.pool
        .applied_drawn
        .insert(NodeId::new(2), [Some(nominal); 8]);
    rig.pool.applied_finest.insert(NodeId::new(2), nominal + 2);
    rig.pool
        .applied_run
        .insert(NodeId::new(2), (nominal, nominal));
    let id = NodeId::new(10);
    let (eval, _) = eval_with(&rig, &ids(&[10]), nominal + 1);
    assert!(eval.hold.contains(&id), "the D5 depth drew over it");
    assert_eq!(
        eval.before.get(&id),
        Some(&nominal),
        "the anchor's `before` does not count D5"
    );
    rig.pool.applied_finest.insert(NodeId::new(2), nominal);
    let (eval, _) = eval_with(&rig, &ids(&[10]), nominal + 1);
    assert!(!eval.hold.contains(&id), "without the D5 depth it releases");
}

/// What the judges say about one case. `eval_holds` is the production decision (`eval_hold`),
/// `old_holds` the per-octant hold of before (`eval_refine_hold`), `gate_releases` is `run_covers`
/// on the real run, `oracle` releases it (the twin).
struct Verdict {
    eval_holds: bool,
    old_holds: bool,
    gate_releases: bool,
    oracle: Result<(), String>,
    run: Option<Run>,
}

/// Where `eval_refine_hold` and `run_covers` part ways.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Parts {
    /// `eval_refine_hold` holds, the gate (and the twin) release: the incumbent is conservative.
    EvalHoldsLonger,
    /// `eval_refine_hold` releases, the gate holds: the node would coarsen a zone.
    EvalReleasesEarly,
}

fn parts(verdict: &Verdict) -> Option<Parts> {
    match (verdict.old_holds, verdict.gate_releases) {
        (true, true) => Some(Parts::EvalHoldsLonger),
        (false, false) => Some(Parts::EvalReleasesEarly),
        _ => None,
    }
}

/// Zoom in: node 2 drew with nominal depth `finest - 1` and finest active depth `finest` (`low`:
/// half of its octants drew `finest - 2`, so its active set is `finest - 2..=finest`); its child
/// 10 (stored 4..=7) enters the cut with the resident levels `resident` and wants `wish`.
fn zoom_in_case((finest, low, wish, resident): (u8, bool, u8, &[u8]), far: Far) -> Verdict {
    let mut rig = Rig::new(two_regions_with_coarse_children(), 512, 3, 1);
    rig.run(60, &ids(&[2, 3]), &camera("fit"), 8.0);
    set_applied(&mut rig, 2, finest);
    if low {
        let drawn = std::array::from_fn(|o| Some(finest - 1 - u8::from(o >= 4)));
        rig.pool.applied_drawn.insert(NodeId::new(2), drawn);
    }
    set_resident(&mut rig, 10, resident);
    let probe = refine_probe(&mut rig, 10, wish, far);
    let grid = grid();
    let ancestor_depth = CellPool::node_depth(&grid, rig.nodes[&NodeId::new(2)].bounds);
    assert_eq!(
        probe.node_depth,
        ancestor_depth + 1,
        "node 10 is a child of 2"
    );
    let before = Owner {
        node_depth: ancestor_depth,
        lo: [0.0, 0.0, 10.0],
        active: (finest - 1 - u8::from(low)..=finest).collect(),
    };
    let after = Owner {
        node_depth: probe.node_depth,
        lo: before.child_lo(0),
        active: probe.active.clone(),
    };
    let samples: Vec<_> = (0..8)
        .map(|o| (before.clone(), after.sub_centre(o)))
        .collect();
    let run = probe.run();
    let low_run = finest - 1 - u8::from(low);
    Verdict {
        eval_holds: probe.held,
        old_holds: probe.old_held,
        gate_releases: run_covers(run, &[(low_run, finest)], wish),
        oracle: twin_releases(&after, wish, &samples),
        run,
    }
}

/// Zoom out: the children 10..=17 of node 2 drew with nominal depth `finest - 1` and finest
/// active depth `finest` (`mixed`: only child 10 had `finest`, the others 5, so the octants of
/// node 2 differ); node 2 (stored 4..=7, never loaded) has `resident` levels and wants `wish`.
fn zoom_out_case((finest, mixed, wish, resident): (u8, bool, u8, &[u8]), far: Far) -> Verdict {
    let (mut rig, _, _) = zoom_out_rig();
    set_resident(&mut rig, 2, resident);
    let finest_of = |child: usize| if mixed && child != 10 { 5 } else { finest };
    for child in 10..18 {
        set_applied(&mut rig, child as u64, finest_of(child));
    }
    let probe = refine_probe(&mut rig, 2, wish, far);
    let grid = grid();
    let child_depth = CellPool::node_depth(&grid, rig.nodes[&NodeId::new(10)].bounds);
    assert_eq!(
        child_depth,
        probe.node_depth + 1,
        "10..=17 are children of 2"
    );
    let after = Owner {
        node_depth: probe.node_depth,
        lo: [0.0, 0.0, 10.0],
        active: probe.active.clone(),
    };
    let mut samples = Vec::new();
    let mut runs = Vec::new();
    for child in 10..18 {
        let f = finest_of(child);
        let owner = Owner {
            node_depth: child_depth,
            lo: after.child_lo(child - 10),
            active: vec![f - 1, f],
        };
        samples.extend((0..8).map(|o| (owner.clone(), owner.sub_centre(o))));
        runs.push((f - 1, f));
    }
    let run = probe.run();
    Verdict {
        eval_holds: probe.held,
        old_holds: probe.old_held,
        gate_releases: run_covers(run, &runs, wish),
        oracle: twin_releases(&after, wish, &samples),
        run,
    }
}

/// `(finest depth the ancestor drew, wish of the child, resident levels of the child, held)`.
/// The observed rule, with `m = min(finest, wish)` and `d` the depth the child would draw
/// (seeded at `finest`, one step towards the wish, else the resident level closest to the wish):
/// the child is held iff `d < m`. The `finest` is the finest ACTIVE depth of the ancestor, not
/// the nominal depth it drew (always `finest - 1` here), and the child may take over below what
/// it wants: reaching `finest` is enough, one level more is not asked.
const ZOOM_IN: &[(u8, u8, &[u8], bool)] = &[
    (6, 7, &[5], true),
    (6, 7, &[4, 5], true),
    (6, 7, &[6], false),
    (6, 7, &[7], false),
    (6, 7, &[4, 7], false),
    (6, 7, &[5, 7], false),
    (6, 6, &[5], true),
    (6, 6, &[4, 5], true),
    (6, 6, &[6], false),
    (6, 6, &[4, 7], false),
    // The wish below what the ancestor drew bounds the threshold: it takes over at the wish.
    (6, 5, &[4], true),
    (6, 5, &[4, 7], true),
    (6, 5, &[5], false),
    (6, 5, &[5, 7], false),
    (7, 7, &[6], true),
    (7, 7, &[4, 5], true),
    (7, 7, &[7], false),
    (7, 7, &[4, 7], false),
    (7, 4, &[4], false),
    (5, 7, &[4], true),
    (5, 7, &[5], false),
    // The ancestor drew finer than the child can build (7): the wish bounds it again.
    (8, 7, &[6], true),
    (8, 7, &[7], false),
];

/// Zoom-in rows whose ancestor has octants that drew different depths (`low`): `(finest, wish,
/// resident, held)` as in `ZOOM_IN`, the ancestor's active set being `finest - 2..=finest`.
const ZOOM_IN_LOW: &[(u8, u8, &[u8], bool)] = &[(6, 7, &[5], true)];

/// The zoom-in rows where `eval_refine_hold` and `run_covers` differ, and how. The twin oracle
/// agrees with `run_covers` on every row (`assert_judged`).
const ZOOM_IN_PARTS: &[(InCase, Parts)] = &[];

/// The zoom-out rows where `eval_refine_hold` and `run_covers` differ, and how.
const ZOOM_OUT_PARTS: &[(OutCase, Parts)] = &[
    ((7, true, 7, &[4, 6, 7]), Parts::EvalHoldsLonger),
    ((7, true, 7, &[4, 7]), Parts::EvalHoldsLonger),
    ((7, true, 6, &[4, 6]), Parts::EvalHoldsLonger),
    ((7, true, 6, &[4, 6, 7]), Parts::EvalHoldsLonger),
    ((6, true, 6, &[4, 6]), Parts::EvalHoldsLonger),
];

/// `(finest, low, wish, resident)` of a zoom-in row, see `zoom_in_case`.
type InCase = (u8, bool, u8, &'static [u8]);
/// `(finest, mixed, wish, resident)` of a zoom-out row, see `zoom_out_case`.
type OutCase = (u8, bool, u8, &'static [u8]);

/// Checks one verdict: the production decision releases the node iff the twin oracle does (and
/// iff `run_covers` does), and `eval_refine_hold` held the node iff `held`. Returns where
/// `eval_refine_hold` and the gate part ways.
fn assert_judged(case: &str, verdict: &Verdict, held: bool) -> Option<Parts> {
    let run = verdict.run;
    assert!(run.is_some(), "{case}: the node must have a run");
    assert_eq!(
        verdict.old_holds, held,
        "{case}: eval_refine_hold, run {run:?}"
    );
    assert_eq!(
        verdict.oracle.is_ok(),
        !verdict.eval_holds,
        "{case}: eval_hold and the twin disagree, run {run:?}: {:?}",
        verdict.oracle
    );
    assert_eq!(
        verdict.gate_releases, !verdict.eval_holds,
        "{case}: eval_hold and run_covers disagree, run {run:?}"
    );
    parts(verdict)
}

/// A row listed in a `*_PARTS` table parts ways in the direction given under some active rule
/// and range and never in the other; any other row never does. The active run depends on the
/// rule and the range, `eval_refine_hold` does not.
fn assert_parts<C: std::fmt::Debug + PartialEq>(
    case: &C,
    table: &[(C, Parts)],
    seen: &[Option<Parts>],
) {
    let expected = table.iter().find(|(c, _)| c == case).map(|(_, p)| *p);
    match expected {
        None => assert!(seen.iter().all(Option::is_none), "{case:?}: {seen:?}"),
        Some(p) => assert!(
            seen.contains(&Some(p)) && seen.iter().all(|s| s.is_none() || *s == Some(p)),
            "{case:?}: expected {p:?}, saw {seen:?}"
        ),
    }
}

/// Judges one case under every range.
fn judge(case: &str, held: bool, run: &dyn Fn(Far) -> Verdict) -> Vec<Option<Parts>> {
    FARS.iter()
        .map(|far| assert_judged(&format!("{far:?} {case}"), &run(*far), held))
        .collect()
}

#[test]
fn zoom_in_a_child_is_held_while_it_would_draw_below_the_ancestors_finest_depth() {
    for (finest, wish, resident, held) in ZOOM_IN {
        let case: InCase = (*finest, false, *wish, *resident);
        let seen = judge(&format!("{case:?}"), *held, &|far| zoom_in_case(case, far));
        assert_parts(&case, ZOOM_IN_PARTS, &seen);
    }
}

#[test]
fn zoom_in_with_octants_that_drew_different_depths_is_judged_by_the_twin() {
    for (finest, wish, resident, held) in ZOOM_IN_LOW {
        let case: InCase = (*finest, true, *wish, *resident);
        let seen = judge(&format!("{case:?}"), *held, &|far| zoom_in_case(case, far));
        assert_parts(&case, ZOOM_IN_PARTS, &seen);
    }
}

#[test]
fn zoom_out_a_node_is_held_while_it_would_draw_below_what_its_descendants_drew() {
    for (finest, mixed, wish, resident, held) in ZOOM_OUT {
        let case: OutCase = (*finest, *mixed, *wish, *resident);
        let seen = judge(&format!("{case:?}"), *held, &|far| zoom_out_case(case, far));
        assert_parts(&case, ZOOM_OUT_PARTS, &seen);
    }
}

/// `(finest depth of the descendants, mixed, wish of the node, resident levels, held)`. The
/// observed rule: with `m = min(finest over its octants, wish)` and `d` the depth the node would
/// draw (seeded at the coarsest nominal depth of the descendants over the octant, one step
/// towards the wish), the node is held iff some octant has `d < m`.
const ZOOM_OUT: &[(u8, bool, u8, &[u8], bool)] = &[
    (6, false, 7, &[5], true),
    (6, false, 7, &[4, 5], true),
    (6, false, 7, &[5, 7], true),
    (6, false, 7, &[6], false),
    (6, false, 7, &[7], false),
    (6, false, 7, &[4, 7], false),
    (6, false, 6, &[5], true),
    (6, false, 6, &[5, 7], true),
    (6, false, 6, &[6], false),
    (6, false, 6, &[4, 7], false),
    (6, false, 5, &[4], true),
    (6, false, 5, &[4, 7], true),
    (6, false, 5, &[5], false),
    (6, false, 5, &[5, 7], false),
    (7, false, 7, &[6], true),
    (7, false, 7, &[4, 5], true),
    (7, false, 7, &[7], false),
    (7, false, 7, &[5, 7], false),
    (7, false, 6, &[5], true),
    (7, false, 6, &[6], false),
    (7, false, 6, &[5, 7], false),
    (5, false, 7, &[4], true),
    (5, false, 7, &[5], false),
    (7, false, 4, &[4], false),
    // One octant over a descendant that drew 7, the rest 5: the finest one counts.
    (7, true, 7, &[6], true),
    (7, true, 7, &[5, 6], true),
    (7, true, 7, &[7], false),
    (7, true, 5, &[5], false),
    // Per-octant mixed: one descendant drew `finest`, the others 5, the node has gaps in its
    // levels (review of task 8: node 2 stored 4..=7, resident {4, 6, 7}).
    (7, true, 7, &[4, 6, 7], true),
    (7, true, 6, &[4, 6], true),
    (7, true, 6, &[4, 6, 7], true),
    (7, true, 7, &[4, 7], true),
    (7, true, 5, &[4, 5], false),
    (6, true, 7, &[4, 5], true),
    (6, true, 7, &[4, 5, 7], true),
    (6, true, 6, &[4, 6], true),
    (6, true, 6, &[4, 5], true),
    // The descendants drew 5 and the node holds the levels above: its run end follows the
    // resident levels (`the_range_moves_the_run_ends`).
    (5, false, 7, &[5, 6, 7], false),
    (5, true, 7, &[4, 5, 6, 7], false),
];

/// `eval_hold` for `id` as if the previous plan already held it. `covering` are the nodes whose
/// entries `cell_cut` appends after the cut because they draw its zone meanwhile (the ancestor on
/// a zoom in, the descendants on a zoom out).
fn eval_held(rig: &Rig, id: u64, covering: &[u64], wish: u8) -> HoldEval {
    let id = NodeId::new(id);
    let nodes: FastMap<NodeId, PoolNode> = rig
        .nodes
        .iter()
        .map(|(id, node)| (*id, node.clone()))
        .collect();
    let mut entries = entries_with(rig, &[id], wish);
    set_range(rig, &mut entries[0], Far::Wide);
    for node in covering {
        entries.extend(entries_with(rig, &[NodeId::new(*node)], wish));
    }
    let held: FastSet<NodeId> = [id].into_iter().collect();
    rig.pool
        .eval_hold(&entries, &[id], &nodes, &grid(), &held, &|_, _, _| wish)
}

/// The hysteresis branch of `eval_refine_hold` (`if held && ...`): a node held by the previous
/// plan is judged like a fresh one while something draws its zone (so the table rows hold for
/// it too, and it stays in `settle`); once nothing draws there it is released.
#[test]
fn a_node_already_held_is_judged_like_a_fresh_one_while_its_zone_is_drawn() {
    for (finest, wish, resident, held) in ZOOM_IN {
        let mut rig = Rig::new(two_regions_with_coarse_children(), 512, 3, 1);
        rig.run(60, &ids(&[2, 3]), &camera("fit"), 8.0);
        set_applied(&mut rig, 2, *finest);
        set_resident(&mut rig, 10, resident);
        let case = format!("zoom in {finest} {wish} {resident:?}");
        let covered = eval_held(&rig, 10, &[2], *wish);
        assert_eq!(covered.hold.contains(&NodeId::new(10)), *held, "{case}");
        assert!(covered.settle.contains(&NodeId::new(10)), "{case}");
        let alone = eval_held(&rig, 10, &[], *wish);
        assert!(alone.hold.is_empty() && alone.settle.is_empty(), "{case}");
    }
    for (finest, mixed, wish, resident, held) in ZOOM_OUT {
        let (mut rig, _, _) = zoom_out_rig();
        set_resident(&mut rig, 2, resident);
        for child in 10..18 {
            let f = if *mixed && child != 10 { 5 } else { *finest };
            set_applied(&mut rig, child, f);
        }
        let case = format!("zoom out {finest} {mixed} {wish} {resident:?}");
        let children: Vec<u64> = (10..18).collect();
        let covered = eval_held(&rig, 2, &children, *wish);
        // The rows `eval_refine_hold` held longer than the twin needs are released now.
        let held = *held
            && !ZOOM_OUT_PARTS
                .iter()
                .any(|(c, _)| *c == (*finest, *mixed, *wish, *resident));
        assert_eq!(covered.hold.contains(&NodeId::new(2)), held, "{case}");
        assert!(covered.settle.contains(&NodeId::new(2)), "{case}");
        let alone = eval_held(&rig, 2, &[], *wish);
        assert!(alone.hold.is_empty() && alone.settle.is_empty(), "{case}");
    }
}

/// With a `range` on the entry its width changes the run of the node (every resident level up
/// to the range end is active), so the judges see more than the drawn levels.
#[test]
fn the_range_moves_the_run_ends() {
    let runs = |case: OutCase| -> std::collections::BTreeSet<Option<Run>> {
        FARS.iter()
            .map(|far| zoom_out_case(case, *far).run)
            .collect()
    };
    // The coarse end moves with the width of the range.
    assert_eq!(
        runs((6, false, 7, &[4, 5])),
        [Some((4, 5)), Some((5, 5))].into()
    );
    // The fine end reaches every resident level: descendants drew 5, the node has 5, 6 and 7.
    assert_eq!(runs((5, false, 7, &[5, 6, 7])), [Some((5, 7))].into());
}

#[test]
fn octants_of_a_node_touch_the_ancestor_octants_below_them() {
    let f = Footprint {
        rep: 6,
        lo: [0; 3],
        hi: [15; 3],
    };
    assert_eq!(f.octants_in(1).count(), 8);
    for octant in 0..8 {
        assert_eq!(f.octants_of(1, octant).collect::<Vec<_>>(), vec![octant]);
    }
    // From two levels up the whole node lies in one octant of the ancestor.
    for octant in 0..8 {
        assert_eq!(f.octants_of(2, octant).collect::<Vec<_>>(), vec![0]);
    }
}

#[test]
fn a_hold_that_does_not_settle_stays_held_and_never_draws_coarser() {
    // One pass cannot reach a fixed point whenever the held set changes: the nodes held in the
    // input or in the new set stay held (so the image never gets coarser) and are counted.
    let nodes = two_regions();
    let mut rig = Rig::new(nodes.clone(), 512, 3, 1);
    rig.pool.hold_passes = 1;
    let cam = camera("fit");
    let (before, resident) = run_with_resident(&mut rig, 60, &ids(&[2, 3]), &cam, 8.0);
    assert_eq!(before.counters.hold_unsettled, 0);
    let points = sample_points(nodes[&NodeId::new(2)].bounds);
    let base: Vec<Option<u8>> = points
        .iter()
        .map(|p| drawn_at(&before, &nodes, &resident, *p))
        .collect();
    let mut cut = ids(&region_children(10));
    cut.push(NodeId::new(3));
    let (mut unsettled, mut held) = (0, Vec::new());
    for frame in 0..40 {
        let resident = resident_levels(&rig);
        let plan = rig.step(&cut, &cam, 8.0);
        assert_eq!(plan.apply, CutApply::Applied, "frame {frame}");
        assert_no_holes(&plan, &nodes);
        for (point, was) in points.iter().zip(&base) {
            let now = drawn_at(&plan, &nodes, &resident, *point);
            assert!(now >= *was, "frame {frame}: {point:?} {now:?} < {was:?}");
        }
        unsettled += plan.counters.hold_unsettled;
        held.push(plan.counters.held_nodes);
    }
    assert!(unsettled > 0, "the hold always settled");
    assert!(held.windows(2).all(|w| w[0] <= w[1]), "{held:?}");
    assert!(held[30..].iter().all(|h| *h == held[39]), "{held:?}");
}

#[test]
fn the_real_cut_is_not_reused_when_the_applied_depths_changed() {
    let nodes = two_regions();
    let mut rig = Rig::new(nodes, 512, 3, 1);
    let cam = camera("fit");
    rig.run(60, &ids(&[2, 3]), &cam, 8.0);
    assert!(rig.step(&ids(&[2, 3]), &cam, 8.0).counters.real_reused);
    rig.pool.applied_digest ^= 1;
    assert!(!rig.step(&ids(&[2, 3]), &cam, 8.0).counters.real_reused);
}

#[test]
fn reset_with_held_nodes_converges_back() {
    let nodes = two_regions();
    let mut rig = Rig::new(nodes.clone(), 512, 3, 1);
    let cam = camera("fit");
    rig.run(60, &ids(&[2, 3]), &cam, 8.0);
    let mut cut = ids(&region_children(10));
    cut.push(NodeId::new(3));
    let mut frames = 0;
    while rig.step(&cut, &cam, 8.0).counters.held_nodes == 0 {
        frames += 1;
        assert!(frames < 20, "setup: nothing was held");
    }
    assert!(!rig.pool.hold.is_empty());
    let _ = rig.pool.reset();
    assert!(rig.pool.hold.is_empty() && rig.pool.applied_drawn.is_empty());
    assert_eq!(rig.pool.applied_digest, 0);
    rig.inflight.clear();
    rig.gpu = FakeGpu::default();
    let mut last = None;
    for _ in 0..80 {
        let plan = rig.step(&cut, &cam, 8.0);
        assert_no_holes(&plan, &nodes);
        last = Some(plan);
    }
    let plan = last.unwrap();
    assert_eq!(plan.apply, CutApply::Applied);
    assert_eq!(plan.entries.len(), cut.len());
    assert_eq!(plan.counters.held_nodes, 0);
    assert_eq!(plan.counters.nodes_without_level, 0);
    assert_eq!(plan.counters.fallback_pages, 0);
    assert!(rig.pool.hold.is_empty());
    rig.assert_gpu_matches_pool();
}
