//! R3-Q19: local fallback, coarse to fine and reset.

use super::*;

#[test]
fn a_region_that_waits_for_a_level_does_not_block_an_unrelated_one() {
    let nodes = two_regions();
    let mut rig = Rig::new(nodes.clone(), 256, 64, 1);
    let cam = camera("fit");
    let before = rig.run(10, &ids(&[2, 3]), &cam, 64.0);
    assert_eq!(before.apply, CutApply::Applied);
    // Both regions refine; node 10 of region A never arrives while the rest does.
    let cut: Vec<NodeId> = region_children(10)
        .into_iter()
        .chain(region_children(20))
        .map(NodeId::new)
        .collect();
    rig.held.insert(NodeId::new(10));
    // B draws one frame after the frame that uploads its last page: the cut of that frame was
    // computed before, so `ready` (residency at the start of the frame) is what must apply.
    let (mut resident_after, mut first_drawn) = (None, None);
    for frame in 0..20 {
        let ready = region_children(20)
            .into_iter()
            .all(|c| coarse_resident(&rig, c));
        let plan = rig.step(&cut, &cam, 64.0);
        assert_no_holes(&plan, &nodes);
        assert!(!coarse_resident(&rig, 10));
        let drawn = region_children(20)
            .into_iter()
            .all(|c| plan.active.iter().any(|a| a.key.node == NodeId::new(c)));
        if resident_after.is_none()
            && region_children(20)
                .into_iter()
                .all(|c| coarse_resident(&rig, c))
        {
            resident_after = Some(frame);
        }
        if ready {
            // Same frame: region B draws its new levels although node 10 has none.
            assert_eq!(plan.apply, CutApply::Applied, "frame {frame}");
            assert!(drawn, "frame {frame}: region B is not drawn");
        }
        if drawn && first_drawn.is_none() {
            first_drawn = Some(frame);
        }
    }
    assert!(resident_after.is_some());
    assert_eq!(first_drawn, resident_after.map(|frame| frame + 1));
    let plan = rig.step(&cut, &cam, 64.0);
    assert_eq!(plan.apply, CutApply::Applied);
    // Region A keeps its ancestor, region B has none left, and node 10 is counted as a stage.
    assert!(has_entry(&plan, 2) && !has_entry(&plan, 3));
    assert_eq!(plan.entries.len(), cut.len() + 1);
    assert!(plan.counters.stages >= 8, "{}", plan.counters.stages);
    assert_eq!(plan.counters.nodes_without_level, 1);
}

#[test]
fn the_fallback_ancestor_leaves_on_the_first_frame_after_the_node_is_resident() {
    let nodes = two_regions();
    let mut rig = Rig::new(nodes.clone(), 256, 64, 1);
    let cam = camera("fit");
    rig.run(10, &ids(&[2, 3]), &cam, 64.0);
    let cut: Vec<NodeId> = region_children(10).into_iter().map(NodeId::new).collect();
    rig.held.insert(NodeId::new(10));
    rig.run(8, &cut, &cam, 64.0);
    rig.held.clear();
    let (mut with, mut without) = (0, 0);
    for frame in 0..12 {
        let resident = coarse_resident(&rig, 10);
        let plan = rig.step(&cut, &cam, 64.0);
        assert_no_holes(&plan, &nodes);
        assert_eq!(plan.apply, CutApply::Applied, "frame {frame}");
        assert_eq!(has_entry(&plan, 2), !resident, "frame {frame}");
        if resident {
            without += 1;
            assert_eq!(plan.entries.len(), cut.len());
            assert_eq!(plan.counters.nodes_without_level, 0);
        } else {
            with += 1;
        }
    }
    assert!(with > 0 && without > 0, "{with} {without}");
}

