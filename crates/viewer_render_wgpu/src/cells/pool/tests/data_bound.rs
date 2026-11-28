//! ADR 0055 plan, tasks 5 and 7a: the data bound of the node range and the wish capped at it;
//! also the R3-Q21 gate under a fit cap and plan task 10b.

use super::*;

// ---- Data bound of the node range (ADR 0055 plan, task 5; stored, not used) -----------------

/// Replaces the pages of every resident level of `id` by one cell containing the local point
/// `at` (the page count is kept: the upload bookkeeping does not change).
fn put_pages_at(rig: &mut Rig, id: NodeId, at: [f64; 3]) {
    for (key, level) in &mut rig.pool.levels {
        let Some(data) = level.data.as_mut().filter(|_| key.node == id) else {
            continue;
        };
        let side = SIDE / 2.0_f64.powi(i32::from(key.depth));
        let mut packed = (**data).clone();
        for page in &mut packed.pages {
            page.origin = at.map(|v| (v / side) as u64);
            page.box_max = [0; 3];
        }
        *data = Arc::new(packed);
    }
}

/// Local points of the zoom camera (eye at (32, 71.9, 71.9) looking at the centre): one cell
/// right in front of the eye, at view depth ~12, and one at the far side, at view depth ~102.
/// Both lie in node 9 and node 3 of `octant_nodes`.
const NEAR_POINT: [f64; 3] = [33.0, 63.5, 63.5];
const FAR_POINT: [f64; 3] = [33.0, 0.2, 0.2];
const NEAR_NODE: u64 = 9;
const FAR_NODE: u64 = 3;
/// Pool target: depths 4 to 7 change with the occupied depth (6 at ~12, 5 at ~102).
const BOUND_TARGET: f64 = 32.0;

/// `NodeDepths::hi_d` of `id` as the pure functions give it at `view`: the least over the
/// resident levels of `max(bound of the level, level depth)`, not below `lo`.
fn bound_from_pages(rig: &Rig, id: NodeId, view: &ViewProjection) -> Option<u8> {
    let node = &rig.nodes[&id];
    let (probes, _) = masked_probes(
        view,
        &grid(),
        ORIGIN,
        node.bounds,
        &std::array::from_fn(|o| node.probe_box(o)),
    );
    let region = probe_region(node.bounds, &probes);
    let lo = rig.pool.depths[&id].far.0;
    (node.buildable.0..=node.buildable.1)
        .filter_map(|depth| {
            let level = rig.pool.level(rig.pool.key(id, depth))?;
            let occupied =
                crate::nearest_occupied_depth(view, &grid(), ORIGIN, depth, &level.pages)?;
            let (_, hi) = crate::cells::cut::node_view_range_at(
                view,
                &grid(),
                ORIGIN,
                region,
                node.buildable,
                BOUND_TARGET / 2.0,
                rig.pool.min_side(),
                Some(occupied),
            )?;
            Some(hi.max(depth))
        })
        .min()
        .map(|bound| bound.max(lo))
}

/// A settled rig at the zoom camera whose node `id` holds one cell at `at` in every level.
fn bound_rig(id: u64, at: [f64; 3]) -> Rig {
    let mut rig = Rig::new(octant_nodes(), 256, 64, 0);
    rig.run(40, &ids(&CHILDREN), &camera("zoom"), BOUND_TARGET);
    put_pages_at(&mut rig, NodeId::new(id), at);
    // A new entry is computed from the pages as they are now.
    rig.pool.depths.remove(&NodeId::new(id));
    rig.step(&ids(&CHILDREN), &camera("zoom"), BOUND_TARGET);
    rig
}

#[test]
fn the_data_bound_is_the_least_over_the_resident_levels_of_hi_at_their_nearest_page() {
    let (near, far) = (NodeId::new(NEAR_NODE), NodeId::new(FAR_NODE));
    let zoom = view(&camera("zoom"));
    let rig = bound_rig(NEAR_NODE, NEAR_POINT);
    let entry = &rig.pool.depths[&near];
    let bound = rig.pool.data_bound(near).expect("a page in view");
    assert_eq!(Some(bound), bound_from_pages(&rig, near, &zoom));
    assert!(entry.far.0 <= bound && bound <= rig.nodes[&near].buildable.1);
    let far_rig = bound_rig(FAR_NODE, FAR_POINT);
    let far_bound = far_rig.pool.data_bound(far).expect("a page in view");
    assert_eq!(Some(far_bound), bound_from_pages(&far_rig, far, &zoom));
    // The nearer the occupied cell, the finer the bound.
    assert!(far_bound < bound, "{far_bound} against {bound}");
}

