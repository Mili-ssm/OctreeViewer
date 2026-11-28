//! Spatial workers and the preparation pipeline.

use super::*;

#[test]
fn bounded_spatial_workers_preserve_the_serial_output() {
    let tmp = test_dir();
    let dir = tmp.path().to_owned();
    let input = dir.join("source.las");
    let serial_output = dir.join("serial.oct");
    let parallel_output = dir.join("parallel.oct");
    fixture(&input, false, true, 200);
    let serial_options = leaf_new(16, 0.01)
        .unwrap()
        .with_spatial_workers(1)
        .unwrap()
        .with_root_partition_ranges(0)
        .unwrap();
    let parallel_options = serial_options.with_spatial_workers(2).unwrap();

    let serial = convert_file(&input, &serial_output, serial_options).unwrap();
    let parallel = convert_file(&input, &parallel_output, parallel_options).unwrap();

    assert_eq!(serial.block_preparation.worker_limit, 1);
    assert_eq!(serial.output_validation.worker_limit, 1);
    assert_eq!(parallel.block_preparation.worker_limit, 2);
    assert_eq!(parallel.output_validation.worker_limit, 2);
    assert!(parallel.block_preparation.estimated_bytes_per_worker > 0);
    assert!(parallel.output_validation.estimated_bytes_per_worker > 0);
    let attributes_path = output_attributes_path(&parallel_output);
    let (block_ids, source_block_ids) = output_block_ids(&parallel_output).unwrap();
    let limited = validate_output_pair(
        &parallel_output,
        &attributes_path,
        &OutputValidation {
            total_points: parallel.point_count,
            block_ids: &block_ids,
            source_block_ids: &source_block_ids,
            file_length: fs::metadata(&parallel_output).unwrap().len(),
            attributes_length: fs::metadata(&attributes_path).unwrap().len(),
            requested_workers: 2,
            memory_budget_bytes: 1,
            reserved_memory_bytes: 1,
        },
        &mut |_| true,
        None,
    )
    .unwrap();
    assert_eq!(limited.worker_limit, 1);
    assert_eq!(
        limited.estimated_bytes_per_worker,
        parallel.output_validation.estimated_bytes_per_worker
    );

    assert_eq!(parallel.point_count, serial.point_count);
    assert_eq!(parallel.block_count, serial.block_count);
    assert_eq!(parallel.output_bytes, serial.output_bytes);
    // Two workers reserve their overhead (1 MiB each) and the extra worker's 8 child spool
    // buffers (32 KiB each), and the index is held twice for the append copy: one more index,
    // between its blocks' share and two nodes per block (see `peak_memory_bytes`).
    let fixed = 2 * 1024 * 1024 + 8 * 32 * 1024;
    let index = parallel.estimated_peak_memory_bytes - serial.estimated_peak_memory_bytes - fixed;
    assert!(
        (serial.block_count * 512..=serial.block_count * (512 + 2 * 256)).contains(&index),
        "extra index {index} for {} blocks",
        serial.block_count
    );
    assert_eq!(
        fs::read(&serial_output).unwrap(),
        fs::read(&parallel_output).unwrap()
    );
    assert_eq!(
        fs::read(output_attributes_path(&serial_output)).unwrap(),
        fs::read(output_attributes_path(&parallel_output)).unwrap()
    );
}

fn convert_with_workers(
    input: &std::path::Path,
    output: &std::path::Path,
    options: ConvertOptions,
    workers: usize,
) -> (Vec<u8>, Vec<u8>) {
    convert_file(
        input,
        output,
        options.with_spatial_workers(workers).unwrap(),
    )
    .unwrap();
    output_pair_bytes(output)
}

#[test]
fn spatial_workers_preserve_output_across_counts_and_compression() {
    let tmp = test_dir();
    let dir = tmp.path().to_owned();
    for compressed in [false, true] {
        let input = dir.join(if compressed {
            "source.laz"
        } else {
            "source.las"
        });
        fixture(&input, compressed, true, 4_000);
        let options = leaf_new(64, 0.01).unwrap();
        let case = format!("{compressed}");
        let reference =
            convert_with_workers(&input, &dir.join(format!("ref-{case}.oct")), options, 1);
        for workers in [2, 4, 8] {
            let output = dir.join(format!("w{workers}-{case}.oct"));
            assert_eq!(
                convert_with_workers(&input, &output, options, workers),
                reference,
                "compressed={compressed} workers={workers}"
            );
        }
    }
}

