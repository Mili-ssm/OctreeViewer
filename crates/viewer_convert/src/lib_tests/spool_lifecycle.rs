//! In-memory spools, preorder commits, failing stores and the ready queue.

use super::*;

#[test]
fn a_range_split_plan_leaves_no_live_spool_once_its_blocks_are_released() {
    use crate::las_source::open_las_reader;
    use viewer_convert_core::pipeline::spool_store::MemSpoolStore;
    use viewer_convert_core::source::inspect_source;
    let tmp = test_dir();
    let input = tmp.path().join("deep.las");
    deep_child_fixture(&input);
    let reader = open_las_reader(&input).unwrap();
    let source = inspect_source(&reader).unwrap();
    let options = cluster_options(8).with_range_split_points_for_tests(150);
    let store = MemSpoolStore::new(true);
    let before = split_counter();
    let plan = plan_from_reader(
        reader,
        &input,
        &source,
        options,
        &store,
        &mut |_| true,
        true,
    )
    .unwrap();
    assert_eq!(plan.spatial_workers_used, 8);
    let log = split_log_since(before);
    assert!(
        log.iter().any(|split| split.chained),
        "the fixture must split a child of a split node"
    );
    assert!(
        store.stats().live_spools > 0,
        "leaf and LOD spools are retained"
    );
    for block in &plan.blocks {
        block.read(&store, source.frame).unwrap();
        block.release_spool_after_write(&store).unwrap();
    }
    let drained = store.stats();
    assert_eq!((drained.live_bytes, drained.live_spools), (0, 0));
}

/// Live-spool peak of variant c with one worker in `benchmarks/model_node_job_arena.py`, replayed
/// from the node events of a run: jobs run in `start_seq` order; a job's child spools are counted
/// in full at its start (each is its own file), its LOD sample joins at its end, and its input
/// dies at its end. Leaf spools and LOD samples stay live until the block phase. A node that
/// became a leaf (all points equal) deletes its scratch children at its end. Pure leaves are no
/// jobs.
fn variant_c_serial_peak(events: &[viewer_convert_core::diagnostics::NodeSpoolEvent]) -> u64 {
    let mut jobs = events
        .iter()
        .filter(|e| !e.is_leaf || e.became_leaf_all_equal)
        .collect::<Vec<_>>();
    jobs.sort_by_key(|e| e.start_seq);
    let mut live: u64 = events
        .iter()
        .filter(|e| e.parent_id.is_none())
        .map(|e| e.input_spool_bytes)
        .sum();
    let mut peak = live;
    for job in jobs {
        let children: u64 = if job.is_leaf {
            job.child_spool_bytes.iter().sum()
        } else {
            events
                .iter()
                .filter(|e| e.parent_id == Some(job.node_id))
                .map(|e| e.input_spool_bytes)
                .sum()
        };
        live += children;
        peak = peak.max(live);
        if job.is_leaf {
            live -= children;
        } else {
            live += job.lod_bytes;
            peak = peak.max(live);
            live -= job.input_spool_bytes;
        }
    }
    peak
}

