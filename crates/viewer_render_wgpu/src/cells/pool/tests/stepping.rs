//! ADR 0053: stepping through pruned residents, timeouts and hops. `Rig::step` checks every frame
//! against the stepping oracle, so these scenarios only pin down when the hops happen.

use super::*;

/// Every octant of node 2 wants the same depth, so a hop counts once per octant.
const OCTANTS: u32 = 8;

/// One frame of node 2 alone in the zoom camera: `(drawn, step, step_hops)` of its octant 0.
fn step_node_2(rig: &mut Rig, target: f64) -> (Option<u8>, Option<u8>, u32) {
    let plan = rig.step(&ids(&[2]), &camera("zoom"), target);
    plan.entries.first().map_or((None, None, 0), |e| {
        (e.drawn[0], e.step[0], plan.counters.step_hops)
    })
}

/// Node 2 draws depth 7 (target 4) with every depth resident; `gone` are evicted and `refused`
/// never arrive again, then the view wants depth 4 (target 64) or 5 (target 32).
fn node_2_drawing_7(gone: &[u8], refused: &[u8]) -> Rig {
    let mut nodes = octant_nodes();
    let node = nodes.get_mut(&NodeId::new(2)).expect("node 2");
    node.stored = vec![3, 4, 5, 6, 7];
    node.buildable = (3, 7);
    let mut rig = Rig::new(nodes, 256, 64, 0);
    // Walk the depths up, so that each one is resident.
    for target in [64.0, 32.0, 16.0, 4.0] {
        for _ in 0..30 {
            step_node_2(&mut rig, target);
        }
    }
    assert_eq!(step_node_2(&mut rig, 4.0), (Some(7), None, 0), "setup");
    for depth in gone {
        assert!(rig.pool.evict(rig.pool.key(NodeId::new(2), *depth)));
    }
    rig.refused = refused
        .iter()
        .map(|depth| rig.pool.key(NodeId::new(2), *depth))
        .collect();
    rig
}

#[test]
fn a_level_that_never_arrives_is_skipped_after_the_wait_with_one_counted_hop() {
    // Depths 6 and 5 are gone and refused: 7 waits for 6, then hops to the closest resident (4).
    let mut rig = node_2_drawing_7(&[5, 6], &[5, 6]);
    for frame in 0..STEP_WAIT_FRAMES {
        assert_eq!(
            step_node_2(&mut rig, 64.0),
            (Some(7), Some(6), 0),
            "frame {frame}"
        );
    }
    assert_eq!(
        step_node_2(&mut rig, 64.0),
        (Some(4), None, OCTANTS),
        "the hop"
    );
    assert_eq!(step_node_2(&mut rig, 64.0), (Some(4), None, 0));
}

#[test]
fn the_wait_survives_a_wanted_depth_that_flips_towards_the_same_side() {
    // Wanted 4 and 5 alternate every frame (both below the drawn 7, awaiting 6): one wait.
    let mut rig = node_2_drawing_7(&[5, 6], &[5, 6]);
    let targets = [64.0, 32.0];
    for frame in 0..STEP_WAIT_FRAMES as usize {
        let (drawn, step, hops) = step_node_2(&mut rig, targets[frame % 2]);
        assert_eq!((drawn, step, hops), (Some(7), Some(6), 0), "frame {frame}");
    }
    let (drawn, _, hops) = step_node_2(&mut rig, targets[0]);
    assert_eq!(
        (drawn, hops),
        (Some(4), OCTANTS),
        "the wait did not restart"
    );
}

#[test]
fn each_hop_gets_its_own_wait_and_a_refused_level_never_reads_as_rest() {
    // 6 and 4 are gone and refused, 5 stays: 7 hops to 5 after one wait, then 4 (still wanted,
    // never delivered) is waited for in turn, and the octant stays pending.
    let mut rig = node_2_drawing_7(&[6, 4], &[6, 4]);
    for frame in 0..STEP_WAIT_FRAMES {
        assert_eq!(
            step_node_2(&mut rig, 64.0),
            (Some(7), Some(6), 0),
            "frame {frame}"
        );
    }
    assert_eq!(
        step_node_2(&mut rig, 64.0),
        (Some(5), Some(4), OCTANTS),
        "first hop"
    );
    for frame in 0..3 * STEP_WAIT_FRAMES {
        assert_eq!(
            step_node_2(&mut rig, 64.0),
            (Some(5), Some(4), 0),
            "frame {frame}"
        );
    }
}