#[test]
fn a_missing_node_without_an_ancestor_in_the_applied_cut_is_left_out_until_its_level_arrives() {
    let mut rig = Rig::new(octant_nodes(), 64, 8, 3);
    let cam = camera("fit");
    let held = rig.run(10, &ids(&[2, 3]), &cam, 64.0);
    assert_eq!(held.apply, CutApply::Applied);
    // Node 4 is not below any node of the applied cut: it waits outside the cut (R3-Q20) while
    // node 2 keeps drawing; node 3 left the cut and is not a loss.
    let mut waited = 0;
    let mut plan = rig.step(&ids(&[2, 4]), &cam, 64.0);
    while !has_entry(&plan, 4) {
        assert_eq!(plan.apply, CutApply::Applied);
        assert_eq!(plan.entries.len(), 1);
        assert_eq!(plan.entries[0], held.entries[0]);
        assert_eq!(plan.counters.nodes_without_level, 1);
        assert_eq!(plan.counters.lost_nodes, 0);
        waited += 1;
        plan = rig.step(&ids(&[2, 4]), &cam, 64.0);
        assert!(waited < 12);
    }
    assert!(waited > 0);
    assert_eq!(plan.apply, CutApply::Applied);
    assert_eq!(plan.entries.len(), 2);
    assert_eq!(plan.counters.nodes_without_level, 0);
}

#[test]
fn the_fallback_reaches_max_mask_levels_up_but_not_further() {
    let nodes = chain_nodes();
    let mut rig = Rig::new(nodes.clone(), 64, 8, 3);
    let cam = camera("fit");
    let held = rig.run(10, &ids(&[1]), &cam, 64.0);
    assert_eq!(held.apply, CutApply::Applied);
    // Node 20 is `MAX_MASK_LEVELS + 1` levels below the root: the root cannot hide it, so the cut
    // waits.
    let mut retained = 0;
    let mut plan = rig.step(&ids(&[20]), &cam, 64.0);
    while plan.apply == CutApply::RetainedPrevious {
        retained += 1;
        assert_eq!(plan.entries, held.entries);
        plan = rig.step(&ids(&[20]), &cam, 64.0);
        assert!(retained < 12);
    }
    assert!(retained > 0);
    assert!(!has_entry(&plan, 1));
    // Back to the root, then to node 21, `MAX_MASK_LEVELS` below it: the root draws until 21
    // arrives.
    rig.run(10, &ids(&[1]), &cam, 64.0);
    let mut seen_fallback = false;
    let mut seen_alone = false;
    for frame in 0..12 {
        let resident = coarse_resident(&rig, 21);
        let plan = rig.step(&ids(&[21]), &cam, 64.0);
        assert_eq!(plan.apply, CutApply::Applied, "frame {frame}");
        assert_no_holes(&plan, &nodes);
        assert_eq!(has_entry(&plan, 1), !resident, "frame {frame}");
        if resident {
            seen_alone = true;
        } else {
            seen_fallback = true;
            assert!(plan.counters.stages > 0);
        }
    }
    assert!(seen_fallback && seen_alone);
}

#[test]
fn the_fallback_never_makes_the_real_cut_too_fine() {
    // Applied {root}; the cut is {Y, Z}: Y is a node one level below the root that never arrives
    // and Z is resident `MAX_MASK_LEVELS + 1` levels below it. The root would cover Y, but with Z
    // in the cut it cannot hide Z: the real cut would fail with `DescendantTooFine`.
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
    // Z becomes resident first (alone), then the root is the applied cut.
    rig.run(8, &ids(&[20]), &cam, 64.0);
    let held = rig.run(10, &ids(&[1]), &cam, 64.0);
    assert_eq!(held.apply, CutApply::Applied);
    assert!(
        rig.pool.is_resident(rig.pool.key(NodeId::new(20), 7)),
        "setup"
    );
    rig.held.insert(NodeId::new(22));
    for frame in 0..10 {
        let plan = rig.step(&ids(&[22, 20]), &cam, 64.0);
        assert!(!plan.counters.descendant_too_fine, "frame {frame}");
        // No valid ancestor: the previous cut stays (a nearer valid ancestor would also do).
        assert_eq!(plan.apply, CutApply::RetainedPrevious, "frame {frame}");
        assert_eq!(plan.entries, held.entries, "frame {frame}");
    }
}