#[test]
fn mem_spool_store_peak_matches_the_native_ledger_and_drains_after_the_block_phase() {
    use crate::{
        file_spool_store::FileSpoolStore, las_source::open_las_reader, workspace::Workspace,
    };
    use viewer_convert_core::pipeline::spool_store::{MemSpoolStore, SpoolAccounting};
    use viewer_convert_core::source::inspect_source;

    let tmp = test_dir();
    let input = tmp.path().join("source.las");
    fixture(&input, false, true, 4_000);
    let options = leaf_new(64, 0.01).unwrap().with_spatial_workers(1).unwrap();
    let reader = open_las_reader(&input).unwrap();
    let source = inspect_source(&reader).unwrap();

    let workspace = Workspace::create(&tmp.path().join("native.oct")).unwrap();
    let native = FileSpoolStore::new(workspace.root(), true);
    let native_plan = plan_from_reader(
        reader,
        &input,
        &source,
        options,
        &native,
        &mut |_| true,
        true,
    )
    .unwrap();
    let native_phases = native_plan.spatial_phases.unwrap();
    assert!(native_phases.peak_live_spool_bytes > 0);
    let native_totals = native.ledger_totals().unwrap();
    assert_eq!(native_totals.written, native_phases.spool_bytes_written);
    assert!(native_totals.physical_written <= native_totals.written);
    assert!(native_totals.physical_peak_live <= native_totals.peak_live);

    let reader = open_las_reader(&input).unwrap();
    let mem = MemSpoolStore::new(true);
    let mem_plan =
        plan_from_reader(reader, &input, &source, options, &mem, &mut |_| true, true).unwrap();
    assert_eq!(mem.stats().peak_bytes, native_phases.peak_live_spool_bytes);
    assert_eq!(
        mem.ledger_totals().unwrap().written,
        native_phases.spool_bytes_written
    );
    assert_eq!(mem_plan.blocks.len(), native_plan.blocks.len());
    assert!(
        mem.stats().live_spools > 0,
        "leaf and LOD spools are retained"
    );

    for block in &mem_plan.blocks {
        block.read(&mem, source.frame).unwrap();
        block.release_spool_after_write(&mem).unwrap();
    }
    let bound = variant_c_serial_peak(&native_phases.node_spool_events);
    assert!(
        mem.stats().peak_bytes <= bound && native_phases.peak_live_spool_bytes <= bound,
        "mem {} native {} bound {bound}",
        mem.stats().peak_bytes,
        native_phases.peak_live_spool_bytes
    );
    // The bound must not be vacuous: some spools die before others are written.
    assert!(bound < native_phases.spool_bytes_written);
    let drained = mem.stats();
    assert_eq!((drained.live_bytes, drained.live_spools), (0, 0));
}

/// `count` identical points with the header max X widened, so the root is divisible and
/// discovers only after its scan that every point is equal (I3).
fn all_equal_root_fixture(path: &std::path::Path, count: usize) {
    {
        let header = Builder::from((1, 2)).into_header().unwrap();
        let mut writer = Writer::from_path(path, header).unwrap();
        for index in 0..count {
            writer
                .write_point(Point {
                    x: 10.0,
                    y: 20.0,
                    z: 30.0,
                    intensity: index as u16,
                    ..Point::default()
                })
                .unwrap();
        }
    }
    let mut bytes = fs::read(path).unwrap();
    bytes[179..187].copy_from_slice(&50.0_f64.to_le_bytes());
    fs::write(path, bytes).unwrap();
}

/// 600 identical points in the lowest root octant plus 200 distinct ones in the highest: the low
/// child is divisible and becomes a leaf after its scan (all points equal).
fn equal_cluster_fixture(path: &std::path::Path) {
    let header = Builder::from((1, 2)).into_header().unwrap();
    let mut writer = Writer::from_path(path, header).unwrap();
    for index in 0..800usize {
        let point = if index < 600 {
            [10.0, 10.0, 10.0]
        } else {
            [60.0 + (index - 600) as f64 * 0.15, 90.0, 90.0]
        };
        writer
            .write_point(Point {
                x: point[0],
                y: point[1],
                z: point[2],
                ..Point::default()
            })
            .unwrap();
    }
}