#[test]
#[ignore = "issue #114: coincident points with small leaves fail in the V8 paged index"]
fn coincident_root_output_is_independent_of_worker_count() {
    let tmp = test_dir();
    let dir = tmp.path().to_owned();
    let input = dir.join("coincident.las");
    coincident_fixture(&input);
    let options = leaf_new(64, 0.01).unwrap();
    let reference = convert_with_workers(&input, &dir.join("reference.oct"), options, 1);
    for workers in [2, 4, 8] {
        let output = dir.join(format!("workers-{workers}.oct"));
        assert_eq!(
            convert_with_workers(&input, &output, options, workers),
            reference,
            "coincident workers={workers}"
        );
    }
}

#[test]
fn pipeline_prepares_next_batch_while_consuming_in_order() {
    let (sender, receiver) = std::sync::mpsc::channel();
    let mut written = Vec::new();
    crate::las_source::process_batches(
        &[0, 1, 2, 3],
        1,
        true,
        |batch| {
            if batch[0] == 1 {
                sender.send(()).unwrap();
            }
            Ok(batch[0])
        },
        |batch, prepared| {
            if batch[0] == 0 {
                receiver
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .unwrap();
            }
            assert_eq!(batch[0], prepared);
            written.push(prepared);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(written, [0, 1, 2, 3]);
}

#[test]
fn sliding_window_consumes_in_order_with_bounded_items_in_flight() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let items: Vec<usize> = (0..64).collect();
    let prepared = AtomicUsize::new(0);
    let mut written = Vec::new();
    crate::las_source::process_batches(
        &items,
        3,
        false,
        |item| {
            // The first item is a straggler: later items finish before it.
            if item[0] == 0 {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            prepared.fetch_add(1, Ordering::SeqCst);
            Ok(item[0])
        },
        |item, value| {
            assert_eq!(item[0], value);
            // Consumed or in flight never exceeds the window of 2 * width items.
            assert!(prepared.load(Ordering::SeqCst) <= written.len() + 6);
            written.push(value);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(written, items);
}

#[test]
fn pipeline_propagates_preparation_and_consumer_errors_without_deadlock() {
    for fail_preparation in [false, true] {
        let result = crate::las_source::process_batches(
            &[0, 1, 2, 3],
            1,
            true,
            |batch| {
                if fail_preparation && batch[0] == 1 {
                    Err(ConvertError::InvalidMemoryBudget)
                } else {
                    Ok(batch[0])
                }
            },
            |_, _| {
                if fail_preparation {
                    Ok(())
                } else {
                    Err(ConvertError::InvalidMemoryBudget)
                }
            },
        );
        assert!(matches!(result, Err(ConvertError::InvalidMemoryBudget)));
    }
}

#[test]
fn pipeline_with_no_memory_margin_preserves_serial_output() {
    let tmp = test_dir();
    let dir = tmp.path().to_owned();
    let input = dir.join("source.las");
    let serial_output = dir.join("serial.oct");
    let output = dir.join("tight.oct");
    fixture(&input, false, true, 100);
    let options = leaf_new(8, 0.01).unwrap().with_spatial_workers(1).unwrap();
    let serial = convert_file(&input, &serial_output, options).unwrap();
    let tight = leaf_budget(8, 0.01, serial.estimated_peak_memory_bytes)
        .unwrap()
        .with_spatial_workers(1)
        .unwrap()
        .with_pipelined_writes(true);
    let report = convert_file(&input, &output, tight).unwrap();
    assert_eq!(report.block_preparation.worker_limit, 1);
    assert_eq!(
        fs::read(&output).unwrap(),
        fs::read(&serial_output).unwrap()
    );
    assert_eq!(
        fs::read(output_attributes_path(&output)).unwrap(),
        fs::read(output_attributes_path(&serial_output)).unwrap()
    );
}

#[test]
fn cancelling_pipelined_conversion_does_not_publish_outputs() {
    let tmp = test_dir();
    let dir = tmp.path().to_owned();
    let input = dir.join("source.las");
    let output = dir.join("pipelined.oct");
    fixture(&input, false, true, 100);
    let options = leaf_new(8, 0.01).unwrap().with_pipelined_writes(true);
    let result = convert_file_with_progress(&input, &output, options, |event| {
        event.stage != ConversionStage::ReadingAndWritingBlocks || event.points_processed == 0
    });
    assert!(matches!(result, Err(ConvertError::Cancelled { .. })));
    assert!(!output.exists());
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
}

#[test]
fn serial_and_parallel_conversion_outputs_are_identical_and_readable() {
    let tmp = test_dir();
    let dir = tmp.path().to_owned();
    let input = dir.join("source.las");
    let serial_output = dir.join("serial.oct");
    let parallel_output = dir.join("parallel.oct");
    fixture(&input, false, true, 24);
    let serial_options = leaf_new(8, 0.01).unwrap().with_spatial_workers(1).unwrap();
    let parallel_options = serial_options
        .with_spatial_workers(2)
        .unwrap()
        .with_pipelined_writes(true);

    let serial = convert_file(&input, &serial_output, serial_options).unwrap();
    let parallel = convert_file(&input, &parallel_output, parallel_options).unwrap();
    assert_eq!(
        fs::read(&serial_output).unwrap(),
        fs::read(&parallel_output).unwrap()
    );
    assert_eq!(
        fs::read(output_attributes_path(&serial_output)).unwrap(),
        fs::read(output_attributes_path(&parallel_output)).unwrap()
    );

    let mut reader = FormatReader::new(fs::File::open(&parallel_output).unwrap()).unwrap();
    assert_eq!(
        reader.format_version(),
        viewer_format::FormatVersion::DEFAULT_OUTPUT
    );
    reader
        .attach_attributes(fs::File::open(output_attributes_path(&parallel_output)).unwrap())
        .unwrap();
    assert_eq!(
        reader.metadata().hierarchy().stats().source_point_count,
        Some(24)
    );
    for entry in reader.block_storage_stats().unwrap() {
        let block = reader.read_block(entry.id).unwrap();
        // Leaf blocks carry the whole schema; V8 LOD blocks only intensity and classification.
        let is_leaf = has_attribute(&block, "rgb");
        // V8 leaves add the `las_flags` column to the eight of V5.
        assert_eq!(block.attributes().len(), if is_leaf { 9 } else { 2 });
        let AttributeValues::U16(intensity) = attribute_values(&block, "intensity") else {
            panic!("intensity must remain u16");
        };
        assert_eq!(intensity.len(), block.points().len());
        if !is_leaf {
            continue;
        }
        let AttributeValues::U16(rgb) = attribute_values(&block, "rgb") else {
            panic!("RGB must remain u16");
        };
        let (colors, remainder) = rgb.as_chunks::<3>();
        assert!(remainder.is_empty(), "{remainder:?}");
        assert!(
            colors
                .iter()
                .all(|color| color == &[0x1200, 0xAB00, 0xFF00])
        );
    }
    assert_eq!(serial.point_count, parallel.point_count);
    assert_eq!(serial.block_count, parallel.block_count);
}

#[test]
fn parallel_worker_limits_are_validated_and_budget_is_respected() {
    let tmp = test_dir();
    let dir = tmp.path().to_owned();
    let input = dir.join("source.las");
    let output = dir.join("parallel.oct");
    fixture(&input, false, false, 100);
    let options = leaf_budget(10, 0.01, 17 * 1024 * 1024)
        .unwrap()
        .with_spatial_workers(2)
        .unwrap();

    assert!(matches!(
        ConvertOptions::default().with_spatial_workers(0),
        Err(ConvertError::InvalidSpatialWorkerCount(0))
    ));
    assert!(matches!(
        ConvertOptions::default().with_spatial_workers(MAX_SPATIAL_WORKERS + 1),
        Err(ConvertError::InvalidSpatialWorkerCount(count)) if count == MAX_SPATIAL_WORKERS + 1
    ));
    assert_eq!(
        ConvertOptions::default()
            .with_spatial_workers(MAX_SPATIAL_WORKERS)
            .unwrap()
            .spatial_workers(),
        MAX_SPATIAL_WORKERS
    );
    // A budget that cannot hold two workers lowers the count instead of failing the plan.
    let report = convert_file(&input, &output, options).unwrap();
    assert_eq!(report.spatial_workers_used, 1);
    // Only one worker not fitting is an error.
    let hopeless = leaf_budget(10, 0.01, 1)
        .unwrap()
        .with_spatial_workers(2)
        .unwrap();
    let rejected = dir.join("rejected.oct");
    assert!(matches!(
        convert_file(&input, &rejected, hopeless),
        Err(ConvertError::MemoryBudgetExceeded { .. })
    ));
    assert!(!rejected.exists());
}

#[test]
fn default_worker_count_uses_available_parallelism_with_a_safe_cap() {
    let expected = std::thread::available_parallelism()
        .map_or(1, |workers| workers.get().min(DEFAULT_SPATIAL_WORKERS));

    assert_eq!(ConvertOptions::default().spatial_workers(), expected);
    assert_eq!(
        leaf_budget(10, 0.01, 32 * 1024 * 1024)
            .unwrap()
            .spatial_workers(),
        expected
    );
}

#[test]
fn cancellation_during_parallel_spatial_planning_does_not_publish_output() {
    let tmp = test_dir();
    let dir = tmp.path().to_owned();
    let input = dir.join("source.las");
    let output = dir.join("parallel.oct");
    fixture(&input, false, false, 100);
    let options = leaf_new(10, 0.01).unwrap().with_spatial_workers(2).unwrap();
    let mut hierarchy_updates = 0;

    let result = convert_file_with_progress(&input, &output, options, |event| {
        if event.stage == ConversionStage::BuildingSpatialHierarchy {
            hierarchy_updates += 1;
            hierarchy_updates < 12
        } else {
            true
        }
    });

    assert!(matches!(result, Err(ConvertError::Cancelled { .. })));
    assert_eq!(hierarchy_updates, 12);
    assert!(!output.exists());
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
}

#[test]
fn output_bytes_do_not_depend_on_the_worker_count_with_a_deep_child() {
    let tmp = test_dir();
    let input = tmp.path().join("deep.las");
    deep_child_fixture(&input);
    let mut expected = None;
    for workers in [1, 2, 4, 8, 16] {
        let output = tmp.path().join(format!("deep-{workers}.oct"));
        let report = convert_file(&input, &output, cluster_options(workers)).unwrap();
        assert_eq!(report.spatial_workers_used, workers, "workers={workers}");
        let bytes = read_pair(&output);
        assert_eq!(
            *expected.get_or_insert_with(|| bytes.clone()),
            bytes,
            "workers={workers}"
        );
    }
}

/// A store that holds every spool read for 40 ms (a job lasts at least that long, so jobs queued
/// behind it overlap with it) and records how many reads are inside at once. A plan reads spools
/// only in node jobs, so the peak is the number of node jobs that really ran together.
struct OverlapStore {
    inner: viewer_convert_core::pipeline::spool_store::MemSpoolStore,
    active: std::sync::atomic::AtomicUsize,
    peak: std::sync::atomic::AtomicUsize,
}

impl viewer_convert_core::pipeline::spool_store::SpoolStore for OverlapStore {
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
        use std::sync::atomic::Ordering::SeqCst;
        let now = self.active.fetch_add(1, SeqCst) + 1;
        self.peak.fetch_max(now, SeqCst);
        std::thread::sleep(std::time::Duration::from_millis(40));
        self.active.fetch_sub(1, SeqCst);
        self.inner.reader(id, extent)
    }
    fn delete(
        &self,
        id: viewer_convert_core::pipeline::spool_store::SpoolId,
    ) -> Result<(), ConvertError> {
        self.inner.delete(id)
    }
}

impl viewer_convert_core::pipeline::spool_store::SpoolAccounting for OverlapStore {
    fn next_event_seq(&self) -> Option<u64> {
        self.inner.next_event_seq()
    }
    fn ledger_totals(&self) -> Option<viewer_convert_core::pipeline::spool_store::LedgerTotals> {
        self.inner.ledger_totals()
    }
}

#[test]
fn hierarchy_jobs_run_on_more_threads_than_the_root_has_children() {
    use crate::las_source::open_las_reader;
    use viewer_convert_core::pipeline::spool_store::MemSpoolStore;
    use viewer_convert_core::source::inspect_source;
    let tmp = test_dir();
    let input = tmp.path().join("deep.las");
    deep_child_fixture(&input);
    let plan_with = |workers: usize| {
        let reader = open_las_reader(&input).unwrap();
        let source = inspect_source(&reader).unwrap();
        let store = OverlapStore {
            inner: MemSpoolStore::new(false),
            active: 0.into(),
            peak: 0.into(),
        };
        let plan = plan_from_reader(
            reader,
            &input,
            &source,
            cluster_options(workers),
            &store,
            &mut |_| true,
            false,
        )
        .unwrap();
        assert_eq!(plan.spatial_workers_used, workers);
        (plan, store.peak.load(std::sync::atomic::Ordering::SeqCst))
    };
    // The batch has one job per occupied root child at first; it is the plan that says how many.
    let (serial, _) = plan_with(1);
    let occupied_root_children = serial
        .metadata
        .dataset()
        .hierarchy()
        .child_ids(NodeId::new(0))
        .len();
    assert!(occupied_root_children > 1);
    let (_, peak) = plan_with(4);
    assert!(peak <= 4, "{peak} jobs overlapped on 4 workers");
    let (_, peak) = plan_with(16);
    assert!(
        peak > occupied_root_children,
        "{peak} jobs overlapped; the root has {occupied_root_children} children"
    );
}

#[test]
fn a_budget_that_cannot_hold_every_worker_lowers_the_effective_worker_count() {
    let tmp = test_dir();
    let input = tmp.path().join("deep.las");
    deep_child_fixture(&input);
    let serial_output = tmp.path().join("serial.oct");
    // One root range, so the serial peak is the plan's own and a budget equal to it is tight.
    let one_range = |options: ConvertOptions| options.with_root_partition_ranges(1).unwrap();
    let serial = convert_file(&input, &serial_output, one_range(cluster_options(1))).unwrap();
    let expected = read_pair(&serial_output);
    // Exactly the serial peak leaves no room for a second worker. Each extra worker needs 1 MiB
    // plus its 256 KiB of child spool buffers, so 9 MiB holds some but not all 16.
    for (extra_mib, workers) in [(0, 1..=1), (9, 2..=15)] {
        let budget = serial.estimated_peak_memory_bytes + extra_mib * 1024 * 1024;
        let output = tmp.path().join(format!("tight-{extra_mib}.oct"));
        let options = leaf_budget(16, 0.01, budget)
            .unwrap()
            .with_spatial_workers(16)
            .unwrap();
        let report = convert_file(&input, &output, one_range(options))
            .unwrap_or_else(|error| panic!("extra={extra_mib} MiB: {error:?}"));
        assert!(
            workers.contains(&report.spatial_workers_used),
            "extra={extra_mib} MiB: {} workers",
            report.spatial_workers_used
        );
        assert_eq!(read_pair(&output), expected, "extra={extra_mib} MiB");
    }
}

/// Opens (counting) what `LasFactory` opens: a plan that reads the input again was retried.
struct CountingFactory<'a> {
    inner: viewer_convert_core::pipeline::point_source::LasFactory<crate::las_source::FileBytes>,
    opens: &'a std::sync::atomic::AtomicUsize,
}

impl viewer_convert_core::pipeline::point_source::SourceFactory for CountingFactory<'_> {
    type Source = <viewer_convert_core::pipeline::point_source::LasFactory<
        crate::las_source::FileBytes,
    > as viewer_convert_core::pipeline::point_source::SourceFactory>::Source;
    fn split(
        &self,
        parts: usize,
        batch: u64,
    ) -> Result<Vec<viewer_convert_core::pipeline::point_source::PointRange>, ConvertError> {
        self.inner.split(parts, batch)
    }
    fn open(
        &self,
        range: viewer_convert_core::pipeline::point_source::PointRange,
    ) -> Result<Self::Source, ConvertError> {
        self.opens.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.open(range)
    }
}

/// Plans `input` with one root range (so only a retry reads it again) and returns the result
/// with the number of times the input was reopened.
fn plan_counting_reopens(
    input: &std::path::Path,
    options: ConvertOptions,
) -> (
    Result<viewer_convert_core::spatial::SpatialPlan, ConvertError>,
    usize,
) {
    use crate::las_source::open_las_reader;
    use viewer_convert_core::pipeline::spool_store::MemSpoolStore;
    use viewer_convert_core::source::inspect_source;
    let reader = open_las_reader(input).unwrap();
    let source = inspect_source(&reader).unwrap();
    let opens = std::sync::atomic::AtomicUsize::new(0);
    let factory = CountingFactory {
        inner: viewer_convert_core::pipeline::point_source::LasFactory::new(
            crate::las_source::FileBytes {
                path: input.to_owned(),
                len: std::fs::metadata(input).unwrap().len(),
            },
        ),
        opens: &opens,
    };
    let result = viewer_convert_core::spatial::build_spatial_plan_from_source(
        &mut viewer_convert_core::pipeline::point_source::RangeSource::Las(
            viewer_convert_core::pipeline::point_source::LasPointSource::full(reader),
        ),
        &factory,
        &source,
        options.with_root_partition_ranges(0).unwrap(),
        &MemSpoolStore::new(false),
        &crate::pipeline::SystemClock,
        &crate::executor::ThreadedExecutor,
        &mut |_| true,
        false,
    );
    (result, opens.load(std::sync::atomic::Ordering::SeqCst))
}

/// A batch that outgrows its share of a budget one worker would pass is planned again with one
/// worker: same plan, one worker reported.
#[test]
fn a_batch_over_the_budget_falls_back_to_one_worker() {
    let tmp = test_dir();
    let input = tmp.path().join("deep.las");
    deep_child_fixture(&input);
    let serial = plan_counting_reopens(&input, cluster_options(1)).0.unwrap();
    let mut retried = 0;
    // Above the serial peak (in 16 KiB steps) some budgets admit a batch that then overflows;
    // measured: steps 142-172 of this fixture, so a coarser scan across them finds some.
    for step in (100..260u64).step_by(8) {
        let budget = serial.estimated_peak_memory_bytes + step * 16 * 1024;
        let options = leaf_budget(16, 0.01, budget)
            .unwrap()
            .with_spatial_workers(4)
            .unwrap();
        let (plan, reopens) = plan_counting_reopens(&input, options);
        let plan = plan.unwrap_or_else(|error| panic!("budget {budget}: {error:?}"));
        if reopens > 0 {
            retried += 1;
            assert_eq!(plan.spatial_workers_used, 1, "budget {budget}");
            assert_eq!(plan.blocks.len(), serial.blocks.len(), "budget {budget}");
            assert_eq!(plan.metadata, serial.metadata, "budget {budget}");
        }
    }
    assert!(retried > 0, "no budget made the batch overflow");
}

#[test]
fn spatial_worker_caps_are_a_sanity_bound_and_zero_is_rejected() {
    assert_eq!(
        ConvertOptions::default()
            .with_spatial_workers(16)
            .unwrap()
            .spatial_workers(),
        16
    );
    for invalid in [0, MAX_SPATIAL_WORKERS + 1] {
        assert!(matches!(
            ConvertOptions::default().with_spatial_workers(invalid),
            Err(ConvertError::InvalidSpatialWorkerCount(count)) if count == invalid
        ));
    }
}