#[test]
fn an_adjacent_level_arriving_during_the_wait_is_taken_and_restarts_the_wait() {
    // 6 arrives at frame 10: 7 -> 6 normally; 5 never arrives, so the next hop comes a full wait
    // after reaching 6, to the resident 4.
    let mut rig = node_2_drawing_7(&[5, 6], &[5, 6]);
    for _ in 0..10 {
        step_node_2(&mut rig, 64.0);
    }
    rig.refused.remove(&rig.pool.key(NodeId::new(2), 6));
    let mut frames = Vec::new();
    for _ in 0..3 * STEP_WAIT_FRAMES {
        frames.push(step_node_2(&mut rig, 64.0));
    }
    let reached_6 = frames.iter().position(|f| f.0 == Some(6)).expect("6 drawn");
    assert!(reached_6 < 4, "{reached_6}");
    assert!(
        frames[..reached_6]
            .iter()
            .all(|f| f.0 == Some(7) && f.2 == 0)
    );
    let reached_4 = frames.iter().position(|f| f.0 == Some(4)).expect("4 drawn");
    assert_eq!(
        reached_4 - reached_6,
        STEP_WAIT_FRAMES as usize,
        "{frames:?}"
    );
    assert_eq!(frames.iter().map(|f| f.2).sum::<u32>(), OCTANTS);
}

#[test]
fn retained_frames_advance_the_step_wait() {
    // 7 waits for 6, which never arrives. A pose jump to a cut that cannot draw yet keeps the
    // previous one on screen (`RetainedPrevious`); those frames count towards the wait, so back
    // on node 2 the octants hop at once to the resident 4.
    let mut rig = node_2_drawing_7(&[5, 6], &[5, 6]);
    assert_eq!(
        step_node_2(&mut rig, 64.0),
        (Some(7), Some(6), 0),
        "the wait starts"
    );
    rig.held.extend([NodeId::new(6), NodeId::new(7)]);
    for frame in 1..STEP_WAIT_FRAMES {
        let plan = rig.step(&ids(&[6, 7]), &camera("zoom"), 64.0);
        assert_eq!(plan.apply, CutApply::RetainedPrevious, "frame {frame}");
        assert_eq!(plan.counters.step_hops, 0, "frame {frame}");
    }
    assert_eq!(
        step_node_2(&mut rig, 64.0),
        (Some(4), None, OCTANTS),
        "the retained frames waited"
    );
}

#[test]
fn a_node_activates_the_contiguous_resident_run_of_its_range() {
    // Only depth 9 serves the node when it enters the cut (range 7..=11), so it draws 9 alone.
    // A range depth beyond a residency gap stays inactive (ADR 0055, proposed): with 10 missing,
    // 11 would make the per-cell rule jump over it. Once the range is resident it is all
    // active at once: no window of one level per frame.
    let nodes = HashMap::from([(
        NodeId::new(1),
        d5_node(SIDE, &[2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]),
    )]);
    let mut rig = Rig::new(nodes, 4096, 64, 10_000);
    seed_resident(&mut rig.pool, 1, &[2, 9]);
    rig.pool.residency_epoch += 1;
    let cam = camera("zoom");
    let plan = d5_step_at(&mut rig, &ids(&[1]), &cam, 2.0);
    assert_eq!(plan.entries[0].range, Some((7, 11)), "setup");
    assert_eq!(active_depths(&plan, &rig, 1), [9], "entering the cut");
    seed_more(&mut rig, 1, &[11]);
    let gapped = active_depths(&d5_step_at(&mut rig, &ids(&[1]), &cam, 2.0), &rig, 1);
    assert_eq!(gapped, [9], "11 lies beyond the gap at 10");
    seed_more(&mut rig, 1, &[7, 8, 10]);
    let full = active_depths(&d5_step_at(&mut rig, &ids(&[1]), &cam, 2.0), &rig, 1);
    assert_eq!(full, [7, 8, 9, 10, 11], "the whole range is resident");
}

/// Children of node 2 (ids 10..=17) that store depths 4..=10, with node 2 drawing depth 6 under
/// `[2, 3]` first. Returns the rig and the cut of the children.
fn children_entering_below_node_2() -> (Rig, Vec<NodeId>) {
    let mut nodes = two_regions_with_coarse_children();
    for child in region_children(10) {
        let node = nodes.get_mut(&NodeId::new(child)).expect("child");
        node.stored = (4..=10).collect();
        node.buildable = (4, 10);
    }
    let mut rig = Rig::new(nodes, 4096, 64, 0);
    let plan = rig.run(60, &ids(&[2, 3]), &camera("fit"), 8.0);
    let drawn = depth_of(&plan, 2)
        .into_iter()
        .flatten()
        .max()
        .expect("node 2 draws");
    assert_eq!(drawn, 6, "setup");
    let mut cut = ids(&region_children(10));
    cut.push(NodeId::new(3));
    (rig, cut)
}