#[test]
fn the_data_bound_does_not_depend_on_the_levels_over_it() {
    let (cut, zoom) = (ids(&CHILDREN), camera("zoom"));
    let id = NodeId::new(NEAR_NODE);
    let mut rig = bound_rig(NEAR_NODE, FAR_POINT);
    let bound = rig.pool.data_bound(id).expect("bound");
    let top = rig.nodes[&id].buildable.1;
    assert!(bound < top, "setup: {bound} against {top}");
    // The levels over the bound leave: it does not change (they could not hold the least).
    let evaluations = rig.pool.bound_evaluations;
    for depth in (bound + 1)..=top {
        let (key, frame) = (rig.pool.key(id, depth), rig.pool.frame);
        rig.pool.evict_resident(key, frame);
    }
    rig.pool.uncapped_wish = false;
    rig.step(&cut, &zoom, BOUND_TARGET);
    assert!(
        rig.pool.bound_evaluations > evaluations,
        "setup: looked at again"
    );
    assert_eq!(rig.pool.data_bound(id), Some(bound));
    // The whole resident set leaves and comes back in the pool's own order, with the same pages:
    // the same bound, wherever the order of arrival went.
    let keys: Vec<LevelKey> = (rig.nodes[&id].buildable.0..=bound)
        .map(|depth| rig.pool.key(id, depth))
        .collect();
    for key in &keys {
        let frame = rig.pool.frame;
        rig.pool.evict_resident(*key, frame);
    }
    rig.step(&cut, &zoom, BOUND_TARGET);
    rig.run(8, &cut, &zoom, BOUND_TARGET);
    put_pages_at(&mut rig, id, FAR_POINT);
    rig.pool.residency_epoch += 1;
    rig.step(&cut, &zoom, BOUND_TARGET);
    assert_eq!(rig.pool.data_bound(id), Some(bound));
}

#[test]
fn a_level_arriving_inside_the_bound_can_lower_it_and_the_wishes_follow() {
    let (cut, zoom) = (ids(&CHILDREN), camera("zoom"));
    let id = NodeId::new(NEAR_NODE);
    let mut rig = bound_rig(NEAR_NODE, NEAR_POINT);
    rig.pool.uncapped_wish = false;
    let before = rig.pool.data_bound(id).expect("bound");
    let coarse = rig.nodes[&id].buildable.0;
    assert!(coarse < before, "setup: {coarse} against {before}");
    // The coarsest level now has its data far from the eye: it alone gives a lower bound.
    let key = rig.pool.key(id, coarse);
    let level = rig.pool.levels.get_mut(&key).expect("level");
    let mut packed = (**level.data.as_ref().expect("data")).clone();
    for page in &mut packed.pages {
        page.origin = FAR_POINT.map(|v| (v / (SIDE / 2.0_f64.powi(i32::from(coarse)))) as u64);
    }
    level.data = Some(Arc::new(packed));
    rig.pool.residency_epoch += 1;
    let mut plan = rig.step(&cut, &zoom, BOUND_TARGET);
    let after = rig.pool.data_bound(id).expect("bound");
    assert!(after < before, "setup: {after} against {before}");
    assert_eq!(
        after,
        bound_from_pages(&rig, id, &view(&zoom)).expect("pages")
    );
    assert!(
        !plan.counters.wanted_reused,
        "the lowered bound redoes the wanted cut"
    );
    let mut wish = None;
    for _ in 0..4 {
        wish = wish.max(finest_wish_and_request(&plan, id).0);
        plan = rig.step(&cut, &zoom, BOUND_TARGET);
    }
    assert_eq!(wish, Some(after), "the wish did not follow the bound");
}

