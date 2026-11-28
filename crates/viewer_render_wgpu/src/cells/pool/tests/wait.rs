//! R3-Q20: nodes without a level wait outside the cut.

use super::*;

/// What the pool held when a frame was planned: the resident levels before the step and the caps
/// after it (the caps the plan was cut with).
struct Seen {
    resident: HashMap<NodeId, Vec<u8>>,
    caps: FastMap<NodeId, u8>,
}

fn observe(rig: &mut Rig, cut: &[NodeId], camera: &OrbitCamera, target: f64) -> (FramePlan, Seen) {
    let resident = rig
        .nodes
        .iter()
        .map(|(id, node)| {
            let levels = node
                .stored
                .iter()
                .copied()
                .filter(|depth| rig.pool.is_resident(rig.pool.key(*id, *depth)))
                .collect();
            (*id, levels)
        })
        .collect();
    let plan = rig.step(cut, camera, target);
    let caps = rig.pool.caps.clone();
    (plan, Seen { resident, caps })
}

/// `(drawn, wanted)` depth of the entry that really draws `point` (not masked, octant in
/// `active`); the geometry is computed here from the bounds. The drawn depth must be the one
/// level that was resident (the exact stepped depth is checked per octant by `Rig::step`), and no
/// point may be drawn by two entries.
fn drawn_and_wanted_at(
    plan: &FramePlan,
    nodes: &HashMap<NodeId, PoolNode>,
    seen: &Seen,
    point: [f64; 3],
) -> Option<(u8, u8)> {
    let hits: Vec<(u8, u8)> = plan
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
                .then(|| Some((entry.drawn[octant]?, entry.wanted[octant]?)))
                .flatten()
                .inspect(|(drawn, _)| {
                    let levels = capped(&seen.resident[&entry.node], seen.caps.get(&entry.node));
                    // The expected stepped depth is `Rig::check_stepping`'s; here, that the
                    // level was resident, and that it is never past the wanted depth.
                    assert!(
                        levels.contains(drawn),
                        "node {:?} octant {octant}",
                        entry.node
                    );
                })
        })
        .collect();
    assert!(hits.len() <= 1, "{point:?} is drawn by {hits:?}");
    hits.first().copied()
}

/// What `points` drew: the drawn depth per point.
fn snapshot(
    plan: &FramePlan,
    nodes: &HashMap<NodeId, PoolNode>,
    seen: &Seen,
    points: &[[f64; 3]],
) -> Vec<Option<u8>> {
    points
        .iter()
        .map(|p| drawn_and_wanted_at(plan, nodes, seen, *p).map(|(drawn, _)| drawn))
        .collect()
}

/// No sample that was drawn is lost, and none is drawn below `min(before, wanted now)` (R3-Q21).
fn assert_kept(
    plan: &FramePlan,
    nodes: &HashMap<NodeId, PoolNode>,
    seen: &Seen,
    points: &[[f64; 3]],
    before: &[Option<u8>],
    label: &str,
) {
    for (point, was) in points.iter().zip(before) {
        let Some(was) = was else { continue };
        let (now, wanted) = drawn_and_wanted_at(plan, nodes, seen, *point)
            .unwrap_or_else(|| panic!("{label}: {point:?} was drawn and is lost"));
        assert!(
            now >= (*was).min(wanted),
            "{label}: {point:?} {now} < min({was}, {wanted})"
        );
    }
}

#[test]
fn the_first_rung_streams_in_one_node_per_frame_without_retaining() {
    let nodes = octant_nodes();
    let mut rig = Rig::new(nodes.clone(), 128, 8, 1);
    let cam = camera("fit");
    let mut seen_before: Vec<(u64, Vec<[f64; 3]>)> = Vec::new();
    for frame in 0..30 {
        let count = (frame + 1).min(CHILDREN.len());
        let cut = ids(&CHILDREN[..count]);
        let ready: Vec<bool> = CHILDREN[..count]
            .iter()
            .map(|id| coarse_resident(&rig, *id))
            .collect();
        let (plan, seen) = observe(&mut rig, &cut, &cam, 64.0);
        assert_eq!(plan.apply, CutApply::Applied, "frame {frame}");
        assert_eq!(plan.counters.lost_nodes, 0, "frame {frame}");
        let mut waiting = 0;
        for (index, id) in CHILDREN[..count].iter().enumerate() {
            // A node draws from the frame after its coarsest level is resident.
            assert_eq!(
                has_entry(&plan, *id),
                ready[index],
                "frame {frame} node {id}"
            );
            for point in sample_points(nodes[&NodeId::new(*id)].bounds) {
                assert_eq!(
                    drawn_and_wanted_at(&plan, &nodes, &seen, point).is_some(),
                    ready[index],
                    "frame {frame} node {id} {point:?}"
                );
            }
            waiting += usize::from(!ready[index]);
        }
        assert_eq!(plan.counters.nodes_without_level as usize, waiting);
        assert_eq!(plan.counters.excluded_nodes as usize, waiting);
        assert_eq!(plan.counters.hold_age_max > 0, waiting > 0, "frame {frame}");
        // Drawn nodes never stop drawing.
        for (id, points) in &seen_before {
            for point in points {
                assert!(
                    drawn_and_wanted_at(&plan, &nodes, &seen, *point).is_some(),
                    "frame {frame}: node {id} lost"
                );
            }
        }
        seen_before = CHILDREN[..count]
            .iter()
            .zip(&ready)
            .filter(|(_, ready)| **ready)
            .map(|(id, _)| (*id, sample_points(nodes[&NodeId::new(*id)].bounds)))
            .collect();
    }
    let last = rig.step(&ids(&CHILDREN), &cam, 64.0);
    assert_eq!(last.counters.nodes_without_level, 0);
    assert_eq!(last.counters.hold_age_max, 0);
    assert_eq!(last.entries.len(), CHILDREN.len());
}

/// Zooming out to node 2 at `target` px. At 64 px node 2 wants its coarsest level (4); at 4 px it
/// wants 7, finer than its coarsest, so the children keep drawing after node 2 has a level until
/// the level they drew is resident (R3-Q21).
fn zoom_out(target: f64) {
    let (mut rig, nodes, cam) = zoom_out_rig();
    let points = sample_points(nodes[&NodeId::new(2)].bounds);
    let (before_plan, seen) = observe(&mut rig, &ids(&region_children(10)), &cam, 4.0);
    let before = snapshot(&before_plan, &nodes, &seen, &points);
    assert!(before.iter().all(|d| *d == Some(7)), "setup");
    let cut = ids(&[2]);
    let done = |rig: &Rig| {
        if target < 8.0 {
            resident_at(rig, 2, 7)
        } else {
            coarse_resident(rig, 2)
        }
    };
    let (mut waiting, mut taken, mut held_with_level) = (0_u32, false, false);
    for frame in 0..60 {
        let ready = done(&rig);
        let coarse = coarse_resident(&rig, 2);
        let (plan, seen) = observe(&mut rig, &cut, &cam, target);
        assert_eq!(plan.apply, CutApply::Applied, "frame {frame}");
        assert!(!plan.counters.descendant_too_fine, "frame {frame}");
        assert_eq!(plan.counters.lost_nodes, 0, "frame {frame}");
        assert_no_holes(&plan, &nodes);
        assert_kept(
            &plan,
            &nodes,
            &seen,
            &points,
            &before,
            &format!("frame {frame}"),
        );
        if ready {
            taken = true;
            assert!(
                has_entry(&plan, 2) && plan.entries.len() == 1,
                "frame {frame}"
            );
            assert_eq!(plan.counters.nodes_without_level, 0);
            assert_eq!(plan.counters.hold_age_max, 0);
        } else {
            waiting += 1;
            assert!(
                !has_entry(&plan, 2) || plan.entries.len() == 9,
                "frame {frame}"
            );
            assert_eq!(plan.counters.nodes_without_level, 1, "frame {frame}");
            assert_eq!(
                plan.counters.excluded_nodes + plan.counters.held_nodes,
                1,
                "frame {frame}"
            );
            assert_eq!(plan.counters.hold_age_max, waiting, "frame {frame}");
            held_with_level |= coarse && plan.counters.held_nodes == 1;
            assert!(!taken, "frame {frame}");
        }
    }
    assert!(waiting > 0 && taken);
    // With a wanted level finer than the coarsest, the node sits held while it has a level.
    assert_eq!(held_with_level, target < 8.0);
    // At a fixed pose the hold stays empty once it has emptied: no flicker, nothing stepping.
    for frame in 0..50 {
        let plan = rig.step(&cut, &cam, target);
        assert_eq!(plan.counters.held_nodes, 0, "rest frame {frame}");
        assert!(rig.pool.hold.is_empty(), "rest frame {frame}");
        assert_eq!(plan.counters.stages, 0, "rest frame {frame}");
    }
}

