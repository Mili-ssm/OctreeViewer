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
    assert_eq!(
        parallel.estimated_peak_memory_bytes,
        // Worker overhead plus the 8 child spool buffers (32 KiB) of V8's two divisions; the V5
        // value was 2 MiB (re-pinned for V8 in D4 Task 5c).
        serial.estimated_peak_memory_bytes + 1024 * 1024 + 8 * 32 * 1024
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
        ConvertOptions::default().with_spatial_workers(9),
        Err(ConvertError::InvalidSpatialWorkerCount(9))
    ));
    assert_eq!(
        ConvertOptions::default()
            .with_spatial_workers(MAX_SPATIAL_WORKERS)
            .unwrap()
            .spatial_workers(),
        MAX_SPATIAL_WORKERS
    );
    assert!(matches!(
        convert_file(&input, &output, options),
        Err(ConvertError::MemoryBudgetExceeded { .. })
    ));
    assert!(!output.exists());
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
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