/// Parents before children, children in ascending key, a depth-first walk in child-key order
/// equal to the hierarchy query order, and every block id used exactly once (V8 pages number
/// nodes as (page, index) and blocks in page order, so neither is a preorder range).
fn assert_preorder_structure(output: &std::path::Path) {
    let reader = FormatReader::new(fs::File::open(output).unwrap()).unwrap();
    let hierarchy = reader.metadata().hierarchy();
    let count = hierarchy.stats().node_count as u64;
    let mut visited = Vec::new();
    let mut stack = vec![NodeId::new(0)];
    while let Some(id) = stack.pop() {
        visited.push(id.get());
        let node = hierarchy.node(id).unwrap();
        let children = hierarchy.child_ids(id);
        let subdivision = viewer_core::SpatialSubdivision::new(
            node.bounds(),
            viewer_convert_core::options::SPATIAL_DIVISIONS,
        )
        .unwrap();
        let keys = children
            .iter()
            .map(|child| {
                let child = hierarchy.node(*child).unwrap();
                assert!(child.parent() == Some(id) && child.id().get() > id.get());
                let (min, max) = (child.bounds().min(), child.bounds().max());
                let cell = subdivision.cell_index(std::array::from_fn(|axis| {
                    f64::midpoint(min[axis], max[axis])
                }));
                // V8 orders children with x as the most significant axis (cell_index has x
                // least significant): reverse the three bits. Re-derived in D4 Task 5c.
                ((cell & 1) << 2) | (cell & 2) | (cell >> 2)
            })
            .collect::<Vec<_>>();
        assert!(keys.windows(2).all(|pair| pair[0] < pair[1]), "{keys:?}");
        stack.extend(children.iter().rev().copied());
    }
    let queried = hierarchy
        .query(hierarchy.stats().root_bounds)
        .map(|node| node.id().get())
        .collect::<Vec<_>>();
    assert_eq!(visited, queried, "the query visits nodes in preorder");
    assert_eq!(visited.len() as u64, count);
    let mut blocks = visited
        .iter()
        .flat_map(|id| hierarchy.node(NodeId::new(*id)).unwrap().blocks().to_vec())
        .map(viewer_core::BlockId::get)
        .collect::<Vec<_>>();
    blocks.sort_unstable();
    assert_eq!(blocks, (0..blocks.len() as u64).collect::<Vec<_>>());
}

type WriteFixture = fn(&std::path::Path);

fn assert_jobs_commit_in_preorder(fixtures: &[(&str, WriteFixture)]) {
    let tmp = test_dir();
    let dir = tmp.path();
    for (name, write) in fixtures {
        let input = dir.join(name);
        write(&input);
        let options = leaf_new(16, 0.01).unwrap();
        // Every worker count gives the same nodes and bytes (planner_snapshot_for).
        let case = dir.join(format!("{name}-case"));
        fs::create_dir(&case).unwrap();
        planner_snapshot_for(&input, &case, options);
        assert_preorder_structure(&case.join("w1.oct"));
    }
}

#[test]
fn node_jobs_commit_nodes_and_blocks_in_preorder() {
    assert_jobs_commit_in_preorder(&[
        ("dense.las", |path| fixture(path, false, true, 4_000)),
        ("clusters.las", eight_cluster_fixture),
    ]);
}

#[test]
#[ignore = "issue #114: coincident points with small leaves fail in the V8 paged index"]
fn node_jobs_commit_coincident_nodes_and_blocks_in_preorder() {
    assert_jobs_commit_in_preorder(&[
        ("coincident.las", coincident_fixture),
        ("equal-root.las", |path| all_equal_root_fixture(path, 300)),
        ("equal-cluster.las", equal_cluster_fixture),
    ]);
}

/// Plans with a `MemSpoolStore`, reads and releases every block as the block phase does, and
/// checks the store lifecycle held and no spool is left.
fn mem_plan_and_drain(
    input: &std::path::Path,
    options: ConvertOptions,
) -> viewer_convert_core::diagnostics::SpatialPhaseDiagnostics {
    use crate::las_source::open_las_reader;
    use viewer_convert_core::pipeline::spool_store::MemSpoolStore;
    use viewer_convert_core::source::inspect_source;
    let reader = open_las_reader(input).unwrap();
    let source = inspect_source(&reader).unwrap();
    let mem = MemSpoolStore::new(true);
    let plan =
        plan_from_reader(reader, input, &source, options, &mem, &mut |_| true, true).unwrap();
    for block in &plan.blocks {
        block.read(&mem, source.frame).unwrap();
        block.release_spool_after_write(&mem).unwrap();
    }
    let stats = mem.stats();
    assert_eq!((stats.live_bytes, stats.live_spools), (0, 0));
    assert!(stats.peak_spools > 0);
    plan.spatial_phases.unwrap()
}