#[test]
fn a_resident_level_beyond_the_fit_cap_is_not_a_fallback() {
    // The root's only resident level is depth 4, but the fit capped it at 3 (as a previous frame's
    // fit could have): it does not draw, so it cannot cover node 21 while that waits.
    let nodes = chain_nodes();
    let mut rig = Rig::new(nodes.clone(), 64, 8, 1);
    let cam = camera("fit");
    let held = rig.run(12, &ids(&[1]), &cam, 1.0);
    assert_eq!(held.apply, CutApply::Applied);
    let (coarse, fine) = (
        rig.pool.key(NodeId::new(1), 3),
        rig.pool.key(NodeId::new(1), 4),
    );
    assert!(rig.pool.is_resident(fine), "setup: the root draws depth 4");
    rig.pool.levels.remove(&coarse);
    rig.pool.residency_epoch += 1;
    rig.pool.caps.insert(NodeId::new(1), 3);
    rig.held.insert(NodeId::new(21));
    let plan = rig.step(&ids(&[21]), &cam, 1.0);
    assert_eq!(plan.apply, CutApply::RetainedPrevious);
    assert_eq!(plan.entries, held.entries);
    assert!(!has_entry(&plan, 21));
}

#[test]
fn a_cap_on_a_dropped_depth_does_not_panic_after_a_recolour() {
    // A fit cap at depth 5 whose level a recolour dropped: the node now stores 6 and 7 only.
    let mut nodes = octant_nodes();
    let node = nodes.get_mut(&NodeId::new(2)).unwrap();
    node.stored = vec![6, 7];
    node.buildable = (6, 7);
    let mut rig = Rig::new(nodes, 128, 8, 0);
    let id = NodeId::new(2);
    rig.pool.caps.insert(id, 5);
    let cam = camera("zoom");
    let plan = rig.run(6, &ids(&[2]), &cam, 1.0);
    assert!(
        plan.entries[0]
            .drawn
            .iter()
            .flatten()
            .all(|d| [6, 7].contains(d))
    );
    // The driver reconciles the cap with what is stored: below every level it clamps to the
    // coarsest one, so no finer level comes back in.
    rig.pool.caps.insert(id, 5);
    rig.pool.reconcile_cap(id, &[6, 7]);
    assert_eq!(rig.pool.caps.get(&id), Some(&6));
    assert_eq!(capped(&[6, 7], rig.pool.caps.get(&id)), vec![6]);
    rig.pool.caps.insert(id, 7);
    rig.pool.reconcile_cap(id, &[4, 6]);
    assert_eq!(
        rig.pool.caps.get(&id),
        Some(&6),
        "clamped to the finest stored below it"
    );
    assert_eq!(capped(&[6, 7], Some(&5)), vec![6], "never empty");
}

#[test]
fn a_far_node_that_draws_nothing_is_not_starved_by_refinements_elsewhere() {
    // Node 8 draws its depth 4 and wants depth 5; node 2 (farther) draws nothing and stores
    // depth 6 and deeper. By absolute depth alone the refinement of node 8 would go first.
    let mut nodes = octant_nodes();
    nodes.get_mut(&NodeId::new(8)).unwrap().stored = vec![4, 5];
    nodes.get_mut(&NodeId::new(8)).unwrap().buildable = (4, 5);
    nodes.get_mut(&NodeId::new(2)).unwrap().stored = vec![6, 7];
    nodes.get_mut(&NodeId::new(2)).unwrap().buildable = (6, 7);
    let mut rig = Rig::new(nodes, 128, 1, 0);
    let cam = camera("zoom");
    assert_eq!(by_distance(&cam, &[2, 8]), vec![8, 2], "setup");
    rig.run(8, &ids(&[8]), &camera("fit"), 64.0);
    assert!(
        rig.pool.is_resident(rig.pool.key(NodeId::new(8), 4)),
        "setup"
    );
    let cut = ids(&[8, 2]);
    // One page per frame: the first frame with data uploads the first level of node 2.
    let requests = rig.step(&cut, &cam, 4.0).fetch_requests;
    let order: Vec<(u64, u8)> = requests.iter().map(|k| (k.node.get(), k.depth)).collect();
    assert!(
        order.contains(&(8, 5)) && order.contains(&(2, 6)),
        "{order:?}"
    );
    assert_eq!(order.first(), Some(&(2, 6)), "{order:?}");
    let uploads = rig.step(&cut, &cam, 4.0).uploads;
    let first = uploads.first().expect("an upload");
    assert_eq!(
        (first.key.node.get(), first.key.depth),
        (2, 6),
        "{uploads:?}"
    );
}