#[test]
fn zoom_out_keeps_the_descendants_drawing_until_the_coarse_node_is_resident() {
    zoom_out(64.0);
}

#[test]
fn zoom_out_keeps_the_descendants_until_the_node_has_the_level_they_drew() {
    zoom_out(4.0);
}

#[test]
fn a_pose_jump_to_a_disjoint_cut_retains_only_while_the_first_levels_arrive() {
    let nodes = two_regions();
    let mut rig = Rig::new(nodes.clone(), 512, 8, 1);
    let cam = camera("fit");
    let a = ids(&region_children(10));
    rig.run(60, &a, &cam, 4.0);
    let held = rig.step(&a, &cam, 4.0);
    let b = ids(&region_children(20));
    let (mut retained, mut applied_once) = (0, false);
    for frame in 0..20 {
        let ready = region_children(20)
            .into_iter()
            .any(|c| coarse_resident(&rig, c));
        let plan = rig.step(&b, &cam, 4.0);
        if ready {
            applied_once = true;
        }
        if applied_once {
            assert_eq!(plan.apply, CutApply::Applied, "frame {frame}");
            assert_eq!(plan.counters.lost_nodes, 0);
        } else {
            // Nothing of the new cut draws yet: the old image stays, and it is not a "loss".
            assert_eq!(plan.apply, CutApply::RetainedPrevious, "frame {frame}");
            assert_eq!(plan.entries, held.entries);
            // The counters describe the retained cut: none of the new nodes is in it.
            assert_eq!(plan.counters.excluded_nodes, 8, "frame {frame}");
            assert_eq!(plan.counters.nodes_without_level, 8, "frame {frame}");
            retained += 1;
        }
    }
    assert!((1..=4).contains(&retained), "{retained}");
}

#[test]
fn a_pose_jump_keeps_the_zones_that_stay_in_the_cut() {
    let nodes = two_regions();
    let mut rig = Rig::new(nodes.clone(), 512, 8, 1);
    let cam = camera("fit");
    rig.run(60, &ids(&region_children(10)), &cam, 4.0);
    let points = sample_points(nodes[&NodeId::new(10)].bounds);
    let (held, seen) = observe(&mut rig, &ids(&region_children(10)), &cam, 4.0);
    let before = snapshot(&held, &nodes, &seen, &points);
    assert!(before.iter().all(Option::is_some), "setup");
    let mut cut = vec![NodeId::new(10)];
    cut.extend(ids(&region_children(20)));
    let mut b_drawn = false;
    for frame in 0..20 {
        let (plan, seen) = observe(&mut rig, &cut, &cam, 4.0);
        assert_eq!(plan.apply, CutApply::Applied, "frame {frame}");
        assert_eq!(plan.counters.lost_nodes, 0);
        assert_kept(
            &plan,
            &nodes,
            &seen,
            &points,
            &before,
            &format!("frame {frame}"),
        );
        b_drawn |= has_entry(&plan, 20);
    }
    assert!(b_drawn);
}

#[test]
fn a_cap_drop_loses_no_sample_and_never_draws_below_the_need() {
    let nodes = octant_nodes();
    let mut rig = Rig::new(nodes.clone(), 44, 64, 2);
    let cam = camera("fit");
    // The six farthest nodes fit at depth 7; adding the two nearest makes the fit coarsen the
    // farthest ones, which are drawing.
    let near = by_distance(&cam, &CHILDREN);
    let first = ids(&near[2..]);
    rig.run(30, &first, &cam, 4.0);
    let (held, seen) = observe(&mut rig, &first, &cam, 4.0);
    let points: Vec<[f64; 3]> = near[2..]
        .iter()
        .flat_map(|id| sample_points(nodes[&NodeId::new(*id)].bounds))
        .collect();
    let before = snapshot(&held, &nodes, &seen, &points);
    assert!(before.iter().all(|d| *d == Some(7)), "setup");
    let (mut dropped, mut waited) = (false, false);
    let mut last = None;
    for frame in 0..60 {
        let (plan, seen) = observe(&mut rig, &ids(&CHILDREN), &cam, 4.0);
        assert_eq!(plan.apply, CutApply::Applied, "frame {frame}");
        assert_eq!(plan.counters.lost_nodes, 0, "frame {frame}");
        assert_kept(
            &plan,
            &nodes,
            &seen,
            &points,
            &before,
            &format!("frame {frame}"),
        );
        dropped |= snapshot(&plan, &nodes, &seen, &points)
            .iter()
            .any(|d| d.is_some_and(|d| d < 7));
        waited |= plan.counters.excluded_nodes > 0;
        last = Some(plan);
    }
    assert!(dropped && waited);
    // No node starves under the cap: every child ends with an entry.
    let last = last.unwrap();
    for child in CHILDREN {
        assert!(has_entry(&last, child), "node {child} never entered");
    }
}

#[test]
fn a_zone_nothing_can_cover_retains_the_previous_cut_and_counts_the_loss() {
    // Node 20 is `MAX_MASK_LEVELS + 1` levels below the root, which is in the cut without a level:
    // the node cannot stand in for the root and nothing above it draws. Node 22 is unrelated.
    let mut nodes = chain_nodes();
    nodes.insert(
        NodeId::new(22),
        PoolNode {
            bounds: cube([ORIGIN[0] + SIDE / 2.0, ORIGIN[1], ORIGIN[2]], SIDE / 2.0),
            stored: vec![4, 5],
            buildable: (4, 5),
            points: 2000,
            octant_data_bounds: None,
        },
    );
    let mut rig = Rig::new(nodes.clone(), 64, 8, 1);
    let cam = camera("fit");
    rig.run(10, &ids(&[20, 22]), &cam, 64.0);
    let held = rig.step(&ids(&[20, 22]), &cam, 64.0);
    assert!(has_entry(&held, 20) && has_entry(&held, 22), "setup");
    let (mut retained, mut taken) = (0, false);
    for frame in 0..12 {
        let ready = coarse_resident(&rig, 1);
        let plan = rig.step(&ids(&[1, 22]), &cam, 64.0);
        if frame == 0 {
            assert_eq!(
                plan.fetch_requests.first(),
                Some(&rig.pool.key(NodeId::new(1), 3))
            );
        }
        if ready {
            taken = true;
            assert_eq!(plan.apply, CutApply::Applied, "frame {frame}");
            assert_eq!(plan.counters.lost_nodes, 0);
            assert!(has_entry(&plan, 1) && !has_entry(&plan, 20));
        } else {
            assert_eq!(plan.apply, CutApply::RetainedPrevious, "frame {frame}");
            assert_eq!(plan.counters.lost_nodes, 1, "frame {frame}");
            assert_eq!(plan.entries, held.entries);
            assert_eq!(plan.counters.nodes_without_level, 1);
            retained += 1;
        }
    }
    assert!(retained > 0 && taken);
}

#[test]
fn the_hold_age_grows_while_a_child_is_held_and_resets_after() {
    let nodes = two_regions();
    let mut rig = Rig::new(nodes.clone(), 512, 3, 1);
    let cam = camera("fit");
    rig.run(60, &ids(&[2, 3]), &cam, 8.0);
    let mut cut = ids(&region_children(10));
    cut.push(NodeId::new(3));
    let (mut streak, mut peak) = (0_u32, 0_u32);
    for frame in 0..40 {
        let plan = rig.step(&cut, &cam, 8.0);
        if plan.counters.held_nodes > 0 {
            streak += 1;
            // The oldest hold is at least as old as the run of frames with something held.
            assert!(plan.counters.hold_age_max >= 1, "frame {frame}");
            assert!(plan.counters.hold_age_max <= streak, "frame {frame}");
            peak = peak.max(plan.counters.hold_age_max);
        } else {
            assert_eq!(plan.counters.hold_age_max, 0, "frame {frame}");
            streak = 0;
        }
    }
    assert!(peak >= 2, "{peak}");
}

#[test]
fn reset_with_excluded_nodes_and_descendants_converges_back() {
    let (mut rig, nodes, cam) = zoom_out_rig();
    let cut = ids(&[2]);
    let plan = rig.step(&cut, &cam, 64.0);
    assert_eq!(plan.counters.excluded_nodes, 1);
    assert_eq!(plan.entries.len(), 8, "setup: the descendants stand in");
    assert!(!rig.pool.wait_ages.is_empty());
    let _ = rig.pool.reset();
    assert!(rig.pool.wait_ages.is_empty() && rig.pool.fallback_memo.is_none());
    assert!(rig.pool.real_memo.is_none() && rig.pool.applied.is_empty());
    rig.inflight.clear();
    rig.gpu = FakeGpu::default();
    let mut last = None;
    for _ in 0..40 {
        let plan = rig.step(&cut, &cam, 64.0);
        assert_no_holes(&plan, &nodes);
        last = Some(plan);
    }
    let plan = last.unwrap();
    assert_eq!(plan.apply, CutApply::Applied);
    assert_eq!(plan.entries.len(), 1);
    assert_eq!(plan.counters.excluded_nodes, 0);
    assert_eq!(plan.counters.nodes_without_level, 0);
    assert_eq!(plan.counters.hold_age_max, 0);
    rig.assert_gpu_matches_pool();
}