#[test]
fn mem_spool_store_lifecycle_holds_for_whole_plans_and_leaves_no_spool() {
    let tmp = test_dir();
    let dir = tmp.path();
    let dense = dir.join("dense.las");
    fixture(&dense, false, true, 4_000);
    let clusters = dir.join("clusters.las");
    eight_cluster_fixture(&clusters);
    let equal_cluster = dir.join("equal-cluster.las");
    equal_cluster_fixture(&equal_cluster);
    for input in [&dense, &clusters, &equal_cluster] {
        for workers in [1, 2, 4] {
            let options = leaf_new(16, 0.01)
                .unwrap()
                .with_spatial_workers(workers)
                .unwrap();
            let phases = mem_plan_and_drain(input, options);
            assert!(!phases.node_spool_events.is_empty());
        }
    }
}

#[test]
fn an_all_equal_root_is_respooled_and_planned_as_one_leaf() {
    let tmp = test_dir();
    let input = tmp.path().join("equal-root.las");
    all_equal_root_fixture(&input, 300);
    let options = leaf_new(16, 0.01).unwrap();
    let phases = mem_plan_and_drain(&input, options);
    let events = &phases.node_spool_events;
    assert_eq!(events.len(), 1);
    assert!(events[0].is_leaf && events[0].became_leaf_all_equal);
    // 300 records stay below one 32 KiB flush in the LAS scan; the rescan of `SOURCE` finishes
    // (flushes) its children before they are dropped.
    assert!(!events[0].child_spool_bytes.is_empty());

    let output = tmp.path().join("equal-root.oct");
    convert_file(&input, &output, options).unwrap();
    let reader = FormatReader::new(fs::File::open(&output).unwrap()).unwrap();
    let hierarchy = reader.metadata().hierarchy();
    assert_eq!(hierarchy.stats().node_count, 1);
    let root = hierarchy.node(NodeId::new(0)).unwrap();
    assert_eq!(root.point_count(), 300);
    assert_eq!(root.blocks().len(), 300_usize.div_ceil(16));
}

#[test]
fn a_child_with_all_points_equal_becomes_a_leaf() {
    let tmp = test_dir();
    let input = tmp.path().join("equal-cluster.las");
    equal_cluster_fixture(&input);
    let options = leaf_new(16, 0.01).unwrap();
    let phases = mem_plan_and_drain(&input, options);
    let equal = phases
        .node_spool_events
        .iter()
        .filter(|event| event.became_leaf_all_equal)
        .collect::<Vec<_>>();
    assert_eq!(equal.len(), 1);
    assert_eq!((equal[0].parent_id, equal[0].depth), (Some(0), 1));
    assert!(equal[0].is_leaf && equal[0].lod_bytes == 0);

    let output = tmp.path().join("equal-cluster.oct");
    convert_file(&input, &output, options).unwrap();
    let reader = FormatReader::new(fs::File::open(&output).unwrap()).unwrap();
    let hierarchy = reader.metadata().hierarchy();
    let node = hierarchy.node(NodeId::new(equal[0].node_id)).unwrap();
    assert_eq!(node.point_count(), 600);
    assert!(
        hierarchy.child_ids(node.id()).is_empty(),
        "{:?}",
        hierarchy.child_ids(node.id())
    );
}

/// A `MemSpoolStore` whose `reader` fails once for each listed point count (after its delay), so
/// a job error lands on the node whose input spool holds that many records.
struct FailingStore {
    inner: viewer_convert_core::pipeline::spool_store::MemSpoolStore,
    record_bytes: u64,
    failures: std::sync::Mutex<Vec<(u64, std::time::Duration)>>,
}