#[test]
fn requests_and_uploads_go_coarse_to_fine() {
    // Node 8 is the nearest and wants depth 7; node 2, farther, stores up to depth 5. By distance
    // alone the depth-7 level of node 8 would come first.
    let mut nodes = octant_nodes();
    nodes.get_mut(&NodeId::new(2)).unwrap().stored = vec![4, 5];
    nodes.get_mut(&NodeId::new(2)).unwrap().buildable = (4, 5);
    let mut rig = Rig::new(nodes, 128, 64, 0);
    let cut = ids(&[8, 2]);
    let cam = camera("zoom");
    assert_eq!(by_distance(&cam, &[2, 8]), vec![8, 2], "setup");
    // Far away both nodes want only their coarsest level.
    rig.run(4, &cut, &camera("fit"), 64.0);
    let requests = rig.step(&cut, &cam, 4.0).fetch_requests;
    let order: Vec<(u64, u8)> = requests.iter().map(|k| (k.node.get(), k.depth)).collect();
    assert!(
        order.contains(&(2, 5)) && order.contains(&(8, 7)),
        "{order:?}"
    );
    assert!(
        requests.windows(2).all(|w| w[0].depth <= w[1].depth),
        "{order:?}"
    );
    let started: Vec<u8> = rig
        .step(&cut, &cam, 4.0)
        .uploads
        .iter()
        .filter(|u| u.page_range.start == 0)
        .map(|u| u.key.depth)
        .collect();
    assert!(
        started.len() >= 2 && started.windows(2).all(|w| w[0] <= w[1]),
        "{started:?}"
    );

    // Nodes that draw nothing: the first level of a parent is requested before the first level of
    // its child, whatever the order of the cut.
    let mut rig = Rig::new(two_regions(), 128, 64, 0);
    let requests = rig.step(&ids(&[10, 2]), &cam, 4.0).fetch_requests;
    let at = |node: u64| requests.iter().position(|k| k.node.get() == node);
    assert!(
        at(2)
            .zip(at(10))
            .is_some_and(|(parent, child)| parent < child),
        "{requests:?}"
    );
}