/// `chain_nodes` plus node 22, unrelated to the chain, and node 30 inside node 20.
fn chain_with_extras() -> HashMap<NodeId, PoolNode> {
    let mut nodes = chain_nodes();
    nodes.insert(
        NodeId::new(22),
        PoolNode {
            bounds: cube([ORIGIN[0] + SIDE / 2.0, ORIGIN[1], ORIGIN[2]], SIDE / 2.0),
            stored: vec![4, 5],
            buildable: (4, 5),
            points: 2000,
            octant_data_bounds: None,
        },
    );
    nodes.insert(
        NodeId::new(30),
        PoolNode {
            bounds: cube(ORIGIN, SIDE / 32.0),
            stored: vec![9, 10],
            buildable: (9, 10),
            points: 2000,
            octant_data_bounds: None,
        },
    );
    nodes
}

#[test]
fn an_excluded_node_too_fine_for_its_ancestor_waits_without_being_requested() {
    // Node 20 is `MAX_MASK_LEVELS + 1` levels below the root: both wait outside the cut while
    // node 22 (applied, an octant of the root) stands in for the root. The root, which is
    // requested, draws the zone as soon as it arrives; node 20 is not requested (its level would
    // make the real cut fail with `DescendantTooFine`) and keeps nothing waiting.
    let mut rig = Rig::new(chain_with_extras(), 64, 8, 1);
    let cam = camera("fit");
    rig.run(10, &ids(&[22]), &cam, 64.0);
    let mut waited = 0;
    let mut arrived = None;
    for frame in 0..12 {
        let plan = rig.step(&ids(&[1, 20]), &cam, 64.0);
        assert_eq!(plan.apply, CutApply::Applied, "frame {frame}");
        assert!(
            plan.fetch_requests
                .iter()
                .all(|k| k.node != NodeId::new(20))
        );
        if has_entry(&plan, 1) {
            arrived = Some(plan);
            break;
        }
        assert!(has_entry(&plan, 22), "frame {frame}: 22 stands in");
        waited += 1;
    }
    let plan = arrived.expect("the root arrives and draws");
    assert!(waited > 0);
    assert!(!has_entry(&plan, 22) && !plan.counters.descendant_too_fine);
    // Node 20 is drawn by the root, coarser: a stage, not a node waiting for a level, so the
    // counters settle (T11's "target reached" and the driver's `busy` depend on them).
    assert_eq!(plan.counters.excluded_nodes, 0);
    assert_eq!(plan.counters.nodes_without_level, 0);
    assert!(plan.counters.stages >= 8);
    assert_eq!(plan.counters.hold_age_max, 0);
    for _ in 0..5 {
        let plan = rig.step(&ids(&[1, 20]), &cam, 64.0);
        assert_eq!(plan.apply, CutApply::Applied);
        assert!(
            plan.fetch_requests
                .iter()
                .all(|k| k.node != NodeId::new(20))
        );
        assert_eq!(plan.counters.nodes_without_level, 0);
    }
}

#[test]
fn a_descendant_under_a_node_that_draws_does_not_stand_in_when_the_mask_cannot_hide_it() {
    // Applied {30}, with the root resident. The cut {root, 20} excludes node 20 (nothing within
    // `MAX_MASK_LEVELS` draws it); node 30, applied under it, is `MAX_MASK_LEVELS + 2` levels
    // below the root, which draws: as a stand-in it would make the real cut `DescendantTooFine`.
    let mut rig = Rig::new(chain_with_extras(), 64, 8, 1);
    let cam = camera("fit");
    rig.run(10, &ids(&[1]), &cam, 64.0);
    let alone = rig.run(15, &ids(&[30]), &cam, 64.0);
    assert!(has_entry(&alone, 30) && alone.entries.len() == 1, "setup");
    for frame in 0..6 {
        let plan = rig.step(&ids(&[1, 20]), &cam, 64.0);
        assert!(!plan.counters.descendant_too_fine, "frame {frame}");
        assert_eq!(plan.apply, CutApply::Applied, "frame {frame}");
        assert!(
            has_entry(&plan, 1) && !has_entry(&plan, 30),
            "frame {frame}"
        );
    }
}

/// An entry of `node` drawing the given octants at depth 7. The anchor is set as the pool does
/// (presence readers read it): a depth when some octant draws.
fn drawing(node: u64, octants: &[usize]) -> CellCutEntry {
    CellCutEntry {
        node: NodeId::new(node),
        wanted: [Some(7); 8],
        drawn: std::array::from_fn(|o| octants.contains(&o).then_some(7)),
        stage: [false; 8],
        step: [None; 8],
        range: None,
        node_anchor: (!octants.is_empty()).then_some(7),
        node_target: None,
        node_run: None,
        node_stage: false,
        node_step: None,
        mask: [0; 8],
    }
}

#[test]
fn a_zone_covered_by_one_sub_cell_of_a_descendant_is_lost() {
    let mut rig = Rig::new(two_regions(), 512, 8, 1);
    let cam = camera("fit");
    let applied = rig.run(30, &ids(&[2]), &cam, 4.0);
    assert!(
        has_entry(&applied, 2) && applied.entries[0].drawn.iter().all(Option::is_some),
        "setup"
    );
    let nodes: FastMap<NodeId, PoolNode> = rig
        .nodes
        .iter()
        .map(|(id, node)| (*id, node.clone()))
        .collect();
    let cut = ids(&[2]);
    let lost = |entries: &[CellCutEntry]| rig.pool.lost_zones(entries, &cut, &nodes, &grid());
    // One octant of one child covers a sub-cell of the zone node 2 drew.
    assert!(lost(&[drawing(10, &[0])]) > 0);
    // Every octant of one child still leaves the other seven children's zones.
    assert!(lost(&[drawing(10, &[0, 1, 2, 3, 4, 5, 6, 7])]) > 0);
    // The eight children drawing everything cover the zone.
    let all: Vec<CellCutEntry> = region_children(10)
        .into_iter()
        .map(|c| drawing(c, &[0, 1, 2, 3, 4, 5, 6, 7]))
        .collect();
    assert_eq!(lost(&all), 0);
}

/// `(drawn, wanted)` at `point` like `drawn_and_wanted_at`, without the `resident_depth` oracle: a
/// capped node may keep drawing a level above its cap (R3-Q21), so the drawn level must be
/// resident in the pool and drawn by an active level instead.
fn drawn_depth_at(
    plan: &FramePlan,
    nodes: &HashMap<NodeId, PoolNode>,
    rig: &Rig,
    point: [f64; 3],
) -> Option<(u8, u8)> {
    let hits: Vec<(u8, u8)> = plan
        .entries
        .iter()
        .filter_map(|entry| {
            let at = cell_of(nodes[&entry.node].bounds, point)?;
            if entry.mask[at[2]] >> (at[0] + 8 * at[1]) & 1 == 1 {
                return None;
            }
            let octant = ((at[0] / 4) << 2) | ((at[1] / 4) << 1) | (at[2] / 4);
            let drawn = entry.drawn[octant]?;
            plan.active
                .iter()
                .any(|a| {
                    a.key.node == entry.node
                        && a.key.depth == drawn
                        && a.octant_bits >> octant & 1 == 1
                })
                .then(|| {
                    assert!(rig.pool.is_resident(rig.pool.key(entry.node, drawn)));
                    (
                        drawn,
                        entry.wanted[octant].expect("a drawn octant is wanted"),
                    )
                })
        })
        .collect();
    assert!(hits.len() <= 1, "{point:?} is drawn by {hits:?}");
    hits.first().copied()
}