impl viewer_convert_core::pipeline::spool_store::SpoolStore for FailingStore {
    type Reader = std::io::Cursor<Vec<u8>>;
    fn create(
        &self,
        id: viewer_convert_core::pipeline::spool_store::SpoolId,
        record_bytes: usize,
    ) -> Result<(), ConvertError> {
        self.inner.create(id, record_bytes)
    }
    fn append(
        &self,
        id: viewer_convert_core::pipeline::spool_store::SpoolId,
        bytes: &[u8],
    ) -> Result<(), ConvertError> {
        self.inner.append(id, bytes)
    }
    fn close(
        &self,
        id: viewer_convert_core::pipeline::spool_store::SpoolId,
    ) -> Result<(), ConvertError> {
        self.inner.close(id)
    }
    fn len(
        &self,
        id: viewer_convert_core::pipeline::spool_store::SpoolId,
    ) -> Result<u64, ConvertError> {
        self.inner.len(id)
    }
    fn reader(
        &self,
        id: viewer_convert_core::pipeline::spool_store::SpoolId,
        extent: viewer_convert_core::pipeline::spool_store::SpoolExtent,
    ) -> Result<Self::Reader, ConvertError> {
        let failure = {
            let mut failures = self.failures.lock().unwrap();
            let index = failures
                .iter()
                .position(|(points, _)| points * self.record_bytes == extent.len);
            index.map(|index| failures.remove(index))
        };
        if let Some((points, delay)) = failure {
            std::thread::sleep(delay);
            return Err(ConvertError::Io(std::io::Error::other(format!(
                "injected read failure ({points} points)"
            ))));
        }
        self.inner.reader(id, extent)
    }
    fn delete(
        &self,
        id: viewer_convert_core::pipeline::spool_store::SpoolId,
    ) -> Result<(), ConvertError> {
        self.inner.delete(id)
    }
}

impl viewer_convert_core::pipeline::spool_store::SpoolAccounting for FailingStore {
    fn next_event_seq(&self) -> Option<u64> {
        self.inner.next_event_seq()
    }
    fn ledger_totals(&self) -> Option<viewer_convert_core::pipeline::spool_store::LedgerTotals> {
        self.inner.ledger_totals()
    }
}

/// Eight corner clusters with `sizes[i]` points in cluster `i`: each is one root child, and a
/// cluster larger than every other one has an input spool length no other node shares.
fn sized_cluster_fixture(path: &std::path::Path, sizes: [usize; 8]) {
    let header = Builder::from((1, 2)).into_header().unwrap();
    let mut writer = Writer::from_path(path, header).unwrap();
    for (cluster, size) in sizes.into_iter().enumerate() {
        for index in 0..size {
            let jitter = index as f64 * 0.01;
            let corner = |bit: usize| if cluster & bit == 0 { 10.0 } else { 90.0 } + jitter;
            writer
                .write_point(Point {
                    x: corner(1),
                    y: corner(2),
                    z: corner(4),
                    ..Point::default()
                })
                .unwrap();
        }
    }
}

/// Plans `input` with failing reads (`(points, delay)` per failure) and returns the error and
/// what the store still holds.
fn failing_plan(
    input: &std::path::Path,
    options: ConvertOptions,
    failures: &[(u64, std::time::Duration)],
    on_progress: &mut dyn FnMut(crate::ConversionProgress) -> bool,
) -> (
    ConvertError,
    viewer_convert_core::pipeline::spool_store::MemSpoolStats,
) {
    use crate::las_source::open_las_reader;
    use viewer_convert_core::pipeline::spool_store::MemSpoolStore;
    use viewer_convert_core::source::inspect_source;
    use viewer_convert_core::spatial::SpoolLayout;
    let reader = open_las_reader(input).unwrap();
    let source = inspect_source(&reader).unwrap();
    let store = FailingStore {
        inner: MemSpoolStore::new(true),
        record_bytes: SpoolLayout::source(&source).record_bytes() as u64,
        failures: std::sync::Mutex::new(failures.to_vec()),
    };
    let result = plan_from_reader(reader, input, &source, options, &store, on_progress, true);
    let Err(error) = result else {
        panic!("the plan must fail")
    };
    (error, store.inner.stats())
}

#[test]
fn a_job_error_leaves_no_live_spool() {
    let tmp = test_dir();
    let input = tmp.path().join("clusters.las");
    sized_cluster_fixture(&input, [40, 40, 40, 40, 40, 300, 40, 40]);
    for workers in [1, 2, 4, 8] {
        let (error, stats) = failing_plan(
            &input,
            cluster_options(workers),
            &[(300, std::time::Duration::ZERO)],
            &mut |_| true,
        );
        let case = format!("workers={workers}");
        assert_eq!(
            error.to_string(),
            "injected read failure (300 points)",
            "{case}"
        );
        assert_eq!((stats.live_bytes, stats.live_spools), (0, 0), "{case}");
        assert!(stats.peak_spools > 1, "{case}");
    }
}