#[test]
fn reset_evicts_every_level_and_the_pool_converges_back() {
    // `reset` assumes a fresh GPU (device lost, or the renderer dropped its pool), so the fake GPU
    // starts empty: there is no "same GPU" case to model.
    let nodes = two_regions();
    let mut rig = Rig::new(nodes.clone(), 256, 4, 2);
    let cam = camera("fit");
    let cut = ids(&[2, 3]);
    let fine = {
        let mut cut = region_children(10);
        cut.extend(region_children(20));
        ids(&cut)
    };
    rig.run(10, &cut, &cam, 64.0);
    let before = rig.run(60, &fine, &cam, 4.0);
    let resident = |rig: &Rig| -> Vec<LevelKey> {
        let mut keys: Vec<LevelKey> = rig
            .pool
            .levels
            .iter()
            .filter(|(_, l)| l.is_resident())
            .map(|(k, _)| *k)
            .collect();
        keys.sort();
        keys
    };
    let keys = resident(&rig);
    let parent_pages: u32 = keys
        .iter()
        .filter(|k| k.node.get() <= 3)
        .map(|k| rig.pool.level(*k).unwrap().pages.len() as u32)
        .sum();
    assert!(keys.len() > 16, "setup: {} levels", keys.len());

    let mut events = rig.pool.reset();
    events.sort_by_key(|event| match event {
        CellPoolEvent::Evict(key) | CellPoolEvent::Cancel(key) => *key,
    });
    let expected: Vec<CellPoolEvent> = keys.iter().map(|k| CellPoolEvent::Evict(*k)).collect();
    assert_eq!(events, expected);
    assert_eq!(rig.pool.resident_pages(), 0);
    // Every page is free at once: nothing is delayed.
    assert_eq!(rig.pool.free.len(), 256);
    assert!(rig.pool.pending.is_empty());
    rig.inflight.clear();
    rig.gpu = FakeGpu::default();

    let mut empty_frames = 0;
    let mut mask_slots = HashSet::new();
    let mut last = None;
    for _ in 0..80 {
        let plan = rig.step(&fine, &cam, 4.0);
        // Nothing is drawn before the cut is complete again, and never with a hole.
        assert_no_holes(&plan, &nodes);
        assert!(plan.page_frees.is_empty(), "the old GPU pages are gone");
        mask_slots.extend(plan.mask_updates.iter().map(|update| update.slot));
        if plan.entries.is_empty() {
            empty_frames += 1;
        }
        last = Some(plan);
    }
    assert!(empty_frames > 0);
    // A new renderer hands mask slots out from 0, with no gap.
    assert_eq!(
        mask_slots,
        (0..mask_slots.len() as u32).collect::<HashSet<u32>>()
    );
    let after = last.unwrap();
    assert_eq!(after.apply, CutApply::Applied);
    assert_eq!(after.entries, before.entries);
    // The coarse levels of the parents were not pinned by the cut any more.
    let children: Vec<LevelKey> = keys.iter().filter(|k| k.node.get() > 3).copied().collect();
    assert_eq!(resident(&rig), children);
    for (name, a, b) in [
        ("stages", after.counters.stages, before.counters.stages),
        (
            "nodes_without_level",
            after.counters.nodes_without_level,
            before.counters.nodes_without_level,
        ),
        (
            "pool_coarsened",
            after.counters.pool_coarsened,
            before.counters.pool_coarsened,
        ),
        (
            "pool_overflow",
            after.counters.pool_overflow,
            before.counters.pool_overflow,
        ),
        (
            "resident_pages",
            after.counters.resident_pages,
            before.counters.resident_pages - parent_pages,
        ),
    ] {
        assert_eq!(a, b, "{name}");
    }
    rig.assert_gpu_matches_pool();
}

#[test]
fn reset_cancels_requests_and_evicts_uploading_levels() {
    let mut rig = Rig::new(octant_nodes(), 128, 1, 4);
    let cam = camera("zoom");
    let cut = ids(&CHILDREN);
    rig.run(14, &cut, &cam, 4.0);
    let (mut evict, mut cancel) = (Vec::new(), Vec::new());
    for (key, level) in &rig.pool.levels {
        match level.state {
            LevelState::Requested => cancel.push(*key),
            _ => evict.push(*key),
        }
    }
    assert!(!evict.is_empty() && !cancel.is_empty(), "setup");
    assert!(!rig.gpu.partial().is_empty(), "setup: an upload in flight");
    let events = rig.pool.reset();
    let evicted: HashSet<LevelKey> = events
        .iter()
        .filter_map(|e| match e {
            CellPoolEvent::Evict(k) => Some(*k),
            CellPoolEvent::Cancel(_) => None,
        })
        .collect();
    let cancelled: HashSet<LevelKey> = events
        .iter()
        .filter_map(|e| match e {
            CellPoolEvent::Cancel(k) => Some(*k),
            CellPoolEvent::Evict(_) => None,
        })
        .collect();
    assert_eq!(evicted, evict.into_iter().collect());
    assert_eq!(cancelled, cancel.into_iter().collect());
    assert!(rig.pool.levels.is_empty());
    // The GPU is fresh, so the slots of the half-uploaded levels are free at once, with no
    // `page_frees`, and the pool keeps all its pages.
    assert_eq!(rig.pool.free.len(), 128);
    assert!(rig.pool.pending.is_empty());
    rig.inflight.clear();
    rig.gpu = FakeGpu::default();
    for _ in 0..4 {
        assert!(rig.step(&cut, &cam, 4.0).page_frees.is_empty());
    }
    rig.assert_gpu_matches_pool();
    assert_eq!(
        rig.pool.free.len() + rig.pool.pending_pages() + rig.gpu.owner.len(),
        128
    );
}