/// Six children draw depth 7 (5 pages each with the pinned level 4). Returns the rig, the camera,
/// the six ids (nearest last), their samples and what they drew. The capacity leaves the caps that
/// a test inserts in place: the six at depth 7 sit between the 80 % and 90 % thresholds.
#[expect(
    clippy::type_complexity,
    reason = "a test rig returning a tuple of fixtures; a named type would be used once"
)]
fn cap_drop_rig(
    capacity: u32,
    delay: u64,
) -> (
    Rig,
    OrbitCamera,
    Vec<NodeId>,
    Vec<[f64; 3]>,
    Vec<Option<u8>>,
) {
    let nodes = octant_nodes();
    let mut rig = Rig::new(nodes.clone(), capacity, 64, delay);
    let cam = camera("fit");
    let near = by_distance(&cam, &CHILDREN);
    let cut = ids(&near[2..]);
    rig.run(30, &cut, &cam, 4.0);
    let points: Vec<[f64; 3]> = cut
        .iter()
        .flat_map(|id| sample_points(nodes[id].bounds))
        .collect();
    let plan = rig.step(&cut, &cam, 4.0);
    let before: Vec<Option<u8>> = points
        .iter()
        .map(|p| drawn_depth_at(&plan, &nodes, &rig, *p).map(|(d, _)| d))
        .collect();
    assert!(before.iter().all(|d| *d == Some(7)), "setup");
    (rig, cam, cut, points, before)
}

/// Steps `frames` frames and counts the samples drawn below `min(before, wanted now)` per frame.
fn cap_drop_run(
    rig: &mut Rig,
    cam: &OrbitCamera,
    cut: &[NodeId],
    points: &[[f64; 3]],
    before: &[Option<u8>],
    frames: usize,
) -> Vec<(FramePlan, usize)> {
    let nodes = rig.nodes.clone();
    let mut out = Vec::new();
    for frame in 0..frames {
        rig.pool.size_epoch += u64::from(frame == 0);
        let plan = rig.step(cut, cam, 4.0);
        assert_eq!(plan.apply, CutApply::Applied, "frame {frame}");
        assert_eq!(plan.counters.lost_nodes, 0, "frame {frame}");
        assert_no_holes(&plan, &nodes);
        let mut coarsened = 0;
        for (point, was) in points.iter().zip(before) {
            let (now, wanted) = drawn_depth_at(&plan, &nodes, rig, *point)
                .unwrap_or_else(|| panic!("frame {frame}: {point:?} lost"));
            coarsened += usize::from(now < was.unwrap().min(wanted));
        }
        out.push((plan, coarsened));
    }
    out
}

#[test]
fn a_cap_drop_keeps_the_finer_level_until_the_capped_one_is_resident() {
    let (mut rig, cam, cut, points, before) = cap_drop_rig(40, 3);
    let node = cut[0];
    let key6 = rig.pool.key(node, 6);
    assert!(
        !rig.pool.is_resident(key6),
        "setup: depth 6 is not resident"
    );
    rig.pool.caps.insert(node, 6);
    let frames = cap_drop_run(&mut rig, &cam, &cut, &points, &before, 20);
    assert_eq!(rig.pool.caps.get(&node), Some(&6), "setup: the cap stays");
    assert!(
        frames.iter().all(|(_, coarsened)| *coarsened == 0),
        "no frame draws below min(before, wanted now)"
    );
    // The node keeps drawing 7 while 6 is missing, switches the frame after it is resident, and
    // the old level is then released.
    let mut switched = None;
    for (frame, (plan, _)) in frames.iter().enumerate() {
        let drawn = depth_drawn(plan, node);
        if switched.is_none() && drawn == [6] {
            switched = Some(frame);
        }
        if switched.is_some() {
            assert_eq!(drawn, [6], "frame {frame}");
        } else {
            assert_eq!(drawn, [7], "frame {frame}");
        }
    }
    let switched = switched.expect("the node switches to the capped level");
    assert!(switched >= 2, "the capped level takes frames to arrive");
    assert!(
        !rig.pool.is_resident(rig.pool.key(node, 7)),
        "old level released"
    );
    assert_eq!(rig.pool.overflow, 0);
}

#[test]
fn a_cap_drop_whose_capped_level_is_resident_switches_at_once() {
    // The frame planned right after the capped level became resident (while the finer one is
    // still resident and drew the frame before) already draws the capped level: no extra frame.
    let (mut rig, cam, cut, points, before) = cap_drop_rig(40, 1);
    let node = cut[0];
    rig.pool.caps.insert(node, 6);
    let mut waited = 0;
    loop {
        let frames = cap_drop_run(&mut rig, &cam, &cut, &points, &before, 1);
        let resident = rig.pool.is_resident(rig.pool.key(node, 6));
        if resident {
            assert!(
                rig.pool.is_resident(rig.pool.key(node, 7)),
                "setup: both resident"
            );
            assert_eq!(depth_drawn(&frames[0].0, node), [7]);
            break;
        }
        waited += 1;
        assert!(waited < 10, "the capped level never arrives");
    }
    let frames = cap_drop_run(&mut rig, &cam, &cut, &points, &before, 1);
    assert_eq!(depth_drawn(&frames[0].0, node), [6], "immediate switch");
    assert_eq!(frames[0].1, 0);
}

#[test]
fn a_cap_drop_in_a_tight_pool_releases_the_old_level_and_completes() {
    let (mut rig, cam, cut, points, before) = cap_drop_rig(40, 2);
    for id in &cut {
        rig.pool.caps.insert(*id, 6);
    }
    let frames = cap_drop_run(&mut rig, &cam, &cut, &points, &before, 40);
    // The six capped levels (24 pages) do not fit beside the six depth-7 levels: the old levels
    // are given up when the capped ones need their pages, so coarser frames are allowed (counted)
    // but the transition ends and nothing overflows.
    let last = &frames.last().unwrap().0;
    for id in &cut {
        assert_eq!(depth_drawn(last, *id), [6], "node {id:?}");
    }
    assert_eq!(rig.pool.overflow, 0);
    assert!(
        frames
            .iter()
            .all(|(plan, _)| plan.counters.pool_overflow == 0)
    );
    println!(
        "tight pool coarsened samples per frame: {:?}",
        frames.iter().map(|(_, c)| *c).collect::<Vec<_>>()
    );
}

/// Depth `plan` draws in every octant of node 1, which must be one.
fn drawn_depth(plan: &FramePlan) -> u8 {
    let drawn = depth_of(plan, 1);
    let first = drawn[0].expect("drawn");
    assert!(drawn.iter().all(|d| *d == Some(first)), "{drawn:?}");
    first
}

#[test]
fn continuity_caps_the_drawn_level_without_a_stage_and_literal_mode_is_unchanged() {
    let cut = ids(&[1]);
    let cam = camera("fit");
    let mut literal = Rig::new(root_node(), 64, 16, 1);
    let plan = literal.run(12, &cut, &cam, 1.0);
    assert_eq!(drawn_depth(&plan), 5);
    let literal_entries = plan.entries.clone();

    // Cells of depth 4 have side SIDE / 16: the minimum admits depths 3 and 4 only.
    let mut rig = Rig::new(root_node(), 64, 16, 1);
    rig.pool.set_min_side(Some(SIDE / 16.0));
    let plan = rig.run(12, &cut, &cam, 1.0);
    assert_eq!(drawn_depth(&plan), 4);
    assert_eq!(plan.counters.stages, 0, "the cap is the wanted look");
    assert_eq!(plan.counters.pool_coarsened, 0);
    assert_eq!(plan.counters.nodes_without_level, 0);
    assert_eq!(plan.entries[0].wanted, [Some(4); 8]);
    assert_covers_once(&plan.entries[0]);
    assert!(!rig.pool.is_resident(rig.pool.key(NodeId::new(1), 5)));

    // Off again: the literal cut comes back, the same one a pool that never had it draws.
    rig.pool.set_min_side(None);
    let plan = rig.run(12, &cut, &cam, 1.0);
    assert_eq!(plan.entries, literal_entries);
}

#[test]
fn the_continuity_cap_uses_the_buildable_range() {
    let cut = ids(&[1]);
    let cam = camera("fit");
    // A minimum between two depths rounds up the side: depth 4 is SIDE / 16, 3 is SIDE / 8. Depth
    // 4 is not stored (3 and 5 are) but it is buildable, so the cap lands on it.
    for (min, expect) in [
        (SIDE / 16.0, 4),
        (SIDE / 12.0, 3),
        (SIDE / 8.0, 3),
        (SIDE, 3),
        (SIDE * 4.0, 3),
        (SIDE / 32.0, 5),
        (SIDE / 1024.0, 7),
    ] {
        let mut nodes = root_node();
        let node = nodes.get_mut(&NodeId::new(1)).unwrap();
        node.stored = vec![3, 5];
        node.buildable = (3, 7);
        let mut rig = Rig::new(nodes, 64, 16, 1);
        rig.pool.set_min_side(Some(min));
        let plan = rig.run(12, &cut, &cam, 1.0);
        assert_eq!(plan.entries[0].wanted, [Some(expect); 8], "min {min}");
        // Cells at the capped depth are the finest the minimum side admits (RQ-09): what draws is
        // the stored level that serves it, never finer than the first stored level at or above;
        // a depth that is not stored is also requested.
        let served = if expect <= 3 { 3 } else { 5 };
        assert_eq!(drawn_depth(&plan), served, "min {min}");
        let stored = [3, 5].contains(&expect);
        let requested: Vec<u8> = plan.level_requests.iter().map(|key| key.depth).collect();
        assert_eq!(
            requested,
            if stored { vec![] } else { vec![expect] },
            "min {min}"
        );
    }
    // Every buildable depth is finer than the minimum: the coarsest one is wanted.
    let mut fine = root_node();
    let node = fine.get_mut(&NodeId::new(1)).unwrap();
    node.stored = vec![6, 7];
    node.buildable = (6, 7);
    let mut rig = Rig::new(fine, 64, 16, 1);
    rig.pool.set_min_side(Some(SIDE / 4.0));
    let plan = rig.run(12, &cut, &cam, 1.0);
    assert_eq!(drawn_depth(&plan), 6);
    assert_eq!(plan.entries[0].wanted, [Some(6); 8]);
}