#[test]
fn a_resident_level_with_no_page_in_view_bounds_the_node_at_its_own_depth() {
    let (cut, zoom) = (ids(&CHILDREN), camera("zoom"));
    let id = NodeId::new(NEAR_NODE);
    let (first, last) = (4_u8, 7_u8);
    // Run with the levels `resident` only, the ones in `unseen` holding their pages far outside
    // the view; return the bound and the finest wish of the node.
    let run = |resident: &[u8], unseen: &[u8]| {
        let mut rig = bound_rig(NEAR_NODE, FAR_POINT);
        rig.pool.uncapped_wish = false;
        for depth in first..=last {
            let (key, frame) = (rig.pool.key(id, depth), rig.pool.frame);
            if !resident.contains(&depth) {
                rig.pool.evict_resident(key, frame);
            } else if unseen.contains(&depth) {
                let side = SIDE / 2.0_f64.powi(i32::from(depth));
                let level = rig.pool.levels.get_mut(&key).expect("level");
                let mut packed = (**level.data.as_ref().expect("data")).clone();
                for page in &mut packed.pages {
                    page.origin = [1.0e9; 3].map(|v| (v / side) as u64);
                }
                level.data = Some(Arc::new(packed));
            }
        }
        rig.pool.residency_epoch += 1;
        let plan = rig.step(&cut, &zoom, BOUND_TARGET);
        let lo = rig.pool.depths[&id].far.0;
        (
            rig.pool.data_bound(id).expect("a resident level bounds"),
            lo,
            finest_wish_and_request(&plan, id).0,
        )
    };
    // {4, 5} with only level 4 in view, against {5, 6} with none: the coarsest level with no box
    // in view gives the bound whichever coarser level is also resident.
    let (with_view, lo, wish) = run(&[4, 5], &[5]);
    let (without_view, _, _) = run(&[5, 6], &[5, 6]);
    assert_eq!(
        with_view, without_view,
        "the bound follows the resident set"
    );
    assert_eq!(with_view, lo.max(5));
    assert!(
        wish.is_some() && wish <= Some(with_view),
        "wish {wish:?}, bound {with_view}"
    );
}

#[test]
fn a_still_camera_at_rest_computes_no_data_bound_and_a_moved_one_does() {
    let (cut, zoom) = (ids(&CHILDREN), camera("zoom"));
    let mut rig = bound_rig(NEAR_NODE, NEAR_POINT);
    rig.run(6, &cut, &zoom, BOUND_TARGET);
    let settled = rig.pool.bound_evaluations;
    assert!(settled > 0, "the bound was computed while settling");
    rig.run(8, &cut, &zoom, BOUND_TARGET);
    assert_eq!(rig.pool.bound_evaluations, settled, "no work at rest");
    rig.step(&cut, &camera("fit"), BOUND_TARGET);
    assert!(
        rig.pool.bound_evaluations > settled,
        "a pose change computes it again"
    );
}

#[test]
fn the_cached_depths_look_at_the_data_bound_again_when_a_resident_level_changes() {
    let (cut, zoom) = (ids(&CHILDREN), camera("zoom"));
    let id = NodeId::new(NEAR_NODE);
    let mut rig = bound_rig(NEAR_NODE, NEAR_POINT);
    let refreshes = |rig: &mut Rig| rig.step(&cut, &zoom, BOUND_TARGET).counters.depth_refreshes;
    // Nothing changes at a fixed pose: no refresh, no work.
    rig.run(4, &cut, &zoom, BOUND_TARGET);
    let evaluations = rig.pool.bound_evaluations;
    assert_eq!(refreshes(&mut rig), 0);
    assert_eq!(rig.pool.bound_evaluations, evaluations);
    // A level that leaves the pool, whichever it is, makes the bound be looked at again without
    // redoing the entry.
    let key = rig.pool.key(id, rig.nodes[&id].buildable.0);
    let frame = rig.pool.frame;
    rig.pool.evict_resident(key, frame);
    assert_eq!(refreshes(&mut rig), 0);
    assert!(rig.pool.bound_evaluations > evaluations);
}

#[test]
fn the_active_set_has_no_gap_between_a_drawn_depth_and_the_range() {
    // (range, drawn, resident, expected active depths).
    type Case = ((u8, u8), u8, &'static [u8], &'static [u8]);
    let cases: [Case; 4] = [
        // Drawn coarser than the range: depth 11 is resident, usable and inside the range.
        ((12, 12), 10, &[2, 7, 10, 11, 12], &[10, 11, 12]),
        // Drawn finer than the range.
        ((8, 9), 11, &[2, 8, 9, 10, 11], &[8, 9, 10, 11]),
        // A residency gap between the drawn depth and the range: the run stops at it.
        ((12, 12), 10, &[2, 10, 12], &[10]),
        // A drawn depth far from the range end (a hop of several levels): the run reaches it.
        (
            (12, 12),
            7,
            &[2, 7, 8, 9, 10, 11, 12],
            &[7, 8, 9, 10, 11, 12],
        ),
    ];
    for (n, (range, drawn, resident, expected)) in cases.into_iter().enumerate() {
        let mut nodes = octant_nodes();
        nodes.insert(
            NodeId::new(1),
            d5_node(SIDE, &(2..=12).collect::<Vec<u8>>()),
        );
        let mut rig = d5_rig(nodes);
        seed_resident(&mut rig.pool, 1, resident);
        let entry = CellCutEntry {
            node: NodeId::new(1),
            wanted: [Some(7); 8],
            drawn: [Some(drawn); 8],
            stage: [false; 8],
            step: [None; 8],
            range: Some(range),
            node_anchor: None,
            node_target: None,
            node_run: None,
            node_stage: false,
            node_step: None,
            mask: [0; 8],
        };
        let node_depth = CellPool::node_depth(&grid(), rig.nodes[&NodeId::new(1)].bounds);
        let active = rig.pool.cell_active_depths(&entry, node_depth, None);
        assert_eq!(active, expected, "case {n}");
    }
}

