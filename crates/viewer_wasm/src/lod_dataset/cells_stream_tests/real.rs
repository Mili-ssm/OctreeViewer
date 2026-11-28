//! Streaming and regraft tests on the real belvedere dataset (skipped when it is absent).

use super::*;

#[test]
fn real_streaming_ladder_belvedere() {
    let Some(mut real) = open_real("belvedere-v8.oct", 4.0, 256 << 20) else {
        return;
    };
    let anchors = real.anchors_near([0.5, 0.5, 0.5]);
    println!(
        "belvedere: {} root anchors, scale {}, first {:?}",
        anchors.len(),
        real.scale,
        anchors[0]
    );
    let (centre, side) = anchors[0];
    let view = real.view(centre, [0.0, -side * 0.2, side * 0.12]);
    let clock = Instant::now();
    let last = real.harness.settle(&view, 3, 20_000);
    real.harness.report("belvedere streaming");
    println!(
        "belvedere streaming: {:.1} s, final cut {} nodes, stage {}",
        clock.elapsed().as_secs_f64(),
        last.cut_nodes,
        last.stage
    );
    let t = &real.harness.totals;
    assert!(t.longest_retained <= MAX_RETAINED_STREAK);
    assert!(t.max_gap <= MAX_CELL_CUT_GAP);
    assert_eq!(t.dtf, 0);
    assert_eq!(t.stage_drops, 0);
    // Coarse to fine through every rung; a straight jump to 4 px would show up here.
    assert_eq!(t.stages, vec![16.0, 8.0, 4.0]);
    assert_eq!(real.harness.zone_coarsenings, 0);
    assert_eq!(real.harness.unexcused_drops, 0);
    assert_eq!(real.harness.lost_samples, 0);
}

/// Settles belvedere at 1 px with `min_side` (dataset units, set as production does: the dataset
/// for the cut, the pool for the depths), after a dolly toward the data. Returns the number of
/// drawn octants not drawn by the root and the smallest cell side among them (dataset units).
fn drawn_cubes_after_dolly(min_side: Option<f64>) -> (usize, f64) {
    let mut real = open_real("belvedere-v8.oct", 1.0, 256 << 20).expect("real data required");
    real.harness.dataset.set_min_side(min_side);
    real.harness.driver.set_min_side(min_side);
    let (centre, side) = real.anchors_near([0.5, 0.5, 0.5])[0];
    for k in [4.0, 0.02] {
        let view = real.view(centre, [0.0, -side * 0.2 * k, side * 0.12 * k]);
        real.harness.camera_moved();
        real.harness.settle(&view, 3, 20_000);
    }
    let h = &real.harness;
    let (root, root_side) = (
        h.dataset.metadata.hierarchy().root_id(),
        h.dataset.cell_grid().root_side,
    );
    let sides: Vec<f64> = (h.last_entries.iter())
        .filter(|(node, ..)| *node != root)
        .flat_map(|(_, _, drawn)| drawn.iter().flatten())
        .map(|&depth| root_side / 2.0_f64.powi(i32::from(depth)))
        .collect();
    (sides.len(), sides.into_iter().fold(f64::INFINITY, f64::min))
}

/// #103 on real data: with a large minimum and a small px target no cell is drawn below the
/// minimum (the cut ignored it), and without a minimum the same poses do draw smaller cells.
#[test]
#[ignore = "real data (minutes): R3_REQUIRE_REAL_DATA=1 cargo test -p viewer_wasm --profile fast --offline --lib -- --ignored real_belvedere_cells_respect_the_minimum_side"]
fn real_belvedere_cells_respect_the_minimum_side() {
    // Belvedere's cut nodes reach sides/8 of 16 m at this pose: 40 m makes the minimum bind.
    const MIN_SIDE: f64 = 40.0;
    let (samples, smallest) = drawn_cubes_after_dolly(Some(MIN_SIDE));
    println!("with min {MIN_SIDE}: {samples} drawn octants, smallest side {smallest}");
    assert!(samples > 0);
    assert!(
        smallest >= MIN_SIDE * (1.0 - 1e-3),
        "a cell of side {smallest} is drawn below the minimum {MIN_SIDE}"
    );
    let (samples, smallest) = drawn_cubes_after_dolly(None);
    println!("without min: {samples} drawn octants, smallest side {smallest}");
    assert!(
        smallest < MIN_SIDE,
        "the pose must draw below {MIN_SIDE} without a minimum"
    );
}

/// Two far-apart regions of belvedere: the views look at the nearest and the farthest root
/// anchor from the middle of the cube.
fn region_views(real: &Real) -> (ViewProjection, ViewProjection) {
    let anchors = real.anchors_near([0.5, 0.5, 0.5]);
    let look = |(centre, side): ([f64; 3], f64)| real.view(centre, [0.0, -side * 0.2, side * 0.12]);
    (look(anchors[0]), look(*anchors.last().unwrap()))
}