#[test]
fn a_wanted_depth_outside_the_chain_is_requested_and_residents_keep_drawing() {
    let cut = ids(&[1]);
    let mut nodes = root_node();
    let node = nodes.get_mut(&NodeId::new(1)).unwrap();
    node.stored = vec![3, 5];
    node.buildable = (3, 7);
    let mut rig = Rig::new(nodes, 64, 16, 1);
    // The finest stored level is resident and drawn.
    let plan = rig.run(12, &cut, &camera("zoom"), 1.0);
    let wanted = plan.entries[0].wanted[0].expect("wanted");
    assert!(
        wanted > 5,
        "setup: the view wants a depth that is not stored"
    );
    assert_eq!(drawn_depth(&plan), 5);
    assert!(rig.pool.is_resident(rig.pool.key(NodeId::new(1), 5)));
    let key = rig.pool.key(NodeId::new(1), wanted);
    for _ in 0..4 {
        let plan = rig.step(&cut, &camera("zoom"), 1.0);
        assert_eq!(plan.level_requests, [key]);
        assert!(plan.fetch_requests.iter().all(|k| k.depth <= 5));
        assert_eq!(plan.apply, CutApply::Applied);
        assert_eq!(drawn_depth(&plan), 5, "the resident level keeps drawing");
        assert_covers_once(&plan.entries[0]);
    }
}

#[test]
fn a_fit_cap_keeps_the_resident_finer_level_while_the_capped_one_is_pending() {
    let cut = ids(&[1]);
    let mut nodes = root_node();
    let node = nodes.get_mut(&NodeId::new(1)).unwrap();
    node.stored = vec![3, 5, 7];
    node.buildable = (3, 9);
    let mut rig = Rig::new(nodes, 64, 16, 1);
    let plan = rig.run(14, &cut, &camera("zoom"), 1.0);
    assert_eq!(drawn_depth(&plan), 7, "setup");
    assert!(
        !rig.pool.is_resident(rig.pool.key(NodeId::new(1), 5)),
        "setup"
    );
    // The fit caps the node below the resident level; the capped level is not resident yet.
    rig.pool.caps.insert(NodeId::new(1), 5);
    let plan = rig.step(&cut, &camera("zoom"), 1.0);
    assert!(
        rig.pool.caps.contains_key(&NodeId::new(1)),
        "the cap stands"
    );
    assert_eq!(drawn_depth(&plan), 7, "the finer resident level stands in");
    assert_eq!(plan.apply, CutApply::Applied);
}

#[test]
fn toggling_continuity_requests_only_levels_of_the_pyramid_and_leaves_no_hole() {
    let cut = ids(&[1]);
    let cam = camera("fit");
    let mut rig = Rig::new(root_node(), 64, 16, 2);
    rig.run(12, &cut, &cam, 1.0);
    for min in [Some(SIDE / 16.0), None, Some(SIDE / 8.0), None] {
        rig.pool.set_min_side(min);
        for frame in 0..10 {
            let plan = rig.step(&cut, &cam, 1.0);
            assert_eq!(plan.counters.nodes_without_level, 0, "{min:?} {frame}");
            assert_covers_once(&plan.entries[0]);
            assert!(
                plan.fetch_requests
                    .iter()
                    .all(|key| rig.nodes[&key.node].stored.contains(&key.depth))
            );
        }
    }
}

#[test]
fn the_minimum_side_is_in_dataset_units_whatever_the_visual_scale() {
    // The pool works in dataset units (the eye is divided by the visual scale), like the grid and
    // the reference spacing, so a scaled view must pick the same levels as an unscaled one.
    let scale = 2.0;
    let mut camera = OrbitCamera::default();
    let bounds = crate::SceneBounds {
        min: [0.0; 3],
        max: [SIDE * scale; 3],
    };
    assert!(camera.fit(bounds, crate::scene_digest_tests::VIEWPORT));
    let scaled = ViewProjection::new(
        camera,
        crate::scene_digest_tests::VIEWPORT,
        scale,
        crate::VoxelBuildLimits::default(),
    );
    let cut = ids(&[1]);
    let min = SIDE / 16.0;
    let mut literal = Rig::new(root_node(), 64, 16, 1);
    let mut rig = Rig::new(root_node(), 64, 16, 1);
    rig.pool.set_min_side(Some(min));
    let (mut lit, mut cont) = (None, None);
    for _ in 0..12 {
        lit = Some(literal.step_view(&cut, &scaled, 1.0));
        cont = Some(rig.step_view(&cut, &scaled, 1.0));
    }
    assert_eq!(drawn_depth(&lit.unwrap()), 5);
    let depth = drawn_depth(&cont.unwrap());
    assert_eq!(depth, 4);
    assert!(SIDE / f64::from(1_u32 << depth) >= min);
}

#[test]
fn a_tight_pool_still_reports_its_coarsening_with_continuity_on_and_off() {
    // Two sibling-level fixtures: the pool fit coarsens the far octant nodes; continuity at
    // depth 6 (above every stored level but the finest) must neither hide nor add to it.
    let cut = ids(&CHILDREN);
    let cam = camera("zoom");
    let mut counts = Vec::new();
    for min in [None, Some(SIDE / 64.0), Some(SIDE / 16.0)] {
        let mut rig = Rig::new(octant_nodes(), 44, 4, 2);
        rig.pool.set_min_side(min);
        let plan = rig.run(240, &cut, &cam, 4.0);
        assert_eq!(plan.apply, CutApply::Applied, "{min:?}");
        counts.push((plan.counters.pool_coarsened, plan.counters.stages));
    }
    // The literal run is the pool-pressure baseline of `a_still_pose_converges_...`: 2 nodes.
    assert_eq!(counts[0], (2, 0));
    // A minimum finer than the wanted depth caps nothing: the same coarsening, no stages.
    assert_eq!(counts[1], counts[0]);
    // A cap that lowers the wanted depth may relieve the pool but never shows up as a stage, and
    // what is still coarsened is the pool's doing.
    assert_eq!(counts[2].1, 0);
    assert!(counts[2].0 <= counts[0].0, "{counts:?}");
}

#[test]
fn mask_updates_carry_active_and_node_depth_in_cell_mode() {
    let cam = camera("fit");
    let node_depth = representative_depth(&grid(), cube(ORIGIN, SIDE)).saturating_sub(3);
    let sent = |plan: &FramePlan| {
        plan.mask_updates
            .iter()
            .map(|u| (u.slot, u.active, u.node_depth))
            .collect::<Vec<_>>()
    };
    let mut rig = Rig::new(root_node(), 64, 8, 0);
    let mut updates = Vec::new();
    let mut plan = rig.step(&ids(&[1]), &cam, 64.0);
    for _ in 0..12 {
        updates.extend(sent(&plan));
        plan = rig.step(&ids(&[1]), &cam, 64.0);
    }
    let entry = &plan.entries[0];
    let active = entry
        .required_levels()
        .into_iter()
        .fold(0_u64, |bits, depth| bits | 1 << (depth - node_depth));
    assert_ne!(active, 0);
    let last = updates.last().expect("a node record is sent");
    assert_eq!((last.1, last.2), (active, node_depth), "{updates:?}");
    // One active level per active depth, drawn whole by the GPU's per-cell choice.
    assert!(!plan.active.is_empty());
    assert!(plan.active.iter().all(|level| level.octant_bits == 0xff));
    assert_eq!(plan.active.len(), active.count_ones() as usize);
    // Steady frames send nothing.
    assert!(plan.mask_updates.is_empty());
}