// ---- The wish is capped at the data bound (ADR 0055 plan, task 7a) --------------------------

/// The finest depth any octant of `id` wishes in `plan`, and the finest depth requested for it.
fn finest_wish_and_request(plan: &FramePlan, id: NodeId) -> (Option<u8>, Option<u8>) {
    let wish = plan
        .entries
        .iter()
        .filter(|entry| entry.node == id)
        .flat_map(|entry| entry.wanted.iter().flatten().copied())
        .max();
    let request = plan
        .fetch_requests
        .iter()
        .filter(|key| key.node == id)
        .map(|key| key.depth)
        .max();
    (wish, request)
}

#[test]
fn a_node_wishes_and_requests_no_depth_finer_than_its_data_bound() {
    let (cut, zoom) = (ids(&CHILDREN), camera("zoom"));
    let id = NodeId::new(NEAR_NODE);
    // The eye node (its octants cross the eye plane and wish the finest depth) holds data only
    // on the far side: its bound is 5 and it wishes 7.
    let mut rig = bound_rig(NEAR_NODE, FAR_POINT);
    rig.pool.uncapped_wish = false;
    let bound = rig.pool.data_bound(id).expect("a page in view");
    let top = rig.nodes[&id].buildable.1;
    assert!(
        bound < top,
        "setup: the bound {bound} must leave room below {top}"
    );
    // The levels finer than the bound go away; the pool must not fetch them again.
    let finer: Vec<LevelKey> = ((bound + 1)..=top).map(|d| rig.pool.key(id, d)).collect();
    for key in finer {
        let frame = rig.pool.frame;
        rig.pool.evict_resident(key, frame);
    }
    let (mut wish, mut request) = (None, None);
    for _ in 0..12 {
        let plan = rig.step(&cut, &zoom, BOUND_TARGET);
        let (w, r) = finest_wish_and_request(&plan, id);
        wish = wish.max(w);
        request = request.max(r);
    }
    assert!(
        wish.is_some() && wish <= Some(bound),
        "wish {wish:?}, bound {bound}"
    );
    assert!(request <= Some(bound), "request {request:?}, bound {bound}");
}

#[test]
fn at_a_fixed_pose_no_level_is_requested_twice_or_cancelled_more_than_twice() {
    let (cut, zoom) = (ids(&CHILDREN), camera("zoom"));
    let mut rig = Rig::new(octant_nodes(), 256, 64, 2);
    rig.pool.uncapped_wish = false;
    let (mut requested, mut cancelled): (HashMap<LevelKey, u32>, HashMap<LevelKey, u32>) =
        Default::default();
    for _ in 0..200 {
        let plan = rig.step(&cut, &zoom, BOUND_TARGET);
        for key in &plan.fetch_requests {
            *requested.entry(*key).or_default() += 1;
        }
        for event in &plan.events {
            if let CellPoolEvent::Cancel(key) = event {
                *cancelled.entry(*key).or_default() += 1;
            }
        }
    }
    assert!(!requested.is_empty(), "setup: levels were requested");
    let repeated: Vec<_> = requested.iter().filter(|(_, n)| **n > 1).collect();
    assert!(repeated.is_empty(), "requested twice: {repeated:?}");
    let cancels: Vec<_> = cancelled.iter().filter(|(_, n)| **n > 2).collect();
    assert!(cancels.is_empty(), "cancelled more than twice: {cancels:?}");
}