/// Resident page budget for the regraft tests: a little above what region A needs, so region B must evict A's pages.
/// V8 pages are smaller than v7's (no attribute directory entries for LOD columns): A needs 41 934 bytes with
/// `belvedere-v8` against 47 398 with v7 (see the printout of `real_regraft_belvedere`), so the v7 budget of 60 000
/// scales by the same ratio to 53 000 and keeps the same headroom (record `RENDER_DATA_V8_21-11-2026.md`).
const REGRAFT_PAGE_BUDGET: usize = 53_000;

#[test]
fn real_regraft_belvedere() {
    let Some(mut real) = open_real("belvedere-v8.oct", 4.0, REGRAFT_PAGE_BUDGET) else {
        return;
    };
    let (a, b) = region_views(&real);
    real.harness.settle(&a, 3, 20_000);
    let before = real.harness.totals.drawn_digest;
    let drawn_a: Vec<NodeId> = real.harness.dataset.cells.pool_drawn().to_vec();
    let used_a = real.harness.dataset.pages.as_ref().unwrap().used_bytes();
    println!(
        "belvedere regraft: A uses {used_a} of {REGRAFT_PAGE_BUDGET} page bytes, draws {} nodes",
        drawn_a.len()
    );

    real.harness.camera_moved();
    real.harness.settle(&b, 3, 20_000);
    let hierarchy = real.harness.dataset.metadata.hierarchy();
    let evicted: Vec<NodeId> = drawn_a
        .iter()
        .copied()
        .filter(|id| hierarchy.node(*id).is_none())
        .collect();
    println!(
        "belvedere regraft: B settled, {} of A's {} drawn nodes were evicted with their pages \
         ({} page evictions)",
        evicted.len(),
        drawn_a.len(),
        real.harness.dataset.pages.as_ref().unwrap().evictions()
    );
    assert!(
        !evicted.is_empty(),
        "region B must evict a page that held nodes of A's cut"
    );

    real.harness.camera_moved();
    let frames_back = real.harness.totals.frames;
    real.harness.settle(&a, 3, 20_000);
    real.harness.report("belvedere regraft");
    println!(
        "belvedere regraft: {} frames back at A, digest before {before:016x} after {:016x}",
        real.harness.totals.frames - frames_back,
        real.harness.totals.drawn_digest
    );
    let hierarchy = real.harness.dataset.metadata.hierarchy();
    assert!(
        evicted.iter().all(|id| hierarchy.node(*id).is_some()),
        "the evicted page was grafted again with the same node ids"
    );
    assert_eq!(real.harness.totals.drawn_digest, before);
    let t = &real.harness.totals;
    assert!(t.longest_retained <= MAX_RETAINED_STREAK);
    assert!(t.max_gap <= MAX_CELL_CUT_GAP);
    assert_eq!(t.dtf, 0);
    assert_eq!(real.harness.zone_coarsenings, 0);
    assert_eq!(real.harness.unexcused_drops, 0);
    assert_eq!(real.harness.lost_samples, 0);
}

#[test]
fn real_pages_with_drawn_nodes_are_not_evicted() {
    let Some(mut real) = open_real("belvedere-v8.oct", 4.0, REGRAFT_PAGE_BUDGET) else {
        return;
    };
    let (a, b) = region_views(&real);
    real.harness.settle(&a, 3, 20_000);
    real.harness.camera_moved();
    // Travel to B; every frame also tries to graft the page of each root anchor still unloaded
    // (a host fetching greedily). The budget cannot hold them all: the grafts that would need
    // the pages of a drawn node are refused, and the pool never draws a node that left.
    let mut refused = 0;
    let mut retained_frames = 0;
    for frame in 0..60 {
        let info = real.harness.frame(&b);
        retained_frames += usize::from(info.apply == CutApply::RetainedPrevious);
        for anchor in real.anchor_ids.clone() {
            if !real
                .harness
                .dataset
                .metadata
                .hierarchy()
                .is_unloaded(anchor)
            {
                continue;
            }
            let page = (real.harness.pages)(anchor).unwrap();
            if let Err(error) = real.harness.dataset.graft_page(page) {
                assert!(
                    matches!(error, LodDatasetError::PageBudget { .. }),
                    "{error}"
                );
                refused += 1;
            }
            let hierarchy = real.harness.dataset.metadata.hierarchy();
            for drawn in real.harness.dataset.cells.pool_drawn() {
                assert!(
                    hierarchy.node(*drawn).is_some(),
                    "frame {frame}: the pool draws {drawn:?} but its page was evicted"
                );
            }
        }
        if !real.harness.dirty && !real.harness.driver.needs_frame() && frame > 30 {
            break;
        }
    }
    real.harness.report("belvedere pinned");
    println!("belvedere pinned: {refused} grafts refused, {retained_frames} frames retained");
    assert!(refused > 0, "the budget must refuse at least one graft");
    assert_eq!(real.harness.zone_coarsenings, 0);
    assert_eq!(real.harness.unexcused_drops, 0);
    assert_eq!(real.harness.lost_samples, 0);
}