#[test]
fn cell_mode_clamps_the_pool_to_the_node_span() {
    // A leaf of node depth >= 2 whose only stored (and resident) level lies below the node.
    let bounds = cube(ORIGIN, SIDE / 4.0);
    let node_depth = representative_depth(&grid(), bounds).saturating_sub(3);
    assert!(node_depth >= 2);
    let below = node_depth - 1;
    let nodes = HashMap::from([(
        NodeId::new(1),
        PoolNode {
            bounds,
            stored: vec![below],
            buildable: (below, node_depth + 6),
            points: 500,
            octant_data_bounds: None,
        },
    )]);
    let cam = camera("fit");
    let mut rig = Rig::new(nodes, 64, 8, 0);
    seed_resident(&mut rig.pool, 1, &[below]);
    let plan = rig.step(&ids(&[1]), &cam, 64.0);
    assert!(plan.active.is_empty(), "{:?}", plan.active);
    assert!(
        plan.level_requests
            .contains(&rig.pool.key(NodeId::new(1), node_depth)),
        "{:?}",
        plan.level_requests
    );
    assert_eq!(
        plan.level_requests.first().map(|key| key.depth),
        Some(node_depth)
    );
}

/// A leaf whose first stored level lies below its node depth, so no cell can draw it.
fn leaf_below_its_node() -> (HashMap<NodeId, PoolNode>, u8, u8) {
    let bounds = cube(ORIGIN, SIDE / 4.0);
    let node_depth = representative_depth(&grid(), bounds).saturating_sub(3);
    assert!(node_depth >= 2);
    let below = node_depth - 1;
    let nodes = HashMap::from([(
        NodeId::new(1),
        PoolNode {
            bounds,
            stored: vec![below, node_depth + 2],
            buildable: (below, node_depth + 6),
            points: 500,
            octant_data_bounds: None,
        },
    )]);
    (nodes, node_depth, below)
}

#[test]
fn cell_mode_mask_updates_match_the_emitted_active_levels() {
    let (nodes, node_depth, below) = leaf_below_its_node();
    let cam = camera("fit");
    let mut rig = Rig::new(nodes, 64, 8, 0);
    // A resident level below the node (never drawn) and the requested ones as they arrive.
    seed_resident(&mut rig.pool, 1, &[below]);
    let mut checked = 0;
    for _ in 0..12 {
        let plan = rig.step(&ids(&[1]), &cam, 64.0);
        for update in &plan.mask_updates {
            let bits = plan
                .active
                .iter()
                .filter(|level| level.mask_slot == update.slot)
                .fold(0_u64, |bits, level| {
                    bits | 1 << (level.key.depth - node_depth)
                });
            assert_eq!(update.active, bits, "{update:?} vs {:?}", plan.active);
            checked += 1;
        }
    }
    assert!(checked > 0);
}

#[test]
fn cell_mode_fit_cap_reasons_about_the_depths_the_node_can_use() {
    // A capped leaf drew its resident level `near` (above the cap) and now wants a depth below
    // the node, which cell mode clamps to the node depth: it lacks the in-span level that serves
    // it, so `near` keeps drawing. The stored level below the node must not serve the want.
    let bounds = cube(ORIGIN, SIDE / 4.0);
    let node_depth = representative_depth(&grid(), bounds).saturating_sub(3);
    assert!(node_depth >= 2);
    let (below, cap, near) = (node_depth - 1, node_depth + 1, node_depth + 3);
    let id = NodeId::new(1);
    let node = PoolNode {
        bounds,
        stored: vec![below, cap, near],
        buildable: (below, near),
        points: 500,
        octant_data_bounds: None,
    };
    let mut pool = CellPool::new(PoolConfig::default());
    seed_resident(&mut pool, 1, &[below, near]);
    pool.applied_drawn.insert(id, [Some(near); 8]);
    let depths = FastMap::from_iter([(
        id,
        NodeDepths {
            node: node.clone(),
            depths: [below; 8],
            far: (0, 0),
            hi_d: None,
            epoch: 0,
            pose: Pose {
                eye: [0.0; 3],
                forward: [0.0, 0.0, 1.0],
            },
            depth: 0.0,
            exact: true,
            scale: 1.0,
        },
    )]);
    let grid = grid();
    let ctx = Ctx {
        grid: &grid,
        origin: ORIGIN,
        depths: &depths,
        evaluations: Cell::new(0),
        capped: true,
    };
    let cut_node = CellCutNode {
        bounds,
        buildable: (below, cap),
        stored: vec![below, cap],
        resident: vec![below],
    };
    // The level that serves the clamped want (the node depth is not stored: `cap` serves it).
    let needed = FastSet::from_iter([pool.key(id, cap)]);
    assert_eq!(
        pool.finer_drawn(id, cap, &cut_node, &needed, &ctx),
        Some(vec![near])
    );
}

/// One frame of the pool alone: the fake GPU cannot follow levels that were seeded by hand.
fn d5_step(rig: &mut Rig, cut: &[NodeId], camera: &OrbitCamera) -> FramePlan {
    d5_step_at(rig, cut, camera, 64.0)
}

fn mask_of(plan: &FramePlan, node: u64) -> [u64; 8] {
    plan.entries
        .iter()
        .find(|entry| entry.node == NodeId::new(node))
        .expect("entry")
        .mask
}

/// Parent 1 (node depth 0, rep 3) with a coarse resident level 2 only; child 21 (rep 6) fully
/// resident. The D5 depth of the parent is 4.
fn d5_pair() -> Rig {
    let mut rig = d5_rig(HashMap::from([
        (NodeId::new(1), d5_node(SIDE, &[2, 4, 5])),
        (NodeId::new(21), d5_node(SIDE / 8.0, &[6, 7])),
    ]));
    seed_resident(&mut rig.pool, 1, &[2]);
    seed_resident(&mut rig.pool, 21, &[6, 7]);
    rig
}

#[test]
fn children_wait_for_the_parent_d5_level() {
    let cam = camera("fit");
    let cut = ids(&[1, 21]);
    let mut rig = d5_pair();
    let plan = d5_step(&mut rig, &cut, &cam);
    assert_eq!(plan.apply, CutApply::Applied);
    // The child is held: it neither draws nor masks the parent, which draws its whole zone.
    assert_eq!(plan.counters.held_nodes, 1);
    assert_eq!(mask_of(&plan, 1), [0; 8]);
    assert!(plan.active.iter().all(|l| l.key.node != NodeId::new(21)));
    assert!(plan.active.iter().any(|l| l.key.node == NodeId::new(1)));
    // The parent's D5 depth is requested first.
    assert_eq!(
        plan.fetch_requests.first(),
        Some(&rig.pool.key(NodeId::new(1), 4)),
        "{:?}",
        plan.fetch_requests
    );
    // It stays so while the level is missing, without oscillating.
    for _ in 0..3 {
        let plan = d5_step(&mut rig, &cut, &cam);
        assert_eq!(plan.counters.held_nodes, 1);
        assert_eq!(mask_of(&plan, 1), [0; 8]);
    }
    seed_more(&mut rig, 1, &[4]);
    let plan = d5_step(&mut rig, &cut, &cam);
    assert_eq!(plan.counters.held_nodes, 0);
    assert_ne!(mask_of(&plan, 1), [0; 8]);
    assert!(plan.active.iter().any(|l| l.key.node == NodeId::new(21)));
}

/// `add_d5_gate` in `eval_hold` on a multi-node cut: the child 21 is held, and node 1 recorded as
/// its covering ancestor, while node 1 draws but has no resident D5 depth, whether or not the
/// applied cut already drew node 1. Node 21 has levels deeper than anything node 1 drew, so the
/// refine hold releases it: only the gate holds it (without `add_d5_gate` every leg fails).
/// That the hold ends once the D5 depth is resident is `children_wait_for_the_parent_d5_level`.
#[test]
fn the_d5_gate_holds_a_child_while_its_ancestor_has_no_d5_depth() {
    let (child, parent) = (NodeId::new(21), NodeId::new(1));
    let cam = camera("fit");
    for applied in [false, true] {
        let mut rig = d5_pair();
        if applied {
            let plan = d5_step(&mut rig, &ids(&[1]), &cam);
            assert_eq!(plan.apply, CutApply::Applied);
            assert!(!rig.pool.applied_drawn.is_empty(), "setup");
        }
        for wish in [5, 7] {
            let (eval, _) = eval_with(&rig, &ids(&[1, 21]), wish);
            let case = format!("applied {applied} wish {wish}");
            assert_eq!(eval.hold, FastSet::from_iter([child]), "{case}");
            assert_eq!(eval.settle, FastSet::from_iter([child]), "{case}");
            assert_eq!(eval.ancestors, FastSet::from_iter([parent]), "{case}");
        }
    }
}

