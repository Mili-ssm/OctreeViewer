//! Synthetic paged octree (always on).

use super::*;

#[test]
fn synthetic_ladder_streams_parents_first_without_holes() {
    let mut h = synthetic_harness(1 << 20, 2.0);
    let view = synthetic_views().0;
    for _ in 0..400 {
        h.frame(&view);
        if !h.dirty && h.stage == Some(2.0) && !h.driver.needs_frame() {
            break;
        }
    }
    h.report("synthetic streaming");
    // Coarse to fine: 16 -> 8 -> 4 -> 2, never back up.
    assert_eq!(h.totals.stages, vec![16.0, 8.0, 4.0, 2.0]);
    assert_eq!(h.totals.stage_drops, 0);
    assert!(h.totals.longest_retained <= MAX_RETAINED_STREAK);
    assert!(h.totals.max_gap <= MAX_CELL_CUT_GAP);
    assert_eq!(h.totals.dtf, 0);
    assert!(h.target.uploads_total > 0);
}

#[test]
fn the_oracle_reports_a_hidden_cell_whose_refinement_does_not_draw() {
    let mut h = synthetic_harness(1 << 20, 2.0);
    let view = synthetic_views().0;
    // Along the ladder some cut has a node hidden under a refinement.
    let (mut cut, hider) = (0..400)
        .find_map(|_| {
            h.frame(&view);
            let cut = CutView::new(&h.dataset, h.last_plan.as_ref()?);
            let hider = (0..cut.nodes.len())
                .find(|i| !cut.descendants[*i].is_empty() && cut.entries[*i].mask != [0; 8]);
            hider.map(|hider| (cut, hider))
        })
        .expect("a cut with a node hidden under a refinement");
    assert!(cut.holes().is_empty());
    // The ancestor wants an octant its mask hides under the refinement (a pool that kept the
    // octant while a child covers it), and the refinement stops drawing: the hidden cells are
    // uncovered.
    let hidden_octant = (0..8)
        .find(|o| {
            (0..512).any(|n| {
                let c = [n % 8, n / 8 % 8, n / 64];
                octant_of_cell(c) == *o && cell_hidden(&cut.entries[hider].mask, c)
            })
        })
        .expect("the mask hides a cell");
    cut.entries[hider].wanted[hidden_octant] = Some(0);
    for j in cut.descendants[hider].clone() {
        cut.active[j] = [None; 8];
    }
    assert!(
        cut.holes().iter().any(|hole| hole.contains("hidden true")),
        "a hidden cell under a refinement that draws nothing is a hole"
    );
}

#[test]
fn the_cell_oracle_reports_a_node_whose_active_depths_are_missing() {
    let mut h = synthetic_harness(1 << 20, 2.0);
    let view = synthetic_views().0;
    h.settle(&view, 3, 400);
    let plan = h.last_plan.clone().expect("a frame ran");
    let digest = cell_cut_digest(&plan.entries);
    // The state the harness checked passes when it is checked again.
    h.cell_signature = None;
    h.check_cell_cut(0, &plan, digest);
    assert!(h.cell_ctx.is_some(), "the check ran to its end");
    // A node that wants its octants, alone in its footprint, loses every active depth: the
    // samples inside it are drawn by no node.
    let lone = (0..h.cell_nodes.len())
        .find(|&j| {
            let a = &h.cell_nodes[j];
            plan.entries[j].wanted.iter().any(Option::is_some)
                && plan.active.iter().any(|level| level.key.node == a.id)
                && h.cell_nodes.iter().enumerate().all(|(k, b)| {
                    k == j
                        || (0..3)
                            .any(|axis| a.max[axis] <= b.min[axis] || b.max[axis] <= a.min[axis])
                })
        })
        .expect("a node of the cut that overlaps no other");
    let mut broken = plan.clone();
    let lost = h.cell_nodes[lone].id;
    broken.active.retain(|level| level.key.node != lost);
    h.cell_signature = None;
    let reported = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        h.check_cell_cut(0, &broken, digest);
    }))
    .expect_err("the oracle reports the missing active depths");
    let message = reported
        .downcast_ref::<String>()
        .cloned()
        .unwrap_or_default();
    assert!(message.contains("is drawn by no node"), "{message}");
}

/// One depth of a multi-depth node's active run goes missing while the node keeps the others:
/// the samples that depth drew end coarser than before (R3-Q21) or lose their drawer.
fn a_missing_depth_of_a_run_is_reported(pick: impl Fn(&[u8]) -> u8) {
    let mut h = synthetic_harness(1 << 20, 2.0);
    let view = synthetic_views().0;
    h.settle(&view, 3, 400);
    let plan = h.last_plan.clone().expect("a frame ran");
    let digest = cell_cut_digest(&plan.entries);
    h.cell_signature = None;
    h.check_cell_cut(0, &plan, digest);
    // The node with the longest active run, which has a depth strictly between its ends.
    let runs: Vec<(NodeId, Vec<u8>)> = plan
        .entries
        .iter()
        .map(|entry| {
            let mut depths: Vec<u8> = plan
                .active
                .iter()
                .filter(|level| level.key.node == entry.node)
                .map(|level| level.key.depth)
                .collect();
            depths.sort_unstable();
            (entry.node, depths)
        })
        .collect();
    let (node, depths) = runs
        .into_iter()
        .filter(|(_, depths)| depths.len() >= 3)
        .max_by_key(|(_, depths)| depths.len())
        .expect("a node of the cut with three active depths");
    let missing = pick(&depths);
    let mut broken = plan.clone();
    broken
        .active
        .retain(|level| !(level.key.node == node && level.key.depth == missing));
    assert!(broken.active.len() < plan.active.len());
    h.cell_signature = None;
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        h.check_cell_cut(0, &broken, digest);
    }))
    .expect_err("the oracle reports the missing depth");
}