/// The first plan in which every child draws something, and what each child drew in it.
fn first_drawn_children(rig: &mut Rig, cut: &[NodeId], target: f64) -> (FramePlan, Vec<u8>) {
    for _ in 0..30 {
        let plan = rig.step(cut, &camera("fit"), target);
        let drawn: Vec<Vec<u8>> = region_children(10)
            .into_iter()
            .map(|child| {
                plan.entries
                    .iter()
                    .find(|e| e.node.get() == child)
                    .map_or_else(Vec::new, |e| e.drawn.iter().flatten().copied().collect())
            })
            .collect();
        if drawn.iter().all(|d| !d.is_empty()) {
            return (plan, drawn.into_iter().flatten().collect());
        }
    }
    panic!("the children never drew");
}

#[test]
fn the_stepping_oracle_runs_in_cell_mode() {
    // The children enter at 6 or 7 and want 10 through resident levels: they step one level per
    // frame, and the cell branch of the oracle checks every one of those frames.
    let (mut rig, cut) = children_entering_below_node_2();
    let before = rig.cell_checked;
    let (_, first) = first_drawn_children(&mut rig, &cut, 0.25);
    let mut deepest = first.into_iter().max().expect("children draw");
    let mut steps = 0;
    for _ in 0..30 {
        let plan = rig.step(&cut, &camera("fit"), 0.25);
        let now = region_children(10)
            .into_iter()
            .flat_map(|child| depth_of(&plan, child))
            .flatten()
            .max()
            .expect("children draw");
        if now > deepest {
            assert_eq!(now, deepest + 1, "one level per frame");
            steps += 1;
        }
        deepest = deepest.max(now);
    }
    assert!(steps > 0, "no drawn depth stepped towards the wanted one");
    assert!(
        rig.cell_checked > before,
        "the cell branch of the oracle checked nothing"
    );
    rig.assert_stepping_was_checked();
}

#[test]
fn an_entering_leaf_starts_one_level_below_its_ancestor() {
    // Node 2 drew depth 6: its children enter at 6 or 7 (one level from it) although they want
    // 10 and the level is as available as the others.
    let (mut rig, cut) = children_entering_below_node_2();
    let (_, drawn) = first_drawn_children(&mut rig, &cut, 0.25);
    assert!(drawn.iter().all(|d| (6..=7).contains(d)), "{drawn:?}");
}

#[test]
fn a_leaving_node_hands_back_one_level() {
    // The children drew from depth 7 and the cut returns to node 2: it restarts one level below
    // their coarsest drawn depth, not at the level closest to what it wants.
    let (mut rig, cut) = children_entering_below_node_2();
    first_drawn_children(&mut rig, &cut, 0.25);
    // Node 2 has left the cut by now; the children are still at their first depth.
    let last = rig.step(&cut, &camera("fit"), 0.25);
    let coarsest = region_children(10)
        .into_iter()
        .flat_map(|child| depth_of(&last, child))
        .flatten()
        .min()
        .expect("children draw");
    let mut plan = rig.step(&ids(&[2, 3]), &camera("fit"), 64.0);
    while !has_entry(&plan, 2) {
        plan = rig.step(&ids(&[2, 3]), &camera("fit"), 64.0);
    }
    let first: Vec<u8> = depth_of(&plan, 2).into_iter().flatten().collect();
    assert!(
        !first.is_empty() && first.iter().all(|d| *d + 1 == coarsest),
        "{first:?}"
    );
}

/// The eight octant nodes with stored (and buildable) depths 4..=12: in the zoom camera the
/// near ones get a range of several depths.
fn wide_octant_nodes() -> HashMap<NodeId, PoolNode> {
    let mut nodes = octant_nodes();
    for node in nodes.values_mut() {
        node.stored = (4..=12).collect();
        node.buildable = (4, 12);
    }
    nodes
}