#[test]
fn the_d5_request_comes_first() {
    let cam = camera("fit");
    let cut = ids(&[1, 21, 22]);
    let mut rig = d5_rig(HashMap::from([
        (NodeId::new(1), d5_node(SIDE, &[2, 4, 5])),
        (NodeId::new(21), d5_node(SIDE / 8.0, &[6, 7])),
        // A node that already draws (level 5) and wants its coarser level 3, shallower than the D5
        // depth (4): by depth alone that request would come before the parent's.
        (
            NodeId::new(22),
            PoolNode {
                bounds: cube([ORIGIN[0] + SIDE / 2.0, ORIGIN[1], ORIGIN[2]], SIDE / 2.0),
                stored: vec![3, 5],
                buildable: (3, 5),
                points: 2000,
                octant_data_bounds: None,
            },
        ),
    ]));
    seed_resident(&mut rig.pool, 1, &[2]);
    seed_resident(&mut rig.pool, 21, &[6, 7]);
    seed_resident(&mut rig.pool, 22, &[5]);
    let plan = d5_step_at(&mut rig, &cut, &cam, 1.0e6);
    let d5 = rig.pool.key(NodeId::new(1), 4);
    let other = rig.pool.key(NodeId::new(22), 3);
    let position = |key| plan.fetch_requests.iter().position(|k| *k == key);
    assert!(position(other).is_some(), "{:?}", plan.fetch_requests);
    assert_eq!(position(d5), Some(0), "{:?}", plan.fetch_requests);
}

#[test]
fn the_pool_fit_counts_the_d5_level() {
    let cam = camera("fit");
    let cut = ids(&[1, 21]);
    let nodes = HashMap::from([
        (NodeId::new(1), d5_node(SIDE, &[2, 4, 5])),
        (NodeId::new(21), d5_node(SIDE / 8.0, &[6, 7])),
    ]);
    let mut rig = Rig::new(nodes, 64, 8, 10_000);
    seed_resident(&mut rig.pool, 1, &[2]);
    seed_resident(&mut rig.pool, 21, &[6, 7]);
    rig.pool
        .set_level_sizes(NodeId::new(1), &[(2, 1), (4, 22), (5, 22)]);
    // A coarse want: node 1 wants level 2 only, so its D5 level 4 is not wanted by the entry.
    d5_step_at(&mut rig, &cut, &cam, 1.0e6);
    let memo = rig.pool.wanted_memo.as_ref().expect("fit ran");
    // The D5 level takes pages (a fetch under pressure), so the fit must count it.
    let d5 = rig.pool.key(NodeId::new(1), 4);
    assert!(memo.needed.contains(&d5), "{:?}", memo.needed);
}

#[test]
fn the_d5_depth_joins_the_active_set_when_the_mask_is_set() {
    let cam = camera("fit");
    let cut = ids(&[1, 21]);
    let mut rig = d5_pair();
    // A resident level above the D5 depth that nothing else makes active.
    seed_more(&mut rig, 1, &[4]);
    // A coarse want: the entry draws level 2 only, so level 4 is active only for the gate.
    let plan = d5_step_at(&mut rig, &cut, &cam, 1.0e6);
    assert_ne!(mask_of(&plan, 1), [0; 8]);
    let entry = plan
        .entries
        .iter()
        .find(|e| e.node == NodeId::new(1))
        .unwrap();
    assert!(
        !entry.required_levels().contains(&4),
        "{:?}",
        entry.required_levels()
    );
    let active = active_depths(&plan, &rig, 1);
    assert!(active.contains(&4), "{active:?}");
    let update = plan
        .mask_updates
        .iter()
        .find(|u| Some(u.slot) == rig.pool.mask_slots.get(&NodeId::new(1)).map(|s| s.slot))
        .expect("update");
    assert_ne!(update.active & (1 << 4), 0, "{update:?}");
}

#[test]
fn the_d5_gate_settles_within_hold_passes() {
    let cam = camera("fit");
    let cut = ids(&[1, 2, 21]);
    let mut rig = d5_rig(HashMap::from([
        (NodeId::new(1), d5_node(SIDE, &[2, 4, 5])),
        (NodeId::new(2), d5_node(SIDE / 2.0, &[2, 5, 6])),
        (NodeId::new(21), d5_node(SIDE / 8.0, &[6, 7])),
    ]));
    seed_resident(&mut rig.pool, 1, &[2]);
    seed_resident(&mut rig.pool, 2, &[2]);
    seed_resident(&mut rig.pool, 21, &[6, 7]);
    for _ in 0..3 {
        let plan = d5_step(&mut rig, &cut, &cam);
        assert_eq!(plan.apply, CutApply::Applied);
        assert_eq!(plan.counters.hold_unsettled, 0);
        assert_eq!(plan.counters.held_nodes, 2);
        assert_eq!(mask_of(&plan, 1), [0; 8]);
        assert_eq!(mask_of(&plan, 2), [0; 8]);
    }
    // The grandparent's D5 releases the parent; the child still waits for the parent's.
    seed_more(&mut rig, 1, &[4]);
    let plan = d5_step(&mut rig, &cut, &cam);
    assert_eq!(plan.counters.hold_unsettled, 0);
    assert_eq!(plan.counters.held_nodes, 1);
    assert_ne!(mask_of(&plan, 1), [0; 8]);
    seed_more(&mut rig, 2, &[5]);
    let plan = d5_step(&mut rig, &cut, &cam);
    assert_eq!(plan.counters.held_nodes, 0);
    assert_eq!(plan.counters.hold_unsettled, 0);
    assert_ne!(mask_of(&plan, 2), [0; 8]);
}

#[test]
fn a_retained_frame_keeps_the_d5_depth_its_record_was_written_with() {
    let cam = camera("fit");
    let cut = ids(&[1, 21]);
    let mut rig = d5_pair();
    seed_more(&mut rig, 1, &[4]);
    let plan = d5_step_at(&mut rig, &cut, &cam, 1.0e6);
    assert_eq!(plan.apply, CutApply::Applied);
    assert_ne!(mask_of(&plan, 1), [0; 8]);
    let applied = active_depths(&plan, &rig, 1);
    assert!(applied.contains(&4), "{applied:?}");
    // The sizes grow past the pool: the fit caps node 1 below the D5 depth and the frame
    // overflows, so it retains the previous cut (masks are not rewritten).
    rig.pool
        .set_level_sizes(NodeId::new(1), &[(2, 300), (4, 600), (5, 600)]);
    rig.pool
        .set_level_sizes(NodeId::new(21), &[(6, 300), (7, 600)]);
    let plan = d5_step_at(&mut rig, &cut, &cam, 64.0);
    assert_eq!(
        plan.apply,
        CutApply::RetainedPrevious,
        "setup {:?} {:?}",
        plan.counters,
        plan.entries
    );
    assert!(plan.mask_updates.is_empty());
    assert_eq!(active_depths(&plan, &rig, 1), applied);
}

/// ADR 0054: the length minimum bounds the wanted depth, even for a leaf the eye is next to and
/// whose source density is far finer.
#[test]
fn min_side_bounds_the_wanted_depth() {
    let min_side = 1e-4;
    // SIDE / 2^19 is the finest side that is not below the minimum.
    let finest_allowed = 19;
    assert!(SIDE / f64::from(1_u32 << finest_allowed) >= min_side);
    assert!(SIDE / f64::from(1_u32 << (finest_allowed + 1)) < min_side);
    let mut nodes = root_node();
    nodes.get_mut(&NodeId::new(1)).unwrap().buildable = (3, 25);
    for cap in [None, Some(min_side)] {
        let mut rig = Rig::new(nodes.clone(), 64, 16, 1);
        rig.pool.set_min_side(cap);
        for eye in [
            [ORIGIN[0] + 20.0, ORIGIN[1] + 30.0, ORIGIN[2] + 10.0],
            [ORIGIN[0] - 0.01, ORIGIN[1] + 30.0, ORIGIN[2] + 10.0],
        ] {
            let mut cam = camera("fit");
            assert!(cam.set_pose(eye, [ORIGIN[0] + 32.0, ORIGIN[1] + 32.0, ORIGIN[2] + 32.0]));
            let plan = rig.run(12, &ids(&[1]), &cam, 4.0);
            for wanted in plan.entries[0].wanted.iter().flatten() {
                match cap {
                    Some(_) => assert!(*wanted <= finest_allowed, "wanted {wanted}"),
                    None => assert!(*wanted > finest_allowed, "setup: uncapped wanted {wanted}"),
                }
            }
        }
    }
}