#[test]
fn cancelling_a_plan_leaves_no_live_spool() {
    let tmp = test_dir();
    let input = tmp.path().join("clusters.las");
    sized_cluster_fixture(&input, [90, 60, 70, 80, 100, 50, 40, 120]);
    for workers in [1, 2, 4] {
        let mut updates = 0;
        let (error, stats) = failing_plan(&input, cluster_options(workers), &[], &mut |event| {
            if event.stage == ConversionStage::BuildingSpatialHierarchy {
                updates += 1;
                updates < 12
            } else {
                true
            }
        });
        let case = format!("workers={workers}");
        assert!(
            matches!(error, ConvertError::Cancelled { .. }),
            "{case}: {error:?}"
        );
        assert_eq!(updates, 12, "{case}: no progress after the cancel");
        assert_eq!((stats.live_bytes, stats.live_spools), (0, 0), "{case}");
    }
}

#[test]
fn the_ready_queue_returns_the_lowest_preorder_job_error() {
    // Two root children fail in one batch of 8. The serial plan stops at the lowest preorder
    // one; the ready queue must return the same even when that failure arrives last (R25).
    let tmp = test_dir();
    let input = tmp.path().join("clusters.las");
    sized_cluster_fixture(&input, [40, 40, 300, 40, 40, 250, 40, 40]);
    let quick = std::time::Duration::ZERO;
    let (serial, _) = failing_plan(
        &input,
        cluster_options(1),
        &[(300, quick), (250, quick)],
        &mut |_| true,
    );
    let lowest: u64 = if serial.to_string().contains("(300 points)") {
        300
    } else {
        250
    };
    let highest = 550 - lowest;
    for _ in 0..5 {
        let (error, stats) = failing_plan(
            &input,
            cluster_options(8),
            &[
                (lowest, std::time::Duration::from_millis(100)),
                (highest, quick),
            ],
            &mut |_| true,
        );
        assert_eq!(error.to_string(), serial.to_string());
        assert_eq!((stats.live_bytes, stats.live_spools), (0, 0));
    }
}

#[test]
fn a_panicking_progress_callback_propagates_under_the_ready_queue() {
    // The callback runs on the host inside the batch scope; a panic there must close the ready
    // queue so the parked workers exit and the scope can resume the panic instead of hanging.
    let tmp = test_dir();
    let input = tmp.path().join("source.las");
    fixture(&input, false, true, 4_000);
    let output = tmp.path().join("panic.oct");
    let options = leaf_new(16, 0.01).unwrap().with_spatial_workers(2).unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = std::panic::catch_unwind(|| {
            let mut updates = 0;
            convert_file_with_progress(&input, &output, options, |event| {
                if event.stage == ConversionStage::BuildingSpatialHierarchy {
                    updates += 1;
                    assert!(updates < 5, "injected progress panic");
                }
                true
            })
        });
        let _ = sender.send(result.is_err());
    });
    let panicked = receiver
        .recv_timeout(std::time::Duration::from_secs(60))
        .expect("the plan hung after a host panic");
    assert!(panicked);
}

#[test]
fn the_serial_executor_plans_like_the_ready_queue() {
    use crate::{las_source::open_las_reader, pipeline::SystemClock};
    use viewer_convert_core::pipeline::{
        point_source::{LasFactory, LasPointSource, MemBytes, RangeSource},
        spool_store::MemSpoolStore,
    };
    use viewer_convert_core::source::inspect_source;
    use viewer_convert_core::spatial::{build_spatial_plan_from_source, executor::SerialExecutor};

    let tmp = test_dir();
    let input = tmp.path().join("clusters.las");
    sized_cluster_fixture(&input, [40, 40, 300, 40, 40, 250, 40, 40]);
    let bytes: std::sync::Arc<[u8]> = std::fs::read(&input).unwrap().into();
    for workers in [1, 2, 8] {
        let options = cluster_options(workers);
        let reader = open_las_reader(&input).unwrap();
        let source = inspect_source(&reader).unwrap();
        let queue_store = MemSpoolStore::new(true);
        let queue = plan_from_reader(
            reader,
            &input,
            &source,
            options,
            &queue_store,
            &mut |_| true,
            true,
        )
        .unwrap();

        let factory = LasFactory::new(MemBytes(bytes.clone()));
        let mut first = RangeSource::Las(LasPointSource::full(open_las_reader(&input).unwrap()));
        let serial_store = MemSpoolStore::new(true);
        let serial = build_spatial_plan_from_source(
            &mut first,
            &factory,
            &source,
            options,
            &serial_store,
            &SystemClock,
            &SerialExecutor,
            &mut |_| true,
            true,
        )
        .unwrap();
        let shape = |plan: &viewer_convert_core::spatial::SpatialPlan| {
            plan.blocks
                .iter()
                .map(|block| (block.id, block.point_count(), block.is_source_points))
                .collect::<Vec<_>>()
        };
        assert_eq!(shape(&serial), shape(&queue), "workers {workers}");
        assert_eq!(
            serial.estimated_peak_memory_bytes,
            queue.estimated_peak_memory_bytes
        );
    }
}