/// What the wide-range scenarios must have exercised: nodes whose range has `lo < hi` and an
/// active level that is range-only (active, but not the depth any octant draws).
fn count_range_depths(plan: &FramePlan) -> usize {
    plan.entries
        .iter()
        .filter_map(|entry| entry.range.filter(|(lo, hi)| lo < hi).map(|_| entry))
        .map(|entry| {
            let drawn = entry.required_levels();
            plan.active
                .iter()
                .filter(|level| level.key.node == entry.node && !drawn.contains(&level.key.depth))
                .count()
        })
        .sum()
}

#[test]
fn an_active_range_depth_is_never_evicted_under_pressure() {
    let mut rig = Rig::new(wide_octant_nodes(), 80, 2, 1);
    let cut = ids(&CHILDREN);
    let (mut evictions, mut range_depths_active, mut previously_active) = (0, 0, 0);
    let mut previous: HashSet<LevelKey> = HashSet::new();
    for frame in 0..80 {
        let mut cam = camera("zoom");
        cam.orbit(0.004 * f64::from(frame), 0.0);
        let plan = rig.step(&cut, &cam, 2.0);
        range_depths_active += count_range_depths(&plan);
        for event in &plan.events {
            let CellPoolEvent::Evict(key) = event else {
                continue;
            };
            evictions += 1;
            // An active level that leaves the wanted set may go; one inside the range the cut
            // still gives its node would be active again at once, so it must be kept.
            if !previous.contains(key) {
                continue;
            }
            previously_active += 1;
            let in_range = plan.entries.iter().any(|entry| {
                entry.node == key.node
                    && entry
                        .range
                        .is_some_and(|(lo, hi)| (lo..=hi).contains(&key.depth))
            });
            assert!(
                !in_range,
                "frame {frame}: {key:?} was active, is still in its node's range and was evicted"
            );
        }
        previous = plan.active.iter().map(|level| level.key).collect();
    }
    // Guards: the pool really was under pressure and range-only depths really were active.
    assert!(evictions > 0, "no eviction happened");
    assert!(
        range_depths_active > 0,
        "no range-only active depth occurred"
    );
    assert!(previously_active > 0, "no active level was ever evicted");
    rig.assert_gpu_matches_pool();
    rig.assert_stepping_was_checked();
}

#[test]
fn cap_clamps_the_range_and_fit_reports_it_with_wide_ranges() {
    // The eight near nodes want ranges of several depths in the zoom camera, which do not fit:
    // the fit caps the farthest nodes first and reports it, and no capped node keeps an active
    // depth finer than its cap.
    let mut rig = Rig::new(wide_octant_nodes(), 80, 64, 1);
    let cut = ids(&CHILDREN);
    let cam = camera("zoom");
    let near = by_distance(&cam, &CHILDREN);
    let (mut reported, mut capped_range_depths) = (0, 0);
    for frame in 0..40 {
        let plan = rig.step(&cut, &cam, 2.0);
        if plan.counters.pool_coarsened == 0 {
            continue;
        }
        reported += 1;
        assert_eq!(
            plan.coarsened_nodes.len(),
            plan.counters.pool_coarsened as usize,
            "frame {frame}"
        );
        // Farthest first: the farthest node is capped, and walking from the farthest to the
        // nearest node the caps never get coarser (an uncapped node in between is allowed:
        // the fit skips a node whose coarsening saves no pages).
        let farthest = *near.last().expect("nodes");
        assert!(
            plan.coarsened_nodes.contains(&NodeId::new(farthest)),
            "frame {frame}: {:?}",
            plan.coarsened_nodes
        );
        let caps: Vec<u8> = near
            .iter()
            .rev()
            .filter_map(|id| rig.pool.caps.get(&NodeId::new(*id)).copied())
            .collect();
        assert!(caps.is_sorted(), "frame {frame}: caps far to near {caps:?}");
        for key in plan.fetch_requests.iter().chain(&plan.level_requests) {
            if let Some(cap) = rig.pool.caps.get(&key.node) {
                assert!(
                    key.depth <= *cap,
                    "frame {frame}: {key:?} requested above cap {cap}"
                );
            }
        }
        for entry in &plan.entries {
            let Some(cap) = rig.pool.caps.get(&entry.node) else {
                continue;
            };
            let drawn = entry.required_levels();
            for level in plan.active.iter().filter(|l| l.key.node == entry.node) {
                assert!(
                    level.key.depth <= *cap,
                    "frame {frame}: {:?} above cap {cap}",
                    level.key
                );
                if entry.range.is_some_and(|(lo, hi)| lo < hi) && !drawn.contains(&level.key.depth)
                {
                    capped_range_depths += 1;
                }
            }
        }
    }
    assert!(reported > 0, "the fit never reported a coarsening");
    assert!(
        capped_range_depths > 0,
        "no capped node had a range-only active depth"
    );
    rig.assert_stepping_was_checked();
}