#[test]
fn run_covers_compares_the_run_end_with_the_finest_drawn_before_capped_by_the_wish() {
    // Zoom in: one ancestor run `[5, 6]`; the child takes over once its run reaches 6, not 7.
    let ancestor = [(5, 6)];
    assert!(!run_covers(Some((4, 5)), &ancestor, 8));
    assert!(run_covers(Some((4, 6)), &ancestor, 8));
    assert!(run_covers(Some((6, 6)), &ancestor, 8));
    assert!(run_covers(Some((4, 9)), &ancestor, 8));
    // The wish bounds the threshold, also when the ancestor drew finer than the child can build.
    assert!(run_covers(Some((4, 5)), &ancestor, 5));
    assert!(!run_covers(Some((4, 4)), &ancestor, 5));
    assert!(run_covers(Some((4, 5)), &[(5, 9)], 5));
    // Zoom out: the finest of the descendants counts, whichever it is.
    let descendants = [(3, 5), (6, 8), (4, 4)];
    assert!(!run_covers(Some((4, 7)), &descendants, 9));
    assert!(run_covers(Some((4, 8)), &descendants, 9));
    assert!(run_covers(Some((4, 7)), &descendants, 7));
    // Nothing drawn before, or nothing of the node to draw: not held by this rule.
    assert!(run_covers(Some((4, 4)), &[], 9));
    assert!(run_covers(None, &ancestor, 9));
}

/// Where the pool of the cap rows is small enough for the fit cap to stay (`fit` lifts the caps
/// when the uncapped cut fits under the low mark).
const CAP_POOL: u32 = 512;
const CAP_POINTS: u64 = 3_000_000;

/// What one frame of a fit-cap row saw.
#[derive(Debug)]
struct CapRow {
    /// The frame (from 0) on which the node came out of the hold, if it did.
    released: Option<usize>,
    /// The wish the oracle is run up to, shown in a failure.
    wish: u8,
    /// The runs `eval_hold` builds for the node (range `Wide` and `Near`).
    gate: [Option<Run>; 2],
    gate_releases: [bool; 2],
    emitted: Vec<u8>,
    cap_stayed: bool,
    oracle: Result<(), String>,
}

/// One zoom (`zoom_in`: child 10 over node 2; else node 2 over its children 10..=17) with a fit
/// cap on the entering node, `resident` levels of it (never delivered more), and, when `injected`,
/// the applied cut pretending the node already drew depth 7 (a level above the cap).
/// Each frame is replayed from scratch so the gate is read on the very state `plan` sees.
fn cap_row(
    zoom_in: bool,
    (cap, target, before_target, resident, injected): (u8, f64, f64, &[u8], bool),
    frames: usize,
) -> CapRow {
    let node = NodeId::new(if zoom_in { 10 } else { 2 });
    let cut = if zoom_in {
        let mut cut = ids(&region_children(10));
        cut.push(NodeId::new(3));
        cut
    } else {
        ids(&[2])
    };
    let setup = || {
        let (mut rig, cam) = if zoom_in {
            let rig = Rig::new(two_regions_with_coarse_children(), CAP_POOL, 3, 1);
            (rig, camera("fit"))
        } else {
            let rig = Rig::new(two_regions(), CAP_POOL, 3, 1);
            (rig, camera("fit"))
        };
        let last = if zoom_in {
            rig.run(60, &ids(&[2, 3]), &cam, before_target)
        } else {
            rig.run(60, &ids(&region_children(10)), &cam, before_target)
        };
        set_resident(&mut rig, node.get(), resident);
        for data in rig.nodes.values_mut() {
            data.points = CAP_POINTS;
        }
        rig.held.insert(node);
        rig.pool.caps.insert(node, cap);
        if injected {
            rig.pool.applied_drawn.insert(node, [Some(7); 8]);
        }
        (rig, cam, last)
    };
    let step = |rig: &mut Rig, cam: &OrbitCamera| d5_step_at(rig, &cut, cam, target / 2.0);
    // The wish of the node on this view, from a first frame of a throwaway rig.
    let wish = {
        let (mut rig, cam, _) = setup();
        let plan = step(&mut rig, &cam);
        plan.entries
            .iter()
            .find(|e| e.node == node)
            .and_then(|e| e.wanted.iter().flatten().max().copied())
            .unwrap_or(0)
    };
    let mut found = None;
    for frame in 0..frames {
        let (mut rig, cam, last) = setup();
        for _ in 0..frame {
            step(&mut rig, &cam);
        }
        let probes =
            [Far::Wide, Far::Near].map(|far| refine_probe(&mut rig, node.get(), wish, far));
        let owner_depth = probes[0].node_depth;
        let befores: Vec<Owner> = if zoom_in {
            vec![Owner {
                node_depth: owner_depth - 1,
                lo: [0.0, 0.0, 10.0],
                active: active_depths(&last, &rig, 2),
            }]
        } else {
            let top = Owner {
                node_depth: owner_depth,
                lo: [0.0, 0.0, 10.0],
                active: Vec::new(),
            };
            (10..18)
                .map(|child| Owner {
                    node_depth: owner_depth + 1,
                    lo: top.child_lo(child - 10),
                    active: active_depths(&last, &rig, child as u64),
                })
                .collect()
        };
        let plan = step(&mut rig, &cam);
        if depth_drawn(&plan, node).is_empty() {
            continue;
        }
        let emitted = active_depths(&plan, &rig, node.get());
        let lo = if zoom_in {
            befores[0].child_lo(0)
        } else {
            [0.0, 0.0, 10.0]
        };
        let after = Owner {
            node_depth: owner_depth,
            lo,
            active: emitted.clone(),
        };
        // Zoom in: the cells of the entering node; zoom out: those of each replaced descendant.
        let samples: Vec<_> = befores
            .iter()
            .flat_map(|before| {
                let at = if zoom_in { &after } else { before };
                (0..8).map(|o| (before.clone(), at.sub_centre(o)))
            })
            .collect();
        let gate = probes.each_ref().map(Probe::run);
        found = Some(CapRow {
            released: Some(frame),
            wish,
            gate_releases: gate.map(|run| {
                let before: Vec<Run> = befores
                    .iter()
                    .filter_map(|b| b.active.first().zip(b.active.last()).map(|(l, h)| (*l, *h)))
                    .collect();
                run_covers(run, &before, wish)
            }),
            gate,
            emitted,
            cap_stayed: rig.pool.caps.get(&node) == Some(&cap),
            oracle: twin_releases(&after, wish, &samples),
        });
        break;
    }
    found.unwrap_or(CapRow {
        released: None,
        wish,
        gate: [None; 2],
        gate_releases: [false; 2],
        emitted: Vec::new(),
        cap_stayed: true,
        oracle: Ok(()),
    })
}