/// A node whose active set is {11, 18} with 12..=17 buildable but not resident has a gap the
/// oracle must report; a usable middle depth, an unbuildable one and the D5 depth are not gaps.
#[test]
fn the_oracle_reports_non_resident_gaps_in_a_nodes_active_set() {
    let resident = [11_u8, 18];
    let usable = |depth: u8| resident.contains(&depth);
    assert_eq!(
        active_gaps_of(&[11, 18], None, (9, 19), usable),
        [12, 13, 14, 15, 16, 17]
    );
    assert_eq!(active_gaps_of(&[11, 18], None, (9, 19), |_| true), []);
    assert_eq!(active_gaps_of(&[11, 18], None, (11, 12), usable), [12]);
    assert_eq!(
        active_gaps_of(&[11, 12, 13], Some(14), (9, 19), usable),
        [] as [u8; 0]
    );
    assert_eq!(
        active_gaps_of(&[11, 17], Some(14), (9, 19), usable),
        [12, 13, 15, 16]
    );
    assert_eq!(active_gaps_of(&[], None, (9, 19), usable), []);
}

// Observed on the synthetic ladder (node 14, active depths 9..=19, depth 14 removed while 31 lattice
// samples drew it): the per-cell twin falls back on a neighbouring depth that is not coarser than
// `min(before, wanted)`, so only the contiguity check of `check_active_runs` reports it.
#[test]
fn the_cell_oracle_reports_a_middle_depth_missing_from_a_run() {
    a_missing_depth_of_a_run_is_reported(|depths| depths[depths.len() / 2]);
}

#[test]
fn the_cell_oracle_reports_the_finest_depth_missing_from_a_run() {
    a_missing_depth_of_a_run_is_reported(|depths| *depths.last().unwrap());
}

#[test]
fn synthetic_build_cap_defers_without_waiting_for_the_camera() {
    let view = synthetic_views().0;
    let mut free = synthetic_harness(1 << 20, 2.0);
    free.settle(&view, 3, 400);
    let expected = free.totals.drawn_digest;
    // Same dataset with every block already decoded but no cells built (a fresh driver and
    // target), so one update has several nodes to build and the cap defers the rest.
    let mut capped = free;
    capped.driver = driver_for(&capped.dataset, u64::MAX);
    capped.target = StrictTarget::default();
    capped.dataset.pool_reset();
    capped.dataset.driver_replaced();
    capped.camera_moved();
    // The new pool draws nothing yet: the old image is not a baseline for lost samples.
    capped.cell_seen.clear();
    capped.totals = Totals::default();
    // The ladder is already at the target rung: every selected node waits for the cap.
    capped.stage = Some(2.0);
    capped.config.max_node_builds = 1;
    let mut deferred_frames = 0;
    let mut builds = 0;
    // Frames run only while the ladder asks for them, as the viewer loop does.
    for _ in 0..800 {
        if !capped.dirty && !capped.driver.needs_frame() {
            break;
        }
        let info = capped.frame(&view);
        // `deferred` also counts the level backlog; only node deferral is compared with builds.
        deferred_frames += usize::from(info.nodes_deferred > 0);
        builds += info.built;
    }
    capped.report("synthetic capped builds");
    assert!(deferred_frames > 0, "the cap must defer some builds");
    assert!(builds > deferred_frames, "deferred nodes were built later");
    assert_eq!(
        capped.totals.drawn_digest, expected,
        "the capped run ends on the cut of the uncapped one with the camera still"
    );
}

#[test]
fn synthetic_regraft_restores_the_cut_without_holes() {
    // One page of 100 bytes fits the 150-byte budget.
    let mut h = synthetic_harness(150, 2.0);
    let (a, b) = synthetic_views();
    h.settle(&a, 3, 600);
    let before = h.totals.drawn_digest;
    assert!(
        h.dataset
            .metadata
            .hierarchy()
            .node(NodeId::new(10))
            .is_some()
    );
    let used_a = h.dataset.pages.as_ref().unwrap().used_bytes();
    assert_eq!(used_a, 100, "only the page of anchor 2 is resident");

    h.camera_moved();
    h.settle(&b, 3, 900);
    assert!(
        h.dataset
            .metadata
            .hierarchy()
            .node(NodeId::new(10))
            .is_none(),
        "the page of A was evicted"
    );
    assert!(
        h.dataset
            .metadata
            .hierarchy()
            .node(NodeId::new(18))
            .is_some()
    );
    // B needs the page of anchor 9 and the budget holds one page: A's page left by eviction.
    assert!(h.dataset.pages.as_ref().unwrap().evictions() > 0);
    println!(
        "synthetic regraft: {} grafts refused",
        h.totals.graft_refused
    );

    h.camera_moved();
    h.settle(&a, 3, 900);
    h.report("synthetic regraft");
    assert!(
        h.dataset
            .metadata
            .hierarchy()
            .node(NodeId::new(10))
            .is_some()
    );
    assert_eq!(
        h.totals.drawn_digest, before,
        "the cut at A is the one from before"
    );
    assert_eq!(h.totals.dtf, 0);
}