#[test]
fn every_active_depth_is_fully_uploaded_with_a_wide_range() {
    // One page per frame: levels spend several frames partially uploaded, while their node's
    // range already holds resident depths. No frame may activate a level that lacks pages.
    let mut rig = Rig::new(wide_octant_nodes(), 4096, 1, 1);
    let cut = ids(&CHILDREN);
    let cam = camera("zoom");
    let (mut range_depths_active, mut partial) = (0, 0);
    for frame in 0..200 {
        let plan = rig.step(&cut, &cam, 2.0);
        range_depths_active += count_range_depths(&plan);
        let active: HashSet<LevelKey> = plan.active.iter().map(|level| level.key).collect();
        for key in &active {
            assert!(
                rig.pool.is_resident(*key),
                "frame {frame}: {key:?} is not resident"
            );
            let pages = rig.pool.level(*key).expect("level").pages.len();
            let uploaded = (0..pages)
                .filter(|page| rig.gpu.slot_of.contains_key(&(*key, *page)))
                .count();
            assert_eq!(uploaded, pages, "frame {frame}: {key:?} lacks pages");
        }
        partial += rig
            .pool
            .levels
            .iter()
            .filter(|(key, level)| {
                !active.contains(key)
                    && matches!(&level.state, LevelState::Uploading { slots, pages_done }
                        if *pages_done < slots.len())
            })
            .count();
    }
    assert!(
        range_depths_active > 0,
        "no range-only active depth occurred"
    );
    assert!(partial > 0, "no level was ever observed partially uploaded");
    rig.assert_stepping_was_checked();
}

#[test]
fn hold_threshold_is_the_finest_active_depth_end_to_end() {
    // Root 1 draws, in the zoom camera, a range of depths 7..=11 whose octant over child 2 draws
    // depth 8 but whose finest active depth is 11 (the GPU picks per cell among the active
    // ones). Child 2 wants 12: with only 9 or 10 resident it would draw below what the ancestor
    // showed there, so it stays held; it takes over once it can draw the ancestor's finest
    // active depth (11), although that is below the 12 it wants.
    let mut rig = hold_rig();
    let zoom = camera("zoom");
    let mut plan = d5_step_at(&mut rig, &ids(&[1]), &zoom, 2.0);
    for _ in 0..8 {
        plan = d5_step_at(&mut rig, &ids(&[1]), &zoom, 2.0);
    }
    let root = NodeId::new(1);
    let (lo, hi) = plan.entries[0].range.expect("a cell-mode node has a range");
    let finest = rig.pool.applied_finest[&root];
    // Child 2 sits under octant 0 of the root.
    let nominal = rig.pool.applied_drawn[&root][0].expect("octant 0 draws");
    // Guards: a wide range whose finest active depth is finer than the octant's drawn one.
    assert!(lo < hi, "range {lo}..{hi}");
    assert!(active_depths(&plan, &rig, 1).len() > 1);
    assert!(nominal + 2 < finest, "nominal {nominal}, finest {finest}");
    let cut = ids(&CHILDREN);
    let child = NodeId::new(2);
    for (resident, held) in [
        (vec![nominal + 1], true),
        (vec![finest - 1], true),
        (vec![finest], false),
    ] {
        seed_more(&mut rig, 2, &resident);
        for frame in 0..2 {
            let plan = d5_step_at(&mut rig, &cut, &zoom, 0.125);
            let entry = plan
                .entries
                .iter()
                .find(|e| e.node == child)
                .expect("child");
            let wanted = entry.wanted[0].expect("child wants a depth");
            assert!(finest < wanted, "the child wants {wanted}");
            assert_eq!(
                rig.pool.hold.contains(&child),
                held,
                "resident {resident:?} frame {frame}"
            );
            assert_eq!(entry.drawn[0].is_some(), !held, "{resident:?}");
            assert_eq!(rig.pool.applied_finest[&root], finest, "threshold moved");
        }
    }
}