/// R3-Q21 under a fit cap: when `eval_hold` releases a node whose depths are capped, the active set
/// `plan` really emits for it (`finer_drawn` may replace its resident levels with finer ones
/// above the cap) draws no sampled cell coarser than what drew there before, judged by the GPU
/// twin, and reaches at least the finest depth of the run the gate assumed.
#[test]
fn a_capped_node_the_gate_releases_draws_nothing_coarser_than_before_with_the_set_plan_emits() {
    let (mut rows, mut zoom_in_rows, mut zoom_out_rows, mut above_cap) = (0, 0, 0, 0);
    let mut failures = Vec::new();
    for zoom_in in [true, false] {
        // `(target now, target before)`: what the node wishes and what drew before.
        for (target, before_target) in [(4.0, 8.0), (4.0, 16.0)] {
            for cap in [4u8, 5, 6] {
                for resident in [&[4u8, 5, 6, 7][..], &[4, 7], &[5, 7], &[4, 5]] {
                    for injected in [false, true] {
                        let case = (cap, target, before_target, resident, injected);
                        let row = cap_row(zoom_in, case, 2);
                        let name = format!("zoom_in {zoom_in} {case:?} wish {}: {row:?}", row.wish);
                        // `fit` lifts a cap the uncapped cut fits under: not a capped row.
                        if !row.cap_stayed || row.released.is_none() {
                            continue;
                        }
                        rows += 1;
                        zoom_in_rows += usize::from(zoom_in);
                        zoom_out_rows += usize::from(!zoom_in);
                        above_cap += usize::from(row.emitted.last().is_some_and(|d| *d > cap));
                        let finest = row.emitted.last().copied();
                        let lower = row
                            .gate
                            .iter()
                            .flatten()
                            .any(|(_, hi)| finest.is_none_or(|f| f < *hi));
                        if row.oracle.is_err() || lower || row.gate_releases.contains(&false) {
                            failures.push(name);
                        }
                    }
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} rows:\n{}",
        failures.len(),
        failures.join("\n")
    );
    assert!(
        zoom_in_rows > 0 && zoom_out_rows > 0,
        "{zoom_in_rows} {zoom_out_rows}"
    );
    assert!(above_cap > 0, "no row drew a level above its cap");
    assert!(rows > 30, "{rows} rows");
}

/// Held with no escape (characterization). A wish that the node can never reach does not exist:
/// `cell_cut` clamps it to the node's buildable range (stored depths and fit cap), so a node
/// whose levels stop short of what the ancestor drew is released once it reaches its own finest
/// level (`min(what drew before, wish)`), by `eval_hold` and by the per-octant `eval_refine_hold`
/// alike. The only long hold is a level that is stored and buildable but not resident (never
/// delivered, e.g. refused): both judges hold it until it arrives.
#[test]
fn a_node_whose_wish_is_clamped_below_the_ancestor_is_released_and_a_missing_level_holds() {
    // `(stored, resident, ancestor finest, wish, held)` of child 10 in a zoom in.
    type Never = (&'static [u8], &'static [u8], u8, u8, bool);
    let rows: [Never; 3] = [
        // Stored up to 5, the ancestor drew 6: the wish clamps to 5, which is resident.
        (&[4, 5], &[4, 5], 6, 7, false),
        (&[4, 5], &[5], 6, 5, false),
        // The level the ancestor drew is stored and buildable but not resident: it must arrive.
        (&[4, 5, 6, 7], &[5], 6, 7, true),
    ];
    for (index, (stored, resident, finest, wish, held)) in rows.iter().enumerate() {
        for far in FARS {
            let mut rig = Rig::new(two_regions_with_coarse_children(), 512, 3, 1);
            rig.run(60, &ids(&[2, 3]), &camera("fit"), 8.0);
            set_applied(&mut rig, 2, *finest);
            {
                let node = rig.nodes.get_mut(&NodeId::new(10)).expect("node 10");
                node.stored = stored.to_vec();
                node.buildable = (stored[0], *stored.last().expect("stored"));
            }
            set_resident(&mut rig, 10, resident);
            let probe = refine_probe(&mut rig, 10, *wish, far);
            let case = format!("row {index} {far:?}: {probe:?}");
            assert_eq!(probe.held, *held, "{case}");
            assert_eq!(probe.old_held, *held, "{case}");
        }
    }
}

/// A node with data in octant 1 straight ahead and in octant 2 just off-screen to the side, seen
/// from `EYE` along +x, then from a pose turned by `YAW`: octant 2 enters the view at `YAW`.
const EYE: [f64; 3] = [-1.0, 32.0, 32.0];
/// Below the drift budget of the node (`ROTATION_SHARE / tan_corner` of the rotation).
const YAW: f64 = 0.01;

fn looking(yaw: f64) -> ViewProjection {
    let mut camera = camera("zoom");
    let at = [EYE[0] + 10.0 * yaw.cos(), EYE[1] + 10.0 * yaw.sin(), EYE[2]];
    assert!(camera.set_pose(EYE, at));
    view(&camera)
}

#[test]
fn a_residency_change_measures_the_octants_in_view_at_the_current_pose() {
    let (id, cut) = (NodeId::new(2), ids(&[2]));
    let at = |lo: [f64; 3], hi: [f64; 3]| {
        Some(
            SpatialBounds::new(
                std::array::from_fn(|a| ORIGIN[a] + lo[a]),
                std::array::from_fn(|a| ORIGIN[a] + hi[a]),
            )
            .unwrap(),
        )
    };
    let mut data = [None; 8];
    data[1] = at([40.0, 28.0, 28.0], [60.0, 31.0, 31.0]);
    // Its far depth sits within 0.15 of the view depth that decides "in view" at both poses.
    data[2] = at([2.0, 50.0, 28.0], [13.9, 60.0, 32.0]);
    let mut nodes = octant_nodes();
    let node = nodes.get_mut(&id).unwrap();
    node.bounds = cube(ORIGIN, SIDE);
    node.octant_data_bounds = Some(data);
    let mut rig = Rig::new(nodes, 256, 64, 0);
    let (before, after) = (looking(0.0), looking(YAW));
    let behind = |view: &ViewProjection| {
        let node = &rig.nodes[&id];
        masked_probes(
            view,
            &grid(),
            ORIGIN,
            node.bounds,
            &std::array::from_fn(|o| node.probe_box(o)),
        )
        .1
        .0
    };
    assert_eq!(
        behind(&before) >> 2 & 1,
        1,
        "setup: octant 2 is out of view"
    );
    assert_eq!(
        behind(&after) >> 2 & 1,
        0,
        "setup: octant 2 comes into view"
    );
    for _ in 0..40 {
        rig.step_view(&cut, &before, BOUND_TARGET);
    }
    // One cell right by the eye: the bound depends on the region only through what is in view.
    put_pages_at(&mut rig, id, [3.0, 32.0, 33.0]);
    // A level leaves: a residency change at the turned pose, which the node is not yet due for.
    let key = *rig
        .pool
        .levels
        .iter()
        .filter(|(key, level)| key.node == id && level.data.is_some())
        .map(|(key, _)| key)
        .max_by_key(|key| key.depth)
        .expect("a resident level");
    let frame = rig.pool.frame;
    rig.pool.evict_resident(key, frame);
    let pose = rig.pool.depths[&id].pose;
    rig.step_view(&cut, &after, BOUND_TARGET);
    assert_eq!(
        rig.pool.depths[&id].pose, pose,
        "setup: the node was not redone"
    );
    assert_eq!(
        rig.pool.data_bound(id),
        bound_from_pages(&rig, id, &after),
        "the bound of the pose it is evaluated at"
    );
}

/// Plans `frames` frames of the root of `hold_rig` and checks the `node_*` fields of each against
/// their definition. Returns the frames that carried a step and the last entry.
fn plan_and_check_runs(rig: &mut Rig, frames: usize) -> (usize, CellCutEntry) {
    let zoom = camera("zoom");
    let root = NodeId::new(1);
    let mut steps = 0;
    let mut last = None;
    for frame in 0..frames {
        let plan = d5_step_at(rig, &ids(&[1]), &zoom, 2.0);
        let entry = plan.entries.iter().find(|e| e.node == root).expect("root");
        let (anchor, run, target) = (
            entry.node_anchor.expect("an anchor"),
            entry.node_run.expect("a run"),
            entry.node_target.expect("a target"),
        );
        assert!((run.0..=run.1).contains(&anchor));
        assert!(run.1 <= target.max(anchor));
        let wish = entry.wanted.iter().flatten().min().copied();
        assert_eq!(
            entry.node_stage,
            run.1 < target || wish.is_some_and(|wish| wish < run.0),
            "frame {frame}: {entry:?}"
        );
        assert_eq!(
            entry.node_stage,
            entry.node_step.is_some(),
            "frame {frame}: a staged node asks for a level"
        );
        if let Some(step) = entry.node_step {
            steps += 1;
            assert!(step == run.1 + 1 || step + 1 == run.0, "frame {frame}");
        }
        last = Some(entry.clone());
    }
    (steps, last.expect("a frame was planned"))
}

#[test]
fn the_planned_cut_carries_the_node_run_and_the_pool_keeps_the_applied_run() {
    // Plumbing of plan task 10b: `fill_node_runs` fills the `node_*` fields of the planned
    // entries and `plan` stores the applied run (without D5) that the next frame reads as `prev`.
    let mut rig = hold_rig();
    let root = NodeId::new(1);
    let (_, entry) = plan_and_check_runs(&mut rig, 9);
    let applied = rig.pool.applied_run_of(root).expect("the root was applied");
    let own = rig.pool.cell_active_depths(
        &entry,
        CellPool::node_depth(&grid(), rig.nodes[&root].bounds),
        None,
    );
    assert_eq!(Some(applied), own.first().copied().zip(own.last().copied()));
}

#[test]
fn a_planned_node_waiting_for_a_level_is_staged_and_names_it() {
    // The same plumbing with a residency gap: nothing above depth 8 is resident, so the run
    // stops short of its target and the node asks for the level that extends it.
    let mut rig = hold_rig();
    for depth in 9..=14 {
        let key = rig.pool.key(NodeId::new(1), depth);
        rig.pool.levels.remove(&key);
    }
    rig.pool.residency_epoch += 1;
    let (steps, _) = plan_and_check_runs(&mut rig, 3);
    assert!(steps > 0, "a run short of its target has a step");
}

#[test]
fn a_node_entering_with_no_before_takes_its_prev_from_the_history() {
    // Task 10b, C1: the hold's `before` is empty when its cover rule finds no covering cell of
    // the cut (`Cover::Nothing`), yet today's per-octant machinery starts the entering node from
    // the depth its applied ancestor drew. `prev` follows that history and is never `None` when
    // there is one.
    let mut rig = hold_rig();
    let zoom = camera("zoom");
    for _ in 0..9 {
        d5_step_at(&mut rig, &ids(&[1]), &zoom, 2.0);
    }
    let root = NodeId::new(1);
    let child = NodeId::new(2);
    let finest = rig.pool.applied_finest[&root];
    let node = &rig.nodes[&child];
    assert!(
        !rig.pool.applied_drawn.contains_key(&child) && !rig.pool.applied_run.contains_key(&child),
        "setup: the child did not draw"
    );
    let empty = FastMap::default();
    assert_eq!(
        rig.pool.node_prev(child, node, None, &empty, &grid()),
        Some(finest),
        "the ancestor's finest drawn depth, from the entering seed"
    );
    // A `before` of the hold wins over the seed.
    let mut before = FastMap::default();
    before.insert(child, 9);
    assert_eq!(
        rig.pool.node_prev(child, node, None, &before, &grid()),
        Some(9)
    );
}