#[test]
fn cell_mode_activates_resident_depths_across_the_range() {
    // Residents at every stored depth: the active set also holds the resident depths between
    // the node's far (lo) and near (hi) depths that no octant draws.
    let cam = camera("zoom");
    let nodes = HashMap::from([(
        NodeId::new(1),
        d5_node(SIDE, &[2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]),
    )]);
    let mut rig = Rig::new(nodes, 4096, 64, 0);
    seed_resident(&mut rig.pool, 1, &[2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
    rig.pool.residency_epoch += 1;
    let plan = d5_step_at(&mut rig, &ids(&[1]), &cam, 2.0);
    let entry = &plan.entries[0];
    let (lo, hi) = entry.range.expect("a cell-mode node has a range");
    assert!(lo < hi, "{lo}..{hi}");
    let required = entry.required_levels();
    assert!(required.iter().all(|d| *d <= hi));
    assert!(
        (lo..=hi).any(|d| !required.contains(&d)),
        "the range reaches depths no octant draws: {lo}..{hi} vs {required:?}"
    );
    let expected: Vec<u8> = (lo..=hi)
        .chain(required.iter().copied())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    assert_eq!(active_depths(&plan, &rig, 1), expected);
}

#[test]
fn cell_mode_fetches_stored_depths_across_the_range() {
    // Only the coarsest depth is resident: every other stored depth inside the node's range is
    // fetched, not only the octants' wanted ones.
    let cam = camera("zoom");
    let stored = [2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
    let nodes = HashMap::from([(NodeId::new(1), d5_node(SIDE, &stored))]);
    let mut rig = Rig::new(nodes, 4096, 64, 0);
    seed_resident(&mut rig.pool, 1, &[2]);
    rig.pool.residency_epoch += 1;
    let plan = d5_step_at(&mut rig, &ids(&[1]), &cam, 2.0);
    let (lo, hi) = plan.entries[0].range.expect("a cell-mode node has a range");
    assert!(lo < hi, "{lo}..{hi}");
    let wanted = plan.entries[0].required_levels();
    let fetched: Vec<u8> = plan.fetch_requests.iter().map(|key| key.depth).collect();
    for depth in (lo..=hi).filter(|d| *d != 2) {
        assert!(
            fetched.contains(&depth),
            "depth {depth} of {lo}..{hi}: {fetched:?}"
        );
    }
    assert!(
        (lo..=hi).any(|d| !wanted.contains(&d)),
        "the range reaches depths no octant wants: {lo}..{hi} vs {wanted:?}"
    );
    // Coarse to fine.
    assert!(
        fetched.windows(2).all(|pair| pair[0] <= pair[1]),
        "{fetched:?}"
    );
}

/// A node whose range reaches below its stored chain: depths 7 and 8 are buildable, not stored,
/// and above the spacing depth (the finest stored one, 9); 10 and 11 are below it.
fn range_gap_rig() -> Rig {
    let mut node = d5_node(SIDE, &[2, 3, 4, 9]);
    node.buildable = (2, 14);
    node.points = 512 * 64;
    let mut rig = Rig::new(HashMap::from([(NodeId::new(1), node)]), 4096, 64, 0);
    seed_resident(&mut rig.pool, 1, &[2]);
    rig.pool.residency_epoch += 1;
    rig
}

#[test]
fn cell_mode_requests_missing_depths_across_the_range_down_to_spacing() {
    let mut rig = range_gap_rig();
    let plan = d5_step_at(&mut rig, &ids(&[1]), &camera("zoom"), 2.0);
    let (lo, hi) = plan.entries[0].range.expect("a cell-mode node has a range");
    assert_eq!((lo, hi), (7, 11), "setup");
    let requested: Vec<u8> = plan.level_requests.iter().map(|key| key.depth).collect();
    for depth in [7, 8] {
        assert!(requested.contains(&depth), "{depth} missing: {requested:?}");
    }
    // 10 lies in the range beyond the spacing depth and no octant wants it: not built.
    assert!(!requested.contains(&10), "{requested:?}");
    // Octant (near) depths keep their behaviour: 11 is wanted by an octant.
    assert!(requested.contains(&11), "{requested:?}");
    // Coarse to fine.
    assert_eq!(requested, [7, 8, 11]);
    // Not stored, so nothing is fetched for them.
    let fetched: Vec<u8> = plan.fetch_requests.iter().map(|key| key.depth).collect();
    assert!(
        !fetched.contains(&7) && !fetched.contains(&8),
        "{fetched:?}"
    );
}

/// Node with stored {2, 3, 4, 9}, buildable 2..=14; `resident` are seeded before the frame.
fn gap_rig_with(resident: &[u8]) -> Rig {
    let mut node = d5_node(SIDE, &[2, 3, 4, 9]);
    node.buildable = (2, 14);
    node.points = 512 * 64;
    let mut rig = Rig::new(HashMap::from([(NodeId::new(1), node)]), 4096, 64, 0);
    seed_resident(&mut rig.pool, 1, resident);
    rig.pool.residency_epoch += 1;
    rig
}

#[test]
fn cell_mode_fills_gaps_between_active_depths() {
    // 7 and 11 are resident and active (range 7..=11): 8 and 10 lie between them, so they are
    // requested although 10 is below the spacing depth 9; 9 is stored and fetched instead.
    let mut rig = gap_rig_with(&[2, 7, 11]);
    let plan = d5_step_at(&mut rig, &ids(&[1]), &camera("zoom"), 2.0);
    let requested: Vec<u8> = plan.level_requests.iter().map(|key| key.depth).collect();
    for depth in [8, 10] {
        assert!(requested.contains(&depth), "{depth} missing: {requested:?}");
    }
    assert!(!requested.contains(&9), "{requested:?}");
    assert!(
        !requested.contains(&12),
        "nothing past the finest active depth: {requested:?}"
    );
}

#[test]
fn cell_mode_fills_gaps_below_spacing_for_a_leaf() {
    // A leaf whose stored chain ends at 9 and whose resident depths are 9 and 11: 10 is below
    // the spacing depth and gets built because it lies between two active depths.
    let mut rig = gap_rig_with(&[2, 9, 11]);
    let plan = d5_step_at(&mut rig, &ids(&[1]), &camera("zoom"), 2.0);
    let requested: Vec<u8> = plan.level_requests.iter().map(|key| key.depth).collect();
    assert!(requested.contains(&10), "{requested:?}");
    assert!(!requested.contains(&12), "{requested:?}");
}

#[test]
fn a_node_requests_its_range_up_to_spacing_and_the_wanted_depths() {
    // The octants wish only the ends of the range (7 and 11) and `hi` stops at 10: the node range
    // is `[7, 11]`. Every depth up to the spacing depth (9, stored) is requested or fetched, the
    // wanted 11 beyond it is requested, and the unwanted 10 is not.
    let mut rig = range_gap_rig();
    let plan = d5_step_at(&mut rig, &ids(&[1]), &camera("zoom"), 2.0);
    let mut entry = plan.entries[0].clone();
    entry.wanted = [None; 8];
    entry.wanted[0] = Some(7);
    entry.wanted[1] = Some(11);
    entry.range = Some((7, 10));
    assert_eq!(CellPool::node_range(&entry), Some((7, 11)));
    let nodes: FastMap<NodeId, PoolNode> =
        rig.nodes.iter().map(|(id, n)| (*id, n.clone())).collect();
    let none = FastMap::default();
    let entries = [entry];
    let key = |depth: u8| rig.pool.key(NodeId::new(1), depth);
    let requested = rig.pool.requested_levels(&entries, &nodes, &none, &grid());
    for depth in [7, 8, 11] {
        assert!(requested.contains(&key(depth)), "{depth}: {requested:?}");
    }
    // 9 is stored, so it is fetched as the level that serves the range, not requested; 10 is
    // beyond the spacing depth and nobody wants it.
    assert!(!requested.contains(&key(9)), "{requested:?}");
    assert!(!requested.contains(&key(10)), "{requested:?}");
    let needed = rig.pool.needed_levels(&entries, &nodes, &none, &grid());
    assert!(needed.contains(&key(9)), "{needed:?}");
}

#[test]
fn the_fit_counts_requested_range_depths() {
    let mut rig = range_gap_rig();
    let plan = d5_step_at(&mut rig, &ids(&[1]), &camera("zoom"), 2.0);
    let nodes: FastMap<NodeId, PoolNode> =
        rig.nodes.iter().map(|(id, n)| (*id, n.clone())).collect();
    let none = FastMap::default();
    let (needed, total) =
        rig.pool
            .needed_and_pages(&plan.entries, &nodes, &none, &grid(), &FastSet::default());
    let pages = |depth: u8| {
        rig.pool.pages_of(
            rig.pool.key(NodeId::new(1), depth),
            &nodes[&NodeId::new(1)],
            &grid(),
        )
    };
    let needed_pages: usize = needed
        .iter()
        .map(|key| rig.pool.pages_of(*key, &nodes[&key.node], &grid()))
        .sum();
    for depth in [7, 8] {
        assert!(!needed.contains(&rig.pool.key(NodeId::new(1), depth)));
    }
    // The counted set is the parent's: the spacing-limited range depths. The wanted depth 11
    // lies beyond the spacing depth and is requested but not counted, as before.
    assert_eq!(total as usize, needed_pages + pages(7) + pages(8));
}