#[test]
fn real_memo_is_recomputed_when_only_the_active_span_changes() {
    // Root 1 shows depths 7..=11 and child 2 (resident 10, wants 12) is held under its finest
    // active depth 11 until the memo of the real cut is reused. Then root's level 11 goes away:
    // the finest active depth falls to 10 with the pose, the cut and every level of the child
    // unchanged. The next frame must read the new span and release the child; the memo must not
    // keep the decision taken under the old one.
    let mut rig = hold_rig();
    let zoom = camera("zoom");
    let root = NodeId::new(1);
    let child = NodeId::new(2);
    let mut plan = d5_step_at(&mut rig, &ids(&[1]), &zoom, 2.0);
    for _ in 0..8 {
        plan = d5_step_at(&mut rig, &ids(&[1]), &zoom, 2.0);
    }
    let finest = rig.pool.applied_finest[&root];
    assert!(active_depths(&plan, &rig, 1).len() > 1, "no wide range");
    seed_more(&mut rig, 2, &[finest - 1]);
    let cut = ids(&CHILDREN);
    let mut reused = false;
    for frame in 0..10 {
        let plan = d5_step_at(&mut rig, &cut, &zoom, 0.125);
        reused |= plan.counters.real_reused;
        assert!(rig.pool.hold.contains(&child), "frame {frame}: held");
    }
    // Guards: the decision was memoised, and it depends on the span (finest - 1 is below it).
    assert!(reused, "the real cut was never reused");
    assert_eq!(rig.pool.applied_finest[&root], finest);
    let gone = rig.pool.key(root, finest);
    assert!(rig.pool.levels.remove(&gone).is_some());
    rig.pool.residency_epoch += 1;
    // The frame that applies the smaller span still judges against what was on screen; the
    // next one must judge against the new span.
    let first = d5_step_at(&mut rig, &cut, &zoom, 0.125);
    assert!(!first.counters.real_reused, "stale real cut");
    assert!(rig.pool.applied_finest[&root] < finest);
    let plan = d5_step_at(&mut rig, &cut, &zoom, 0.125);
    assert!(!rig.pool.hold.contains(&child), "still held");
    let entry = plan
        .entries
        .iter()
        .find(|e| e.node == child)
        .expect("child");
    assert_eq!(entry.drawn[0], Some(finest - 1));
}

#[test]
fn requests_rank_first_step_then_coarse_to_fine_nearest_first() {
    // Eight octant nodes store depths {4, 12} (both resident) and can build the ones between; in
    // the zoom camera the near ones want a range of depths. Every depth of the node ranges is
    // requested from one source, so apart from the step depths the requests rank by depth, coarse
    // to fine, then nearest node first (and by id on an exact tie).
    let mut nodes = octant_nodes();
    for node in nodes.values_mut() {
        node.stored = vec![4, 12];
        node.buildable = (4, 12);
    }
    let mut rig = d5_rig(nodes);
    for id in CHILDREN {
        seed_resident(&mut rig.pool, id, &[4, 12]);
    }
    rig.pool.residency_epoch += 1;
    // Off the symmetry axis, so no two nodes of a pair are at the same distance.
    let mut zoom = camera("zoom");
    zoom.orbit(-0.03, 0.02);
    let cut = ids(&CHILDREN);
    let mut plan = d5_step_at(&mut rig, &cut, &zoom, 2.0);
    for _ in 0..3 {
        plan = d5_step_at(&mut rig, &cut, &zoom, 2.0);
    }
    let requests: Vec<(u64, u8)> = plan
        .level_requests
        .iter()
        .map(|key| (key.node.get(), key.depth))
        .collect();
    let near = by_distance(&zoom, &CHILDREN);
    let at = |id: u64| near.iter().position(|n| *n == id).expect("node");
    // The nearer node of each pair has the higher id, so the id tie-break cannot give the order.
    assert!(at(3) < at(2) && at(9) < at(8), "{near:?}");
    // Guard: a node asks for several depths, so the order within a node is exercised.
    assert!(
        requests.iter().filter(|r| r.0 == 9).count() > 1,
        "{requests:?}"
    );
    assert!(requests.is_sorted_by_key(|r| r.1), "{requests:?}");
    // Within a depth the nearer node of a pair goes first (here the ranking is by the
    // distance to the node box, which `by_distance` only approximates for the other nodes).
    let first = |node: u64, depth: u8| requests.iter().position(|r| *r == (node, depth));
    // Both nodes of a pair ask for these depths (the restored rule: up to the spacing depth,
    // plus what an octant wants).
    for (nearer, farther, depths) in [(9, 8, 8..=11), (3, 2, 7..=8)] {
        for depth in depths {
            let a = first(nearer, depth).expect("nearer requested");
            let b = first(farther, depth).expect("farther requested");
            assert!(a < b, "depth {depth}: {requests:?}");
        }
    }
}