#[test]
fn synthetic_page_of_a_drawn_node_is_refused() {
    let mut h = synthetic_harness(150, 2.0);
    let (a, _) = synthetic_views();
    h.settle(&a, 3, 600);
    let drawn = h.dataset.cells.pool_drawn().to_vec();
    assert!(
        drawn.contains(&NodeId::new(10)),
        "the pool draws a node of page 1"
    );
    let error = h
        .dataset
        .graft_page((h.pages)(NodeId::new(9)).unwrap())
        .unwrap_err();
    assert!(
        matches!(error, LodDatasetError::PageBudget { .. }),
        "{error}"
    );
    assert!(
        h.dataset
            .metadata
            .hierarchy()
            .node(NodeId::new(10))
            .is_some()
    );
    assert!(
        h.dataset
            .metadata
            .hierarchy()
            .node(NodeId::new(18))
            .is_none()
    );
}

/// A straight zoom towards the synthetic cube: at rest at each distance, along the view axis the
/// depth a node draws never increases with the distance to the eye (nearer cells are finer or
/// equal). Along the axis the viewing angle of a cell is constant, which off-axis samples lack:
/// the projected edge of a cube depends on the angle too, so a farther off-axis point may draw
/// finer.
#[test]
fn cell_mode_bands_follow_the_distance() {
    let target = [SIDE / 2.0; 3];
    let direction = [0.6_f64, 0.5, 0.62];
    let norm = direction.iter().map(|v| v * v).sum::<f64>().sqrt();
    let mut h = synthetic_harness(1 << 20, 2.0);
    let mut banded_nodes = 0;
    for distance in [300.0, 150.0, 80.0, 40.0, 20.0, 10.0] {
        let eye: [f64; 3] = std::array::from_fn(|a| target[a] + direction[a] / norm * distance);
        let mut camera = OrbitCamera::default();
        assert!(camera.fit(
            SceneBounds {
                min: [0.0; 3],
                max: [SIDE; 3],
            },
            [800, 600],
        ));
        assert!(camera.set_pose(eye, target));
        let view = ViewProjection::new(camera, [800, 600], 1.0, VoxelBuildLimits::default());
        h.camera_moved();
        h.settle(&view, 3, 2_000);
        let ctx = h.cell_ctx.as_ref().expect("a cell-mode frame ran");
        // Points of the view axis from the eye through the target and beyond, dataset frame.
        let mut per_node: HashMap<NodeId, Vec<u8>> = HashMap::new();
        // The node the axis enters first, nearest the eye.
        let mut nearest_node: Option<NodeId> = None;
        for step in 0..=4_000 {
            let along = distance * (f64::from(step) / 2_000.0);
            let p: [f64; 3] = std::array::from_fn(|a| eye[a] - direction[a] / norm * along);
            let rel: [f64; 3] =
                std::array::from_fn(|a| (p[a] - ctx.origin[a]) * ctx.scale - ctx.eye[a]);
            for node in &h.cell_nodes {
                if !(0..3).all(|a| p[a] >= node.min[a] && p[a] <= node.max[a]) {
                    continue;
                }
                if let Some(depth) = drawn_depth_at(&ctx.lod, &node.lod, rel, node.lo, node.side) {
                    nearest_node.get_or_insert(node.id);
                    per_node.entry(node.id).or_default().push(depth);
                }
            }
        }
        let nearest = nearest_node.expect("the view axis crosses the cut");
        let depths = &per_node[&nearest];
        let distinct: HashSet<u8> = depths.iter().copied().collect();
        // Far away a node is one band; at the closest distance the first node entered is a
        // single deep band (observed: 33); in between it must show at least two depths.
        assert!(
            distance > 50.0 || distance < 15.0 || distinct.len() >= 2,
            "distance {distance}: the nearest node {nearest:?} draws one depth {depths:?}"
        );
        for (node, depths) in per_node {
            // `depths` follows the distance to the eye, nearest first.
            assert!(
                depths.windows(2).all(|pair| pair[1] <= pair[0]),
                "distance {distance}: node {node:?} draws {depths:?} along the view axis"
            );
            let distinct: HashSet<u8> = depths.iter().copied().collect();
            banded_nodes += usize::from(distinct.len() > 1);
        }
    }
    assert!(banded_nodes > 0, "the zoom never produced a band");
}