#[test]
fn the_ready_queue_returns_the_lowest_preorder_error_among_range_jobs() {
    // Node sizes 300 (preorder 2) and 250 (preorder 5). Read failures are injected by extent, so
    // a range of a split node fails as `(range points)` and an unsplit node as its own size. The
    // lowest preorder error must win whichever of the two is split and whichever fails first.
    let tmp = test_dir();
    let input = tmp.path().join("clusters.las");
    sized_cluster_fixture(&input, [40, 40, 300, 40, 40, 250, 40, 40]);
    let quick = std::time::Duration::ZERO;
    let slow = std::time::Duration::from_millis(100);
    // (split points, failure of the lower node, failure of the higher node, expected message)
    let cases = [
        // 300 → 3 ranges of 100; 250 → 3 ranges of 83, 83, 84.
        (100, (100, slow), (83, quick), "(100 points)"),
        // 300 → 2 ranges of 150; 250 stays one job of 250 points.
        (260, (150, slow), (250, quick), "(150 points)"),
        // Neither node reaches the threshold, so both run as one job.
        (400, (300, slow), (250, quick), "(300 points)"),
    ];
    for (split_points, (low, low_delay), (high, high_delay), expected) in cases {
        for _ in 0..3 {
            let before = split_counter();
            let (error, stats) = failing_plan(
                &input,
                cluster_options(8).with_range_split_points_for_tests(split_points),
                &[(low, low_delay), (high, high_delay)],
                &mut |_| true,
            );
            assert!(
                error.to_string().contains(expected),
                "T={split_points}: {error}"
            );
            assert_eq!(
                (stats.live_bytes, stats.live_spools),
                (0, 0),
                "T={split_points}"
            );
            if split_points < 400 {
                assert!(splits_since(before) > 0, "T={split_points}: no split");
            }
        }
    }
    // A higher node that is split fails alone: its error is the one returned.
    let (error, stats) = failing_plan(
        &input,
        cluster_options(8).with_range_split_points_for_tests(100),
        &[(83, quick)],
        &mut |_| true,
    );
    assert!(error.to_string().contains("(83 points)"), "{error}");
    assert_eq!((stats.live_bytes, stats.live_spools), (0, 0));
}

#[test]
fn cancelling_during_range_jobs_leaves_no_live_spool() {
    let tmp = test_dir();
    let input = tmp.path().join("clusters.las");
    sized_cluster_fixture(&input, [90, 60, 70, 80, 100, 50, 40, 120]);
    for workers in [2, 4, 8] {
        for cancel_at in [3, 8, 14] {
            let before = split_counter();
            let mut updates = 0;
            let options = cluster_options(workers).with_range_split_points_for_tests(30);
            let (error, stats) = failing_plan(&input, options, &[], &mut |event| {
                if event.stage == ConversionStage::BuildingSpatialHierarchy {
                    updates += 1;
                    updates < cancel_at
                } else {
                    true
                }
            });
            let case = format!("workers={workers} cancel_at={cancel_at}");
            assert!(
                matches!(error, ConvertError::Cancelled { .. }),
                "{case}: {error:?}"
            );
            assert!(splits_since(before) > 0, "{case}: no split");
            assert_eq!((stats.live_bytes, stats.live_spools), (0, 0), "{case}");
        }
    }
}
