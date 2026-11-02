use crate::publish::{
    OutputValidation, output_attributes_path, output_block_ids, validate_output_pair,
    verify_candidate_visual_scale, visual_scale_for_bounds,
};
use std::{
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::PathBuf,
};

use las::{Builder, Color, Point, Writer, point::Format};
use viewer_core::{AttributeValues, BlockId, DatasetSource, NodeId, SpatialBounds};
use viewer_format::{FormatReader, OctreeReader};

use super::{
    CheckpointError, CheckpointMode, ConversionStage, ConvertError, ConvertOptions, convert_file,
    convert_file_with_checkpoint, convert_file_with_diagnostics, convert_file_with_progress,
};
use crate::options::{DEFAULT_SPATIAL_WORKERS, MAX_SPATIAL_WORKERS};

#[test]
fn visual_scale_normalizes_bounds_without_changing_coordinates() {
    let bounds = SpatialBounds::new([10.0, -4.0, 0.0], [20.0, 16.0, 2.0]).unwrap();
    assert_eq!(visual_scale_for_bounds(bounds).unwrap(), 5.0);
    assert_eq!(
        visual_scale_for_bounds(SpatialBounds::new([0.0; 3], [0.0; 3]).unwrap()).unwrap(),
        1.0
    );
    assert!(matches!(
        visual_scale_for_bounds(
            SpatialBounds::new([0.0; 3], [f64::MIN_POSITIVE, 0.0, 0.0]).unwrap()
        ),
        Err(ConvertError::InvalidVisualScale(_))
    ));
}

#[test]
fn rejects_checkpoint_candidates_with_a_stale_visual_scale() {
    let tmp = test_dir();
    let dir = tmp.path().to_owned();
    let input = dir.join("source.las");
    let output = dir.join("result.oct");
    fixture(&input, false, false, 3);
    convert_file(&input, &output, ConvertOptions::new(2, 0.01).unwrap()).unwrap();

    assert!(matches!(
        verify_candidate_visual_scale(&output, 25.0),
        Err(ConvertError::StaleVisualScaleCheckpoint { .. })
    ));
}

fn test_dir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn fixture(path: &std::path::Path, compressed: bool, colored: bool, count: usize) {
    let mut builder = Builder::from((1, 2));
    builder.point_format = Format::new(if colored { 2 } else { 0 }).unwrap();
    builder.point_format.is_compressed = compressed;
    let header = builder.into_header().unwrap();
    let mut writer = Writer::from_path(path, header).unwrap();
    for index in 0..count {
        let mut point = Point {
            x: 1_000_000.0 + index as f64 * 0.25,
            y: 2_000_000.0 + (index % 3) as f64,
            z: 50.0 + (index % 5) as f64,
            intensity: (index as u16).wrapping_mul(10_000),
            return_number: 2,
            number_of_returns: 3,
            classification: las::point::Classification::Ground,
            scan_angle: index as f32 - 1.0,
            user_data: (index as u8).wrapping_add(7),
            point_source_id: 100_u16.wrapping_add(index as u16),
            ..Point::default()
        };
        if colored {
            point.color = Some(Color::new(0x1200, 0xAB00, 0xFF00));
        }
        writer.write_point(point).unwrap();
    }
}

#[test]
fn diagnostics_report_internal_windows_without_changing_output_bytes() {
    let tmp = test_dir();
    let dir = tmp.path().to_owned();
    let input = dir.join("source.las");
    let baseline = dir.join("baseline.oct");
    let profiled = dir.join("profiled.oct");
    fixture(&input, false, true, 24);

    convert_file(&input, &baseline, ConvertOptions::default()).unwrap();
    let (_, diagnostics) =
        convert_file_with_diagnostics(&input, &profiled, ConvertOptions::default(), |_| true)
            .unwrap();

    assert_eq!(fs::read(&baseline).unwrap(), fs::read(&profiled).unwrap());
    assert_eq!(
        fs::read(output_attributes_path(&baseline)).unwrap(),
        fs::read(output_attributes_path(&profiled)).unwrap()
    );
    for window in [
        diagnostics.spatial_planning,
        diagnostics.block_stream,
        diagnostics.output_validation,
    ] {
        let window = window.expect("invoked conversion phase must have a diagnostic window");
        assert!(window.elapsed_ns > 0);
        assert!(
            window
                .start_unix_ns
                .zip(window.end_unix_ns)
                .is_some_and(|(start, end)| end >= start)
        );
    }
    assert!(diagnostics.block_preparation_elapsed_ns.is_some());
    assert!(diagnostics.ordered_write_and_release_elapsed_ns.is_some());
    assert!(diagnostics.finalize_and_sync_elapsed_ns.is_some());
    let spools = diagnostics
        .spatial_spools
        .as_ref()
        .expect("profiled planning reports spool payload counters");
    assert_eq!(spools.source_payload_written_bytes, 24 * 33);
    assert_eq!(spools.source_payload_scanned_bytes, 0);
    assert_eq!(spools.child_payload_written_bytes, 0);
    assert_eq!(spools.child_payload_scanned_bytes, 0);
    assert_eq!(spools.lod_payload_written_bytes, 0);
    assert_eq!(
        diagnostics.block_stream_source_payload_read_bytes,
        Some(24 * 33)
    );
    assert_eq!(diagnostics.block_stream_lod_payload_read_bytes, Some(0));
    assert_eq!(diagnostics.block_stream_max_prepared_blocks, Some(1));
    assert!(
        diagnostics
            .block_stream_max_encoded_payload_capacity_per_block_bytes
            .is_some_and(|bytes| bytes > 0)
    );
    assert_eq!(
        diagnostics.block_stream_max_encoded_payload_capacity_per_batch_bytes,
        diagnostics.block_stream_max_encoded_payload_capacity_per_block_bytes
    );
    let validation_workers = diagnostics
        .validation_workers
        .expect("profiled validation reports worker aggregates");
    assert!(!validation_workers.is_empty());
    assert!(validation_workers.iter().all(|worker| {
        worker.blocks_read > 0
            && worker.points_read > 0
            && worker.read_elapsed_ns > 0
            && worker.max_decoded_block_estimated_bytes > 0
    }));
}

#[test]
fn v7_codec_diagnostics_preserve_output_and_separate_winner_from_attempts() {
    for version in [
        viewer_format::FormatVersion::V7_PRE1,
        viewer_format::FormatVersion::V7_PRE2,
        viewer_format::FormatVersion::V7_PRE3,
    ] {
        let tmp = test_dir();
        let dir = tmp.path().to_owned();
        let input = dir.join("source.las");
        let baseline = dir.join("baseline.oct");
        let profiled = dir.join("profiled.oct");
        fixture(&input, false, true, 24);
        let options = ConvertOptions::new(8, 0.01)
            .unwrap()
            .with_format_version(version)
            .unwrap();
        convert_file(&input, &baseline, options).unwrap();
        let (_, diagnostics) =
            convert_file_with_diagnostics(&input, &profiled, options, |_| true).unwrap();
        assert_eq!(fs::read(&baseline).unwrap(), fs::read(&profiled).unwrap());
        assert_eq!(
            fs::read(output_attributes_path(&baseline)).unwrap(),
            fs::read(output_attributes_path(&profiled)).unwrap()
        );
        let candidates = diagnostics
            .geometry_candidates
            .expect("codec candidate rows");
        assert!(candidates.iter().any(|candidate| candidate.attempts > 0));
        let winners: u64 = candidates
            .iter()
            .map(|candidate| candidate.selected_winners)
            .sum();
        let attempts: u64 = candidates.iter().map(|candidate| candidate.attempts).sum();
        assert!(winners > 0);
        assert!(attempts > winners);
        assert!(diagnostics.occupancy_builds.is_some());
    }
}

#[test]
fn diagnostics_count_partition_and_lod_spool_payloads() {
    let tmp = test_dir();
    let dir = tmp.path().to_owned();
    let input = dir.join("source.las");
    let serial_output = dir.join("serial.oct");
    let parallel_output = dir.join("parallel.oct");
    fixture(&input, false, false, 200);
    let options = ConvertOptions::new(16, 0.01)
        .unwrap()
        .with_spatial_workers(1)
        .unwrap()
        .with_spatial_divisions(2)
        .unwrap();

    let (_, serial_diagnostics) =
        convert_file_with_diagnostics(&input, &serial_output, options, |_| true).unwrap();
    let (_, parallel_diagnostics) = convert_file_with_diagnostics(
        &input,
        &parallel_output,
        options.with_spatial_workers(2).unwrap(),
        |_| true,
    )
    .unwrap();

    assert_eq!(
        fs::read(&serial_output).unwrap(),
        fs::read(&parallel_output).unwrap()
    );
    assert_eq!(
        fs::read(output_attributes_path(&serial_output)).unwrap(),
        fs::read(output_attributes_path(&parallel_output)).unwrap()
    );
    let spools = serial_diagnostics.spatial_spools.unwrap();
    let parallel_spools = parallel_diagnostics.spatial_spools.unwrap();
    assert_eq!(spools, parallel_spools);
    assert_eq!(
        serial_diagnostics.block_stream_source_payload_read_bytes,
        Some(200 * 27)
    );
    assert_eq!(
        serial_diagnostics.block_stream_lod_payload_read_bytes,
        Some(spools.lod_payload_written_bytes)
    );
    assert_eq!(
        serial_diagnostics.block_stream_source_payload_read_bytes,
        parallel_diagnostics.block_stream_source_payload_read_bytes
    );
    assert_eq!(
        serial_diagnostics.block_stream_lod_payload_read_bytes,
        parallel_diagnostics.block_stream_lod_payload_read_bytes
    );
    assert!(
        serial_diagnostics
            .block_stream_max_encoded_payload_capacity_per_batch_bytes
            .is_some_and(|bytes| bytes > 0)
    );
    assert!(
        parallel_diagnostics
            .block_stream_max_prepared_blocks
            .is_some_and(|blocks| blocks <= 2)
    );
    assert_eq!(spools.source_payload_written_bytes, 0);
    assert!(spools.child_payload_written_bytes >= 200 * 27);
    assert!(spools.child_payload_scanned_bytes > 0);
    assert!(spools.lod_payload_written_bytes > 0);
    let phases = serial_diagnostics
        .spatial_phases
        .as_ref()
        .expect("profiled planning reports spatial phase timings");
    assert!(phases.root_las_fill_elapsed_ns > 0);
    assert!(phases.spool_partition_and_sample_elapsed_ns > 0);
    assert!(phases.spool_scan_read_elapsed_ns > 0);
    assert!(phases.lod_sample_write_elapsed_ns > 0);
}

fn coincident_fixture(path: &std::path::Path) {
    let header = Builder::from((1, 2)).into_header().unwrap();
    let mut writer = Writer::from_path(path, header).unwrap();
    for intensity in [1, 2, 3, 4, 5] {
        writer
            .write_point(Point {
                x: 10.0,
                y: 20.0,
                z: 30.0,
                intensity,
                ..Point::default()
            })
            .unwrap();
    }
}

fn spool_file_count(directory: &std::path::Path) -> usize {
    fs::read_dir(directory)
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            if path.is_dir() {
                spool_file_count(&path)
            } else {
                usize::from(matches!(
                    path.extension().and_then(|extension| extension.to_str()),
                    Some("spool" | "lod")
                ))
            }
        })
        .sum()
}

fn extended_fixture(path: &std::path::Path) {
    let mut builder = Builder::from((1, 4));
    builder.point_format = Format::new(8).unwrap();
    let header = builder.into_header().unwrap();
    let mut writer = Writer::from_path(path, header).unwrap();
    writer
        .write_point(Point {
            x: 10.0,
            y: 20.0,
            z: 30.0,
            intensity: 1234,
            return_number: 2,
            number_of_returns: 3,
            classification: las::point::Classification::Ground,
            scan_angle: -1.25,
            user_data: 9,
            point_source_id: 77,
            gps_time: Some(123.5),
            color: Some(Color::new(0x1200, 0xAB00, 0xFF00)),
            nir: Some(0xCAFE),
            ..Point::default()
        })
        .unwrap();
}

#[test]
fn converts_laz_in_bounded_blocks_and_preserves_intensity_and_rgb16() {
    let tmp = test_dir();
    let dir = tmp.path().to_owned();
    let input = dir.join("source.laz");
    let output = dir.join("result.oct");
    fixture(&input, true, true, 5);
    let mut progress = Vec::new();
    let report = convert_file_with_progress(
        &input,
        &output,
        ConvertOptions::new(2, 0.01).unwrap(),
        |event| {
            progress.push(event);
            true
        },
    )
    .unwrap();
    assert_eq!(report.point_count, 5);
    assert!(report.block_count >= 4);
    assert!(report.estimated_peak_memory_bytes <= 512 * 1024 * 1024);
    assert!(
        !report
            .omitted_dimensions
            .contains(&"classification".to_owned())
    );
    assert!(
        report
            .omitted_dimensions
            .iter()
            .any(|name| name.contains("flags"))
    );
    assert_eq!(
        progress.first().unwrap().stage,
        ConversionStage::InspectingInput
    );
    assert!(progress.iter().any(|event| {
        event.stage == ConversionStage::ReadingAndWritingBlocks
            && event.points_processed == report.point_count
            && event.bytes_written > 24
    }));
    let completed = progress.last().unwrap();
    assert_eq!(completed.stage, ConversionStage::Complete);
    assert_eq!(completed.bytes_written, report.output_bytes);

    let mut reader = FormatReader::new(fs::File::open(&output).unwrap()).unwrap();
    reader
        .attach_attributes(fs::File::open(output_attributes_path(&output)).unwrap())
        .unwrap();
    assert_eq!(reader.format_metadata().visual_scale(), 25.0);
    let (root_block_ids, root_error, node_count) = {
        let hierarchy = reader.metadata().hierarchy();
        let root = hierarchy.node(NodeId::new(0)).unwrap();
        (
            root.blocks().to_vec(),
            root.geometric_error(),
            hierarchy.stats().node_count,
        )
    };
    assert_eq!(root_block_ids.len(), 1);
    assert!(root_error > 0.0);
    assert!(node_count > 1);
    let root_sample = reader.read_block(root_block_ids[0]).unwrap();
    assert!(root_sample.points().len() <= 512);
    for index in 0..5 {
        let source_position = [index as f64 * 0.25, (index % 3) as f64, (index % 5) as f64];
        let nearest_distance = root_sample
            .points()
            .points()
            .iter()
            .map(|sample| {
                (0..3)
                    .map(|axis| (source_position[axis] - f64::from(sample.position[axis])).powi(2))
                    .sum::<f64>()
                    .sqrt()
            })
            .fold(f64::INFINITY, f64::min);
        assert!(nearest_distance <= root_error);
    }
    let mut all_source_ids = std::collections::BTreeSet::new();
    let mut source_values = [false; 5];
    for block_number in 0..report.block_count {
        let block_id = BlockId::new(block_number);
        let block = reader.read_block(block_id).unwrap();
        let intensity = block.attributes()[0].values();
        let AttributeValues::U16(values) = intensity else {
            panic!("intensity must remain u16");
        };
        let AttributeValues::U8(classification) = block.attributes()[1].values() else {
            panic!("classification must remain u8");
        };
        assert!(classification.iter().all(|value| *value == 2));
        let AttributeValues::U8(return_numbers) = block.attributes()[2].values() else {
            panic!("return number must remain u8");
        };
        assert!(return_numbers.iter().all(|value| *value == 2));
        let AttributeValues::U8(number_of_returns) = block.attributes()[3].values() else {
            panic!("number of returns must remain u8");
        };
        assert!(number_of_returns.iter().all(|value| *value == 3));
        let AttributeValues::U8(user_data) = block.attributes()[5].values() else {
            panic!("user data must remain u8");
        };
        let AttributeValues::U16(source_ids) = block.attributes()[6].values() else {
            panic!("point source ID must remain u16");
        };
        for ((source_id, intensity), user_data) in source_ids.iter().zip(values).zip(user_data) {
            all_source_ids.insert(*source_id);
            let index = usize::from(*source_id - 100);
            if index < source_values.len()
                && *intensity == index as u16 * 10_000
                && *user_data == index as u8 + 7
            {
                source_values[index] = true;
            }
        }
        let AttributeValues::U16(rgb) = block.attributes()[7].values() else {
            panic!("RGB must remain u16");
        };
        let (colors, remainder) = rgb.as_chunks::<3>();
        assert!(remainder.is_empty());
        assert!(
            colors
                .iter()
                .all(|color| color == &[0x1200, 0xAB00, 0xFF00])
        );
        assert!(
            block
                .points()
                .points()
                .iter()
                .all(|point| point.color == viewer_core::ColorRgb::new(0x12, 0xAB, 0xFF))
        );
    }
    assert_eq!(all_source_ids, (100..105).collect());
    assert!(source_values.into_iter().all(|found| found));
}

#[test]
fn converts_las_without_rgb_and_uses_intensity_for_display_color() {
    let tmp = test_dir();
    let dir = tmp.path().to_owned();
    let input = dir.join("source.las");
    let output = dir.join("result.oct");
    fixture(&input, false, false, 3);
    let report = convert_file(&input, &output, ConvertOptions::new(2, 0.01).unwrap()).unwrap();

    let mut reader = FormatReader::new(fs::File::open(&output).unwrap()).unwrap();
    reader
        .attach_attributes(fs::File::open(output_attributes_path(&output)).unwrap())
        .unwrap();
    let mut found_last_point = false;
    for block_number in 0..report.block_count {
        let block = reader.read_block(BlockId::new(block_number)).unwrap();
        let AttributeValues::U16(intensity) = block.attributes()[0].values() else {
            panic!("intensity must remain u16");
        };
        if intensity.contains(&20_000)
            && block
                .points()
                .points()
                .iter()
                .any(|point| point.color == viewer_core::ColorRgb::new(78, 78, 78))
        {
            found_last_point = true;
        }
    }
    assert!(found_last_point);
}

#[test]
fn bounded_spatial_workers_preserve_the_serial_output() {
    let tmp = test_dir();
    let dir = tmp.path().to_owned();
    let input = dir.join("source.las");
    let serial_output = dir.join("serial.oct");
    let parallel_output = dir.join("parallel.oct");
    fixture(&input, false, true, 200);
    let serial_options = ConvertOptions::new(16, 0.01)
        .unwrap()
        .with_spatial_workers(1)
        .unwrap()
        .with_spatial_divisions(4)
        .unwrap()
        .with_root_partition_ranges(0)
        .unwrap();
    let parallel_options = serial_options.with_spatial_workers(2).unwrap();

    let serial = convert_file(&input, &serial_output, serial_options).unwrap();
    let parallel = convert_file(&input, &parallel_output, parallel_options).unwrap();

    assert_eq!(serial.block_preparation.unwrap().worker_limit, 1);
    assert_eq!(serial.output_validation.worker_limit, 1);
    assert_eq!(parallel.block_preparation.unwrap().worker_limit, 2);
    assert_eq!(parallel.output_validation.worker_limit, 2);
    assert!(
        parallel
            .block_preparation
            .unwrap()
            .estimated_bytes_per_worker
            > 0
    );
    assert!(parallel.output_validation.estimated_bytes_per_worker > 0);
    let attributes_path = output_attributes_path(&parallel_output);
    let (block_ids, source_block_ids) = output_block_ids(&parallel_output).unwrap();
    let limited = validate_output_pair(
        &parallel_output,
        Some(&attributes_path),
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
        serial.estimated_peak_memory_bytes + 1024 * 1024 + 2 * 1024 * 1024
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

fn output_pair_bytes(output: &std::path::Path) -> (Vec<u8>, Vec<u8>) {
    (
        fs::read(output).unwrap(),
        fs::read(output_attributes_path(output)).unwrap(),
    )
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
fn spatial_workers_preserve_output_across_counts_divisions_and_compression() {
    let tmp = test_dir();
    let dir = tmp.path().to_owned();
    for compressed in [false, true] {
        let input = dir.join(if compressed {
            "source.laz"
        } else {
            "source.las"
        });
        fixture(&input, compressed, true, 4_000);
        for divisions in [2, 4, 8] {
            let options = ConvertOptions::new(64, 0.01)
                .unwrap()
                .with_spatial_divisions(divisions)
                .unwrap();
            let case = format!("{compressed}-{divisions}");
            let reference =
                convert_with_workers(&input, &dir.join(format!("ref-{case}.oct")), options, 1);
            for workers in [2, 4, 8] {
                let output = dir.join(format!("w{workers}-{case}.oct"));
                assert_eq!(
                    convert_with_workers(&input, &output, options, workers),
                    reference,
                    "compressed={compressed} divisions={divisions} workers={workers}"
                );
            }
        }
    }
}

#[test]
fn coincident_root_output_is_independent_of_worker_count() {
    let tmp = test_dir();
    let dir = tmp.path().to_owned();
    let input = dir.join("coincident.las");
    coincident_fixture(&input);
    let options = ConvertOptions::new(64, 0.01)
        .unwrap()
        .with_spatial_divisions(8)
        .unwrap();
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
    let options = ConvertOptions::new(8, 0.01)
        .unwrap()
        .with_spatial_workers(1)
        .unwrap()
        .with_format_version(viewer_format::FormatVersion::V6_PRE4)
        .unwrap();
    let serial = convert_file(&input, &serial_output, options).unwrap();
    let tight = ConvertOptions::with_memory_budget(8, 0.01, serial.estimated_peak_memory_bytes)
        .unwrap()
        .with_spatial_workers(1)
        .unwrap()
        .with_format_version(viewer_format::FormatVersion::V6_PRE4)
        .unwrap()
        .with_pipelined_writes(true);
    let report = convert_file(&input, &output, tight).unwrap();
    assert_eq!(report.block_preparation.unwrap().worker_limit, 1);
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
    let options = ConvertOptions::new(8, 0.01)
        .unwrap()
        .with_format_version(viewer_format::FormatVersion::V6_PRE4)
        .unwrap()
        .with_pipelined_writes(true);
    let result = convert_file_with_progress(&input, &output, options, |event| {
        event.stage != ConversionStage::ReadingAndWritingBlocks || event.points_processed == 0
    });
    assert!(matches!(result, Err(ConvertError::Cancelled { .. })));
    assert!(!output.exists());
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
}

#[test]
fn v6_serial_and_parallel_conversion_outputs_are_identical_and_readable() {
    for version in [
        viewer_format::FormatVersion::V6_PRE1,
        viewer_format::FormatVersion::V6_PRE2,
        viewer_format::FormatVersion::V6_PRE3,
        viewer_format::FormatVersion::V6_PRE4,
        viewer_format::FormatVersion::V7_PRE1,
        viewer_format::FormatVersion::V7_PRE2,
        viewer_format::FormatVersion::V7_PRE3,
    ] {
        let tmp = test_dir();
        let dir = tmp.path().to_owned();
        let input = dir.join("source.las");
        let serial_output = dir.join("serial-v6.oct");
        let parallel_output = dir.join("parallel-v6.oct");
        fixture(&input, false, true, 24);
        let serial_options = ConvertOptions::new(8, 0.01)
            .unwrap()
            .with_format_version(version)
            .unwrap()
            .with_spatial_workers(1)
            .unwrap()
            .with_spatial_divisions(4)
            .unwrap();
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
        assert_eq!(reader.format_version(), version);
        reader
            .attach_attributes(fs::File::open(output_attributes_path(&parallel_output)).unwrap())
            .unwrap();
        assert_eq!(
            reader.metadata().hierarchy().stats().source_point_count,
            Some(24)
        );
        for entry in reader.block_storage_stats().unwrap() {
            let block = reader.read_block(entry.id).unwrap();
            assert_eq!(block.attributes().len(), 8);
            let AttributeValues::U16(intensity) = block.attributes()[0].values() else {
                panic!("intensity must remain u16");
            };
            assert_eq!(intensity.len(), block.points().len());
            let AttributeValues::U16(rgb) = block.attributes()[7].values() else {
                panic!("RGB must remain u16");
            };
            let (colors, remainder) = rgb.as_chunks::<3>();
            assert!(remainder.is_empty());
            assert!(
                colors
                    .iter()
                    .all(|color| color == &[0x1200, 0xAB00, 0xFF00])
            );
        }
        assert_eq!(serial.point_count, parallel.point_count);
        assert_eq!(serial.block_count, parallel.block_count);
    }
}

#[test]
fn parallel_worker_limits_are_validated_and_budget_is_respected() {
    let tmp = test_dir();
    let dir = tmp.path().to_owned();
    let input = dir.join("source.las");
    let output = dir.join("parallel.oct");
    fixture(&input, false, false, 100);
    let options = ConvertOptions::with_memory_budget(10, 0.01, 17 * 1024 * 1024)
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
        ConvertOptions::default().with_spatial_divisions(3),
        Err(ConvertError::InvalidSpatialDivisions(3))
    ));
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
        ConvertOptions::with_memory_budget(10, 0.01, 32 * 1024 * 1024)
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
    let options = ConvertOptions::new(10, 0.01)
        .unwrap()
        .with_spatial_workers(2)
        .unwrap();
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
fn preserves_optional_gps_time_rgb_and_nir_dimensions() {
    let tmp = test_dir();
    let dir = tmp.path().to_owned();
    let input = dir.join("extended.las");
    let output = dir.join("result.oct");
    extended_fixture(&input);
    let report = convert_file(&input, &output, ConvertOptions::default()).unwrap();
    assert!(
        report
            .omitted_dimensions
            .iter()
            .any(|name| name == "scanner channel")
    );

    let mut reader = FormatReader::new(fs::File::open(&output).unwrap()).unwrap();
    reader
        .attach_attributes(fs::File::open(output_attributes_path(&output)).unwrap())
        .unwrap();
    let block = reader.read_block(BlockId::new(0)).unwrap();
    let value = |name: &str| {
        block
            .attributes()
            .iter()
            .find(|column| column.descriptor().name() == name)
            .unwrap()
            .values()
    };
    let AttributeValues::F64(gps) = value("gps_time") else {
        panic!("GPS time must remain f64");
    };
    assert_eq!(gps, &[123.5]);
    let AttributeValues::U16(nir) = value("nir") else {
        panic!("NIR must remain u16");
    };
    assert_eq!(nir, &[0xCAFE]);
    let AttributeValues::U16(rgb) = value("rgb") else {
        panic!("RGB must remain u16");
    };
    assert_eq!(rgb, &[0x1200, 0xAB00, 0xFF00]);
}

#[test]
fn keeps_existing_outputs_unchanged_and_cleans_temporary_files_on_failure() {
    let tmp = test_dir();
    let dir = tmp.path().to_owned();
    let input = dir.join("source.las");
    let output = dir.join("result.oct");
    fixture(&input, false, false, 3);
    fs::write(&output, b"existing").unwrap();
    assert!(matches!(
        convert_file(&input, &output, ConvertOptions::default()),
        Err(ConvertError::OutputExists(_))
    ));
    assert_eq!(fs::read(&output).unwrap(), b"existing");

    fs::remove_file(&output).unwrap();
    let mut truncated = fs::read(&input).unwrap();
    truncated.pop();
    fs::write(&input, truncated).unwrap();
    assert!(convert_file(&input, &output, ConvertOptions::default()).is_err());
    assert!(!output.exists());
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
}

#[test]
fn memory_budget_rejects_the_plan_before_creating_an_output() {
    let tmp = test_dir();
    let dir = tmp.path().to_owned();
    let input = dir.join("source.las");
    let output = dir.join("result.oct");
    fixture(&input, false, false, 3);
    let options = ConvertOptions::with_memory_budget(2, 0.01, 1).unwrap();

    assert!(matches!(
        convert_file(&input, &output, options),
        Err(ConvertError::MemoryBudgetExceeded { limit: 1, .. })
    ));
    assert!(!output.exists());
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
}

#[test]
fn identical_positions_remain_full_resolution_without_deep_empty_splits() {
    let tmp = test_dir();
    let dir = tmp.path().to_owned();
    let input = dir.join("identical.las");
    let output = dir.join("result.oct");
    coincident_fixture(&input);

    let report = convert_file(&input, &output, ConvertOptions::new(2, 0.01).unwrap()).unwrap();
    assert_eq!(report.point_count, 5);
    assert_eq!(report.block_count, 3);
    let mut reader = FormatReader::new(fs::File::open(&output).unwrap()).unwrap();
    reader
        .attach_attributes(fs::File::open(output_attributes_path(&output)).unwrap())
        .unwrap();
    let root_blocks = {
        let hierarchy = reader.metadata().hierarchy();
        assert_eq!(hierarchy.stats().node_count, 1);
        let root = hierarchy.node(NodeId::new(0)).unwrap();
        assert_eq!(root.point_count(), 5);
        root.blocks().to_vec()
    };
    assert_eq!(root_blocks.len(), 3);
    let mut intensities = Vec::new();
    for block_id in root_blocks {
        let block = reader.read_block(block_id).unwrap();
        let AttributeValues::U16(values) = block.attributes()[0].values() else {
            panic!("intensity must remain u16");
        };
        intensities.extend_from_slice(values);
    }
    intensities.sort_unstable();
    assert_eq!(intensities, [1, 2, 3, 4, 5]);
}

#[test]
fn shared_leaf_and_lod_spools_are_released_before_validation() {
    for shared_spool in [true, false] {
        for workers in [1, 2, 3] {
            let tmp = test_dir();
            let dir = tmp.path().to_owned();
            let input = dir.join("source.las");
            let output = dir.join("result.oct");
            if shared_spool {
                coincident_fixture(&input);
            } else {
                fixture(&input, false, true, 5);
            }
            let options = ConvertOptions::new(2, 0.01)
                .unwrap()
                .with_spatial_workers(workers)
                .unwrap();
            let mut validated = false;
            convert_file_with_progress(&input, &output, options, |progress| {
                if shared_spool && progress.stage == ConversionStage::ReadingAndWritingBlocks {
                    assert_eq!(
                        spool_file_count(&dir),
                        usize::from(progress.points_processed < 5),
                        "a shared spool must survive until its final range is written"
                    );
                }
                if progress.stage == ConversionStage::ValidatingOutput {
                    validated = true;
                    assert_eq!(spool_file_count(&dir), 0);
                }
                true
            })
            .unwrap();
            assert!(validated);
        }
    }
}

#[test]
fn lod_voxels_average_color_and_copy_other_attributes_from_nearest_point() {
    let tmp = test_dir();
    let dir = tmp.path().to_owned();
    let input = dir.join("colors.las");
    let output = dir.join("result.oct");
    let mut builder = Builder::from((1, 2));
    builder.point_format = Format::new(2).unwrap();
    let mut writer = Writer::from_path(&input, builder.into_header().unwrap()).unwrap();
    for (position, intensity, source_id, color) in [
        ([0.05, 0.05, 0.05], 10, 11, Color::new(0, 0, 0)),
        (
            [0.06, 0.06, 0.06],
            20,
            22,
            Color::new(u16::MAX, u16::MAX, u16::MAX),
        ),
        ([1.0, 1.0, 1.0], 30, 33, Color::new(0, 0, 0)),
    ] {
        writer
            .write_point(Point {
                x: position[0],
                y: position[1],
                z: position[2],
                intensity,
                point_source_id: source_id,
                color: Some(color),
                ..Point::default()
            })
            .unwrap();
    }
    drop(writer);

    convert_file(&input, &output, ConvertOptions::new(1, 0.01).unwrap()).unwrap();
    let mut reader = FormatReader::new(fs::File::open(&output).unwrap()).unwrap();
    reader
        .attach_attributes(fs::File::open(output_attributes_path(&output)).unwrap())
        .unwrap();
    let (root_block_id, max_level) = {
        let hierarchy = reader.metadata().hierarchy();
        (
            hierarchy.node(NodeId::new(0)).unwrap().blocks()[0],
            hierarchy.stats().max_level,
        )
    };
    assert!(max_level > 1);
    let root_sample = reader.read_block(root_block_id).unwrap();
    let averaged_sample = root_sample
        .points()
        .points()
        .iter()
        .position(|point| point.color == viewer_core::ColorRgb::new(128, 128, 128))
        .expect("the two clustered source colors are averaged");
    let AttributeValues::U16(intensity) = root_sample.attributes()[0].values() else {
        panic!("intensity must remain u16");
    };
    assert_eq!(intensity[averaged_sample], 20);
    let AttributeValues::U16(source_ids) = root_sample.attributes()[6].values() else {
        panic!("point source ID must remain u16");
    };
    assert_eq!(source_ids[averaged_sample], 22);
    let AttributeValues::U16(rgb) = root_sample.attributes()[7].values() else {
        panic!("RGB must remain u16");
    };
    let rgb = &rgb[averaged_sample * 3..averaged_sample * 3 + 3];
    assert_eq!(rgb, &[0x8000; 3]);
}

#[test]
fn cancellation_removes_the_temporary_file_without_publishing_output() {
    let tmp = test_dir();
    let dir = tmp.path().to_owned();
    let input = dir.join("source.las");
    let output = dir.join("result.oct");
    fixture(&input, false, false, 5);
    let mut cancelled = false;
    let result = convert_file_with_progress(
        &input,
        &output,
        ConvertOptions::new(2, 0.01).unwrap(),
        |event| {
            if event.stage == ConversionStage::ReadingAndWritingBlocks
                && event.points_processed >= 2
            {
                cancelled = true;
                false
            } else {
                true
            }
        },
    );

    assert!(cancelled);
    assert!(matches!(
        result,
        Err(ConvertError::Cancelled {
            points_processed: 2
        })
    ));
    assert!(!output.exists());
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);

    let rebuilt = convert_file(&input, &output, ConvertOptions::new(2, 0.01).unwrap())
        .expect("a cancelled conversion can be rebuilt from its source");
    assert_eq!(rebuilt.point_count, 5);
    assert!(output.exists());
}

#[test]
fn resumes_from_the_completed_source_spool_and_rejects_changed_options() {
    let tmp = test_dir();
    let dir = tmp.path().to_owned();
    let input = dir.join("source.las");
    let output = dir.join("result.oct");
    let expected = dir.join("expected.oct");
    let mut checkpoint_dir = output.as_os_str().to_owned();
    checkpoint_dir.push(".checkpoint");
    let checkpoint_dir = PathBuf::from(checkpoint_dir);
    coincident_fixture(&input);
    let options = ConvertOptions::new(2, 0.01)
        .unwrap()
        .with_spatial_workers(2)
        .unwrap();

    let mut checkpoint_source = None;
    let cancelled =
        convert_file_with_checkpoint(&input, &output, options, CheckpointMode::Create, |event| {
            if event.stage == ConversionStage::ReadingAndWritingBlocks {
                checkpoint_source
                    .get_or_insert_with(|| fs::read(checkpoint_dir.join("source.spool")).unwrap());
                if event.points_processed == 5 {
                    assert_eq!(spool_file_count(&checkpoint_dir.join("plan")), 0);
                    return false;
                }
            }
            true
        });
    assert!(matches!(cancelled, Err(ConvertError::Cancelled { .. })));
    assert!(checkpoint_dir.join("source.spool").is_file());
    assert_eq!(
        fs::read(checkpoint_dir.join("source.spool")).unwrap(),
        checkpoint_source.expect("cancelled after consuming some planned spools")
    );
    assert!(!output.exists());

    let changed_options = ConvertOptions::new(3, 0.01).unwrap();
    assert!(matches!(
        convert_file_with_checkpoint(
            &input,
            &output,
            changed_options,
            CheckpointMode::Resume,
            |_| true
        ),
        Err(ConvertError::Checkpoint(CheckpointError::Mismatch))
    ));
    assert!(checkpoint_dir.join("source.spool").is_file());

    let mut reread_source = false;
    let resume_options = options.with_spatial_workers(1).unwrap();
    let report = convert_file_with_checkpoint(
        &input,
        &output,
        resume_options,
        CheckpointMode::Resume,
        |event| {
            reread_source |= event.stage == ConversionStage::PlanningSpatialIndex;
            true
        },
    )
    .unwrap();
    assert_eq!(report.point_count, 5);
    assert!(!reread_source);
    assert!(output.exists());
    assert!(!checkpoint_dir.exists());

    convert_file(&input, &expected, options).unwrap();
    assert_eq!(fs::read(&output).unwrap(), fs::read(&expected).unwrap());
    assert_eq!(
        fs::read(output_attributes_path(&output)).unwrap(),
        fs::read(output_attributes_path(&expected)).unwrap()
    );
}

#[test]
fn resumes_validation_after_output_is_published_but_checkpoint_cleanup_did_not_run() {
    for version in [
        viewer_format::FormatVersion::V6_PRE1,
        viewer_format::FormatVersion::V6_PRE2,
        viewer_format::FormatVersion::V6_PRE3,
        viewer_format::FormatVersion::V6_PRE4,
        viewer_format::FormatVersion::V7_PRE1,
        viewer_format::FormatVersion::V7_PRE2,
        viewer_format::FormatVersion::V7_PRE3,
    ] {
        let tmp = test_dir();
        let dir = tmp.path().to_owned();
        let input = dir.join("source.las");
        let output = dir.join("result.oct");
        let expected = dir.join("expected.oct");
        let mut checkpoint_dir = output.as_os_str().to_owned();
        checkpoint_dir.push(".checkpoint");
        let checkpoint_dir = PathBuf::from(checkpoint_dir);
        fixture(&input, false, false, 5);
        let options = ConvertOptions::new(2, 0.01)
            .unwrap()
            .with_format_version(version)
            .unwrap();

        let cancelled = convert_file_with_checkpoint(
            &input,
            &output,
            options,
            CheckpointMode::Create,
            |event| event.stage != ConversionStage::ValidatingOutput,
        );
        assert!(matches!(cancelled, Err(ConvertError::Cancelled { .. })));
        let candidate = checkpoint_dir.join("candidate.oct");
        assert!(candidate.is_file());
        let candidate_bytes = fs::read(&candidate).unwrap();
        assert!(matches!(
            convert_file_with_checkpoint(
                &input,
                &output,
                ConvertOptions::new(2, 0.01).unwrap(),
                CheckpointMode::Resume,
                |_| true
            ),
            Err(ConvertError::Checkpoint(CheckpointError::Invalid(reason)))
                if reason == "unsupported output major version"
        ));
        assert!(!output.exists());
        assert!(candidate.is_file());
        let mut corrupted = candidate_bytes.clone();
        *corrupted.last_mut().unwrap() ^= 1;
        fs::write(&candidate, corrupted).unwrap();
        assert!(matches!(
            convert_file_with_checkpoint(&input, &output, options, CheckpointMode::Resume, |_| {
                true
            }),
            Err(ConvertError::Checkpoint(CheckpointError::Invalid(_)))
        ));
        assert!(candidate.exists());
        fs::write(&candidate, candidate_bytes).unwrap();
        fs::hard_link(&candidate, &output).unwrap();

        let report =
            convert_file_with_checkpoint(&input, &output, options, CheckpointMode::Resume, |_| {
                true
            })
            .unwrap();
        assert_eq!(report.point_count, 5);
        assert_eq!(report.block_preparation, None);
        assert!(report.output_validation.worker_limit > 0);
        assert!(!checkpoint_dir.exists());

        let mut reader = FormatReader::new(fs::File::open(&output).unwrap()).unwrap();
        assert_eq!(reader.format_version(), version);
        reader
            .attach_attributes(fs::File::open(output_attributes_path(&output)).unwrap())
            .unwrap();
        assert_eq!(
            reader.metadata().hierarchy().stats().source_point_count,
            Some(5)
        );
        let block_id = reader.block_storage_stats().unwrap()[0].id;
        assert!(!reader.read_block(block_id).unwrap().attributes().is_empty());

        convert_file(&input, &expected, options).unwrap();
        assert_eq!(fs::read(output).unwrap(), fs::read(expected).unwrap());
    }
}

#[test]
fn rejects_changed_input_and_preserves_the_checkpoint() {
    let tmp = test_dir();
    let dir = tmp.path().to_owned();
    let input = dir.join("source.las");
    let output = dir.join("result.oct");
    let mut checkpoint_dir = output.as_os_str().to_owned();
    checkpoint_dir.push(".checkpoint");
    let checkpoint_dir = PathBuf::from(checkpoint_dir);
    fixture(&input, false, false, 5);
    let options = ConvertOptions::new(2, 0.01).unwrap();

    let cancelled =
        convert_file_with_checkpoint(&input, &output, options, CheckpointMode::Create, |event| {
            event.stage != ConversionStage::BuildingSpatialHierarchy
        });
    assert!(matches!(cancelled, Err(ConvertError::Cancelled { .. })));
    let mut file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&input)
        .unwrap();
    file.seek(SeekFrom::End(-1)).unwrap();
    let mut last_byte = [0];
    file.read_exact(&mut last_byte).unwrap();
    last_byte[0] ^= 1;
    file.seek(SeekFrom::End(-1)).unwrap();
    file.write_all(&last_byte).unwrap();
    file.sync_all().unwrap();

    assert!(matches!(
        convert_file_with_checkpoint(&input, &output, options, CheckpointMode::Resume, |_| {
            true
        }),
        Err(ConvertError::Checkpoint(CheckpointError::Mismatch))
    ));
    assert!(checkpoint_dir.join("source.spool").is_file());
    assert!(!output.exists());
}
#[test]
fn pair_output_refuses_existing_companion_and_geometry_alone_is_gray() {
    let tmp = test_dir();
    let dir = tmp.path().to_owned();
    let input = dir.join("source.las");
    let output = dir.join("result.oct");
    let attrs = output_attributes_path(&output);
    fixture(&input, false, false, 5);
    fs::write(&attrs, b"foreign").unwrap();
    assert!(matches!(
        convert_file(&input, &output, ConvertOptions::default()),
        Err(ConvertError::OutputExists(_))
    ));
    assert!(!output.exists());
    assert_eq!(fs::read(&attrs).unwrap(), b"foreign");
    fs::remove_file(&attrs).unwrap();
    let report = convert_file(&input, &output, ConvertOptions::default()).unwrap();
    assert_eq!(
        report.output_bytes,
        fs::metadata(&output).unwrap().len() + fs::metadata(&attrs).unwrap().len()
    );
    let mut reader = FormatReader::new(fs::File::open(&output).unwrap()).unwrap();
    for block in reader.block_storage_stats().unwrap() {
        let data = reader.read_block(block.id).unwrap();
        assert!(data.attributes().is_empty());
        assert!(
            data.points()
                .points()
                .iter()
                .all(|p| p.color == viewer_core::ColorRgb::new(160, 160, 160))
        );
    }
}

#[test]
fn checkpoint_resumes_companion_only_publication_and_rejects_foreign_companion() {
    let tmp = test_dir();
    let dir = tmp.path().to_owned();
    let input = dir.join("source.las");
    let output = dir.join("result.oct");
    fixture(&input, false, false, 5);
    let options = ConvertOptions::default();
    assert!(matches!(
        convert_file_with_checkpoint(
            &input,
            &output,
            options,
            CheckpointMode::Create,
            |event| event.stage != ConversionStage::PublishingOutput
        ),
        Err(ConvertError::Cancelled { .. })
    ));
    let candidate_dir = dir.join("result.oct.checkpoint");
    let attrs = output_attributes_path(&output);
    fs::write(&attrs, b"foreign").unwrap();
    assert!(matches!(
        convert_file_with_checkpoint(&input, &output, options, CheckpointMode::Resume, |_| {
            true
        }),
        Err(ConvertError::OutputExists(_))
    ));
    assert_eq!(fs::read(&attrs).unwrap(), b"foreign");
    fs::remove_file(&attrs).unwrap();
    fs::hard_link(candidate_dir.join("candidate.octattrs"), &attrs).unwrap();
    convert_file_with_checkpoint(&input, &output, options, CheckpointMode::Resume, |_| true)
        .unwrap();
    let mut reader = FormatReader::new(fs::File::open(&output).unwrap()).unwrap();
    reader
        .attach_attributes(fs::File::open(&attrs).unwrap())
        .unwrap();
    assert!(!candidate_dir.exists());
}

/// Opt-in check against local datasets: `LAS_REAL_FILES` is newline-separated.
#[test]
#[ignore = "needs LAS_REAL_FILES"]
fn real_files_open() {
    let list = std::env::var("LAS_REAL_FILES").expect("LAS_REAL_FILES");
    let mut count = 0;
    for path in list.lines().filter(|line| !line.is_empty()) {
        crate::las_source::open_las_reader(std::path::Path::new(path))
            .unwrap_or_else(|error| panic!("{path}: {error}"));
        count += 1;
    }
    println!("real files opened: {count}");
}

#[test]
fn spool_diagnostics_account_written_and_live_bytes() {
    let tmp = test_dir();
    let dir = tmp.path().to_owned();
    let input = dir.join("source.las");
    fixture(&input, false, false, 4_000);
    let options = ConvertOptions::new(64, 0.01)
        .unwrap()
        .with_spatial_divisions(4)
        .unwrap()
        .with_root_partition_ranges(0)
        .unwrap();

    let mut written = Vec::new();
    for workers in [1, 2] {
        let output = dir.join(format!("workers-{workers}.oct"));
        let (_, diagnostics) = convert_file_with_diagnostics(
            &input,
            &output,
            options.with_spatial_workers(workers).unwrap(),
            |_| true,
        )
        .unwrap();
        let spools = diagnostics.spatial_spools.unwrap();
        let phases = diagnostics.spatial_phases.unwrap();
        assert!(phases.spool_bytes_written > 0);
        assert_eq!(
            phases.spool_bytes_written,
            spools.source_payload_written_bytes
                + spools.child_payload_written_bytes
                + spools.lod_payload_written_bytes
        );
        assert!(phases.peak_live_spool_bytes > 0);
        // Strict: internal nodes delete their input spools, so dropped removals would fail this.
        assert!(phases.peak_live_spool_bytes < phases.spool_bytes_written);
        assert!(!phases.root_child_spool_bytes_written.is_empty());
        assert!(
            phases.root_child_spool_bytes_written.iter().sum::<u64>() <= phases.spool_bytes_written
        );
        written.push((
            phases.spool_bytes_written,
            phases.root_child_spool_bytes_written,
        ));
    }
    // Totals and per-root-child bytes do not depend on the worker count.
    assert!(written.windows(2).all(|pair| pair[0] == pair[1]));
}

#[test]
fn node_spool_events_match_child_spool_totals() {
    let tmp = test_dir();
    let dir = tmp.path().to_owned();
    let input = dir.join("source.las");
    fixture(&input, false, false, 4_000);
    let options = ConvertOptions::new(64, 0.01)
        .unwrap()
        .with_spatial_divisions(4)
        .unwrap();

    let mut shapes = Vec::new();
    for workers in [1, 2, 4] {
        let output = dir.join(format!("events-{workers}.oct"));
        let (_, diagnostics) = convert_file_with_diagnostics(
            &input,
            &output,
            options.with_spatial_workers(workers).unwrap(),
            |_| true,
        )
        .unwrap();
        let spools = diagnostics.spatial_spools.unwrap();
        let events = diagnostics.spatial_phases.unwrap().node_spool_events;
        assert!(events.len() > 1);
        assert_eq!(
            events
                .iter()
                .flat_map(|event| event.child_spool_bytes.iter())
                .sum::<u64>(),
            spools.child_payload_written_bytes
        );
        assert_eq!(
            events.iter().map(|event| event.lod_bytes).sum::<u64>(),
            spools.lod_payload_written_bytes
        );
        // Final preorder ids: unique, contiguous, and every parent precedes its children.
        for (index, event) in events.iter().enumerate() {
            assert_eq!(event.node_id, index as u64);
            assert!(event.start_seq < event.end_seq);
            assert_eq!(event.is_leaf, event.lod_bytes == 0);
            let Some(parent_id) = event.parent_id else {
                assert_eq!(event.depth, 0);
                continue;
            };
            assert!(parent_id < event.node_id);
            let parent = &events[parent_id as usize];
            assert!(parent.start_seq < event.start_seq && event.end_seq < parent.end_seq);
            assert_eq!(event.depth, parent.depth + 1);
            assert!(parent.child_spool_bytes.contains(&event.input_spool_bytes));
        }
        // The tree and its byte counts do not depend on the worker count.
        shapes.push(
            events
                .iter()
                .map(|e| (e.node_id, e.parent_id, e.input_spool_bytes, e.lod_bytes))
                .collect::<Vec<_>>(),
        );
    }
    assert!(shapes.windows(2).all(|pair| pair[0] == pair[1]));
}

// --- Characterization of the spatial planner (Task 4, stage 2) -------------------------------
// These pin today's observable behaviour so the per-node-job refactor can prove it changed nothing.

fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut padded = data.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&(data.len() as u64 * 8).to_be_bytes());
    for chunk in padded.chunks(64) {
        let mut w = [0u32; 64];
        for (i, word) in chunk.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes(word.try_into().unwrap());
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let mut v = h;
        for i in 0..64 {
            let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
            let ch = (v[4] & v[5]) ^ (!v[4] & v[6]);
            let t1 = v[7]
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
            let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            let t2 = s0.wrapping_add(maj);
            v = [
                t1.wrapping_add(t2),
                v[0],
                v[1],
                v[2],
                v[3].wrapping_add(t1),
                v[4],
                v[5],
                v[6],
            ];
            // Rotate registers: a..h = [t1+t2, a, b, c, d+t1, e, f, g].
        }
        for (slot, value) in h.iter_mut().zip(v) {
            *slot = slot.wrapping_add(value);
        }
    }
    h.iter().map(|word| format!("{word:08x}")).collect()
}

/// One line per node in id order, plus the SHA-256 of `.oct` followed by its attributes file.
fn planner_snapshot(output: &std::path::Path) -> (Vec<String>, String) {
    let mut reader = FormatReader::new(fs::File::open(output).unwrap()).unwrap();
    reader
        .attach_attributes(fs::File::open(output_attributes_path(output)).unwrap())
        .unwrap();
    let hierarchy = reader.metadata().hierarchy();
    let nodes = (0..hierarchy.stats().node_count as u64)
        .map(|id| {
            let node = hierarchy.node(NodeId::new(id)).unwrap();
            let blocks = node.blocks().iter().map(|b| b.get()).collect::<Vec<_>>();
            let parent = node.parent().map(|p| p.get());
            let children = hierarchy
                .child_ids(node.id())
                .iter()
                .map(|c| c.get())
                .collect::<Vec<_>>();
            if let Some(parent) = parent {
                assert!(parent < id, "parent must precede child");
            }
            assert!(
                children.windows(2).all(|pair| pair[0] < pair[1]) && children.iter().all(|c| *c > id),
                "children ascend and follow their parent"
            );
            format!(
                "id={id} parent={parent:?} level={} points={} blocks={blocks:?} children={children:?}",
                node.level(),
                node.point_count()
            )
        })
        .collect();
    let mut bytes = fs::read(output).unwrap();
    bytes.extend(fs::read(output_attributes_path(output)).unwrap());
    (nodes, sha256_hex(&bytes))
}

fn planner_snapshot_for(
    input: &std::path::Path,
    dir: &std::path::Path,
    options: ConvertOptions,
) -> (Vec<String>, String) {
    let reference = {
        let output = dir.join("w1.oct");
        convert_file(input, &output, options.with_spatial_workers(1).unwrap()).unwrap();
        planner_snapshot(&output)
    };
    for workers in [2, 4] {
        let output = dir.join(format!("w{workers}.oct"));
        convert_file(
            input,
            &output,
            options.with_spatial_workers(workers).unwrap(),
        )
        .unwrap();
        assert_eq!(planner_snapshot(&output), reference, "workers={workers}");
    }
    reference
}

#[test]
fn sha256_matches_the_known_vector() {
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

fn assert_structure(
    nodes: &[String],
    sha: &str,
    expected_count: usize,
    expected_head: &[&str],
    expected_listing_sha: &str,
    expected_output_sha: &str,
) {
    assert_eq!(nodes.len(), expected_count);
    assert_eq!(&nodes[..expected_head.len()], expected_head);
    for (id, line) in nodes.iter().enumerate() {
        // Block ids are assigned in node preorder, one block per node here.
        if expected_count > 1 {
            assert!(line.contains(&format!("blocks=[{id}]")), "{line}");
        }
    }
    assert_eq!(
        sha256_hex(nodes.join("\n").as_bytes()),
        expected_listing_sha
    );
    assert_eq!(sha, expected_output_sha);
}

#[test]
fn characterize_planner_structure_for_4000_points_with_divisions_4() {
    let tmp = test_dir();
    let input = tmp.path().join("source.las");
    fixture(&input, false, true, 4_000);
    let options = ConvertOptions::new(64, 0.01)
        .unwrap()
        .with_spatial_divisions(4)
        .unwrap();
    let (nodes, sha) = planner_snapshot_for(&input, tmp.path(), options);
    println!(
        "{}\nL={}\nSHA={sha}",
        nodes.len(),
        sha256_hex(nodes.join("\n").as_bytes())
    );
    assert_structure(
        &nodes,
        &sha,
        85,
        &[
            "id=0 parent=None level=0 points=8 blocks=[0] children=[1, 22, 43, 64]",
            "id=1 parent=Some(0) level=1 points=8 blocks=[1] children=[2, 7, 12, 17]",
            "id=2 parent=Some(1) level=2 points=8 blocks=[2] children=[3, 4, 5, 6]",
            "id=3 parent=Some(2) level=3 points=63 blocks=[3] children=[]",
        ],
        "15f90b1c8422ed4e14e21a47e0bf996dab964931fcc1e3a03e5f8f3ec1ef9b24",
        "8aab7d4add63a113f902367da78aaa77c0ad3b33094dd3726354a07253ebba42",
    );
}

#[test]
fn characterize_planner_structure_for_coincident_points() {
    let tmp = test_dir();
    let input = tmp.path().join("coincident.las");
    coincident_fixture(&input);
    let options = ConvertOptions::new(2, 0.01)
        .unwrap()
        .with_spatial_divisions(4)
        .unwrap();
    let (nodes, sha) = planner_snapshot_for(&input, tmp.path(), options);
    println!("L={}\nSHA={sha}", sha256_hex(nodes.join("\n").as_bytes()));
    assert_structure(
        &nodes,
        &sha,
        1,
        &["id=0 parent=None level=0 points=5 blocks=[0, 1, 2] children=[]"],
        "4b45d9a0f4c2345becff50c73ebf2e64ec214b97e5dfab5ce66b218274090908",
        "3ab4e637732e6ba996f5613244a97b77b2d03ed80160ddc3b8b46225303c1278",
    );
}

const BUDGET_HINT: &str = "budget literals depend on the estimate_memory constants and the fixture; \
    a mismatch may mean the estimate changed, not necessarily the planner";

/// `(requested, limit)` of a rejected plan, `None` when the plan was accepted.
fn planner_budget_outcome(
    input: &std::path::Path,
    dir: &std::path::Path,
    budget: u64,
    workers: usize,
) -> Option<(u64, u64)> {
    let output = dir.join(format!("budget-{budget}-{workers}.oct"));
    let options = ConvertOptions::with_memory_budget(64, 0.01, budget)
        .unwrap()
        .with_spatial_divisions(4)
        .unwrap()
        .with_spatial_workers(workers)
        .unwrap();
    match convert_file(input, &output, options) {
        Ok(_) => None,
        Err(ConvertError::MemoryBudgetExceeded { requested, limit }) => {
            assert!(!output.exists());
            Some((requested, limit))
        }
        Err(other) => panic!("budget={budget} workers={workers}: unexpected {other:?}"),
    }
}

#[test]
fn characterize_planner_budget_outcomes_with_one_worker() {
    let tmp = test_dir();
    let input = tmp.path().join("source.las");
    fixture(&input, false, true, 4_000);
    // (budget, expected). The root check rejects below 19_017_728; between that and
    // 19_083_007 a later check in the serial plan rejects with a larger `requested`.
    let cases = [
        (1, Some((19_017_728, 1))),
        (19_017_727, Some((19_017_728, 19_017_727))),
        (19_017_728, Some((19_018_240, 19_017_728))),
        (19_033_856, Some((19_034_368, 19_033_856))),
        (19_083_007, Some((19_083_008, 19_083_007))),
        (19_083_008, None),
        (23_342_592, None),
    ];
    for (budget, expected) in cases {
        assert_eq!(
            planner_budget_outcome(&input, tmp.path(), budget, 1),
            expected,
            "{BUDGET_HINT} budget={budget}"
        );
    }
}

#[test]
fn characterize_planner_budget_outcomes_with_two_workers() {
    let tmp = test_dir();
    let input = tmp.path().join("source.las");
    fixture(&input, false, true, 4_000);
    // 19_017_728..=23_000_000 fail the batch budget (`requested` is the batch reservation);
    // 23_342_591 fails a branch-local check, whose error wins over the batch one;
    // 23_342_592 is the smallest budget that is accepted.
    let cases = [
        (1, Some((19_017_728, 1))),
        (19_017_727, Some((19_017_728, 19_017_727))),
        (19_017_728, Some((19_018_240, 19_017_728))),
        (19_033_855, Some((23_213_568, 19_033_855))),
        (22_228_736, Some((23_213_568, 22_228_736))),
        (23_000_000, Some((23_213_568, 23_000_000))),
        (23_342_591, Some((19_033_856, 19_033_855))),
        (23_342_592, None),
        (23_342_593, None),
    ];
    for (budget, expected) in cases {
        assert_eq!(
            planner_budget_outcome(&input, tmp.path(), budget, 2),
            expected,
            "{BUDGET_HINT} budget={budget}"
        );
    }
}
fn eight_cluster_fixture(path: &std::path::Path) {
    let header = Builder::from((1, 2)).into_header().unwrap();
    let mut writer = Writer::from_path(path, header).unwrap();
    for index in 0..800usize {
        let cluster = index % 8;
        let jitter = (index / 8) as f64 * 0.01;
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

#[test]
fn characterize_planner_budget_outcomes_with_three_and_four_workers() {
    let tmp = test_dir();
    let input = tmp.path().join("source.las");
    fixture(&input, false, true, 4_000);
    // W=3 splits the 4 root children into batches of 3 + 1; W=4 is a single batch.
    let cases = [
        (3, 26_359_295, Some((26_359_296, 26_359_295))),
        (3, 26_359_296, Some((19_018_240, 19_017_728))),
        (3, 26_454_720, Some((19_033_856, 19_033_632))),
        (3, 26_458_816, None),
        (4, 29_505_023, Some((29_505_024, 29_505_023))),
        (4, 29_505_024, Some((19_018_240, 19_017_728))),
        (4, 29_633_216, Some((19_033_856, 19_033_752))),
        (4, 29_637_312, None),
    ];
    for (workers, budget, expected) in cases {
        assert_eq!(
            planner_budget_outcome(&input, tmp.path(), budget, workers),
            expected,
            "{BUDGET_HINT} budget={budget} workers={workers}"
        );
    }
}

#[test]
fn characterize_later_batches_never_fail_the_batch_budget() {
    // A later batch could only fail its own batch check (`branch_memory_budget` with the
    // committed counts) in a window where batch 1 passes its batch check and its branch-local
    // checks. A dense probe (step 64) of the 4000-point fixture above the batch-1 threshold at
    // W=2 and W=3 found no such window: below the accept threshold batch 1 fails a
    // branch-local check, at or above it everything passes. The 8-cluster fixture (8 root
    // children: 4 batches at W=2, 3 at W=3) shows the same shape at a coarse step.
    let tmp = test_dir();
    let input = tmp.path().join("clusters.las");
    eight_cluster_fixture(&input);
    let cases = [
        (2, 23_213_567, Some((23_213_568, 23_213_567))),
        (2, 23_220_000, Some((19_019_776, 19_019_336))),
        (2, 23_340_000, None),
        (3, 26_360_000, Some((19_018_240, 19_017_845))),
        (3, 26_460_000, None),
    ];
    for (workers, budget, expected) in cases {
        assert_eq!(
            planner_budget_outcome(&input, tmp.path(), budget, workers),
            expected,
            "{BUDGET_HINT} budget={budget} workers={workers}"
        );
    }
}

fn convert_error_at(
    input: &std::path::Path,
    dir: &std::path::Path,
    workers: usize,
) -> ConvertError {
    let output = dir.join(format!("err-{workers}.oct"));
    let options = ConvertOptions::new(16, 0.01)
        .unwrap()
        .with_spatial_divisions(4)
        .unwrap()
        .with_spatial_workers(workers)
        .unwrap();
    let error = convert_file(input, &output, options).unwrap_err();
    assert!(!output.exists());
    error
}

#[test]
fn characterize_input_errors_are_identical_across_worker_counts() {
    let tmp = test_dir();
    let dir = tmp.path();
    // Header max X lowered (offset 179, as in the point_source tests): the first point beyond it
    // is reported by global index while the source spool is written, before any planner worker,
    // so branches never see it. No data error is reachable from a planner branch today.
    let bounds = dir.join("bounds.las");
    fixture(&bounds, false, false, 1_000);
    let mut bytes = fs::read(&bounds).unwrap();
    bytes[179..187].copy_from_slice(&1_000_100.0_f64.to_le_bytes());
    fs::write(&bounds, bytes).unwrap();
    // Truncated point data is rejected at preflight, so PointCountMismatch is not reachable
    // through the public conversion API; the preflight error is pinned instead.
    let truncated = dir.join("truncated.las");
    fixture(&truncated, false, false, 1_000);
    let length = fs::metadata(&truncated).unwrap().len();
    fs::OpenOptions::new()
        .write(true)
        .open(&truncated)
        .unwrap()
        .set_len(length - 100)
        .unwrap();
    for workers in [1, 2, 4] {
        let error = convert_error_at(&bounds, dir, workers);
        assert!(
            matches!(
                error,
                ConvertError::PointOutsideHeaderBounds {
                    point_index: 401,
                    ..
                }
            ),
            "workers={workers}: {error:?}"
        );
        assert_eq!(
            error.to_string(),
            "point 401 at [1000100.25, 2000002.0, 51.0] is outside the LAS/LAZ header bounds"
        );
        let error = convert_error_at(&truncated, dir, workers);
        assert!(
            matches!(error, ConvertError::LasPreflight(_)),
            "workers={workers}: {error:?}"
        );
        assert_eq!(
            error.to_string(),
            "unsafe LAS/LAZ header: header point count x point_data_record_length (1000 x 20 B) exceeds the 19900 B after offset_to_point_data"
        );
    }
}

#[test]
fn sha256_matches_the_two_block_vector() {
    assert_eq!(
        sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
        "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
    );
}

/// Live-spool peak of variant c with one worker in `benchmarks/model_node_job_arena.py`, replayed
/// from the node events of a run: jobs run in `start_seq` order; a job's child spools are counted
/// in full at its start (each is its own file), its LOD sample joins at its end, and its input
/// dies at its end. Leaf spools and LOD samples stay live until the block phase. A node that
/// became a leaf (all points equal) deletes its scratch children at its end. Pure leaves are no
/// jobs.
fn variant_c_serial_peak(events: &[crate::diagnostics::NodeSpoolEvent]) -> u64 {
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
        las_source::{inspect_source, open_las_reader},
        pipeline::spool_store::{FileSpoolStore, MemSpoolStore, SpoolAccounting},
        workspace::Workspace,
    };

    let tmp = test_dir();
    let input = tmp.path().join("source.las");
    fixture(&input, false, true, 4_000);
    let options = ConvertOptions::new(64, 0.01)
        .unwrap()
        .with_spatial_divisions(4)
        .unwrap()
        .with_spatial_workers(1)
        .unwrap();
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

/// Preorder ids (a depth-first walk in child-key order visits 0, 1, 2, …), parents before
/// children, children in ascending key, and block ids ascending in that preorder.
fn assert_preorder_structure(output: &std::path::Path, divisions: u8) {
    let reader = FormatReader::new(fs::File::open(output).unwrap()).unwrap();
    let hierarchy = reader.metadata().hierarchy();
    let count = hierarchy.stats().node_count as u64;
    let mut visited = Vec::new();
    let mut stack = vec![NodeId::new(0)];
    while let Some(id) = stack.pop() {
        visited.push(id.get());
        let node = hierarchy.node(id).unwrap();
        let children = hierarchy.child_ids(id);
        let subdivision = viewer_core::SpatialSubdivision::new(node.bounds(), divisions).unwrap();
        let keys = children
            .iter()
            .map(|child| {
                let child = hierarchy.node(*child).unwrap();
                assert!(child.parent() == Some(id) && child.id().get() > id.get());
                let (min, max) = (child.bounds().min(), child.bounds().max());
                subdivision.cell_index(std::array::from_fn(|axis| (min[axis] + max[axis]) / 2.0))
            })
            .collect::<Vec<_>>();
        assert!(keys.windows(2).all(|pair| pair[0] < pair[1]), "{keys:?}");
        stack.extend(children.iter().rev().copied());
    }
    assert_eq!(visited, (0..count).collect::<Vec<_>>(), "ids are preorder");
    let blocks = (0..count)
        .flat_map(|id| hierarchy.node(NodeId::new(id)).unwrap().blocks().to_vec())
        .map(|block| block.get())
        .collect::<Vec<_>>();
    assert_eq!(blocks, (0..blocks.len() as u64).collect::<Vec<_>>());
}

#[test]
fn node_jobs_commit_nodes_and_blocks_in_preorder() {
    let tmp = test_dir();
    let dir = tmp.path();
    type WriteFixture = fn(&std::path::Path);
    let fixtures: [(&str, WriteFixture); 5] = [
        ("dense.las", |path| fixture(path, false, true, 4_000)),
        ("clusters.las", eight_cluster_fixture),
        ("coincident.las", coincident_fixture),
        ("equal-root.las", |path| all_equal_root_fixture(path, 300)),
        ("equal-cluster.las", equal_cluster_fixture),
    ];
    for (name, write) in fixtures {
        let input = dir.join(name);
        write(&input);
        for divisions in [2, 4, 8] {
            let options = ConvertOptions::new(16, 0.01)
                .unwrap()
                .with_spatial_divisions(divisions)
                .unwrap();
            // Every worker count gives the same nodes and bytes (planner_snapshot_for).
            let case = dir.join(format!("{name}-{divisions}"));
            fs::create_dir(&case).unwrap();
            planner_snapshot_for(&input, &case, options);
            assert_preorder_structure(&case.join("w1.oct"), divisions);
        }
    }
}

/// Plans with a `MemSpoolStore`, reads and releases every block as the block phase does, and
/// checks the store lifecycle held and no spool is left.
fn mem_plan_and_drain(
    input: &std::path::Path,
    options: ConvertOptions,
) -> crate::diagnostics::SpatialPhaseDiagnostics {
    use crate::{
        las_source::{inspect_source, open_las_reader},
        pipeline::spool_store::MemSpoolStore,
    };
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
            let options = ConvertOptions::new(16, 0.01)
                .unwrap()
                .with_spatial_divisions(4)
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
    let options = ConvertOptions::new(16, 0.01).unwrap();
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
    let options = ConvertOptions::new(16, 0.01).unwrap();
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
    assert!(hierarchy.child_ids(node.id()).is_empty());
}

/// A `MemSpoolStore` whose `reader` fails once for each listed point count (after its delay), so
/// a job error lands on the node whose input spool holds that many records.
struct FailingStore {
    inner: crate::pipeline::spool_store::MemSpoolStore,
    record_bytes: u64,
    failures: std::sync::Mutex<Vec<(u64, std::time::Duration)>>,
}

impl crate::pipeline::spool_store::SpoolStore for FailingStore {
    type Reader = std::io::Cursor<Vec<u8>>;
    fn create(&self, id: crate::pipeline::spool_store::SpoolId) -> Result<(), ConvertError> {
        self.inner.create(id)
    }
    fn append(
        &self,
        id: crate::pipeline::spool_store::SpoolId,
        bytes: &[u8],
    ) -> Result<(), ConvertError> {
        self.inner.append(id, bytes)
    }
    fn close(&self, id: crate::pipeline::spool_store::SpoolId) -> Result<(), ConvertError> {
        self.inner.close(id)
    }
    fn len(&self, id: crate::pipeline::spool_store::SpoolId) -> Result<u64, ConvertError> {
        self.inner.len(id)
    }
    fn reader(
        &self,
        id: crate::pipeline::spool_store::SpoolId,
        extent: crate::pipeline::spool_store::SpoolExtent,
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
    fn delete(&self, id: crate::pipeline::spool_store::SpoolId) -> Result<(), ConvertError> {
        self.inner.delete(id)
    }
}

impl crate::pipeline::spool_store::SpoolAccounting for FailingStore {
    fn next_event_seq(&self) -> Option<u64> {
        self.inner.next_event_seq()
    }
    fn ledger_totals(&self) -> Option<crate::pipeline::spool_store::LedgerTotals> {
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
    on_progress: &mut dyn FnMut(super::ConversionProgress) -> bool,
) -> (ConvertError, crate::pipeline::spool_store::MemSpoolStats) {
    use crate::{
        las_source::{inspect_source, open_las_reader},
        pipeline::spool_store::MemSpoolStore,
        spatial::SpoolLayout,
    };
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

/// Plans from an already opened reader with the native clock and executor.
fn plan_from_reader<S: crate::pipeline::spool_store::PlanStore>(
    reader: las::Reader,
    input: &std::path::Path,
    source: &crate::source::SourceInfo,
    options: ConvertOptions,
    store: &S,
    on_progress: &mut dyn FnMut(super::ConversionProgress) -> bool,
    collect: bool,
) -> Result<crate::spatial::SpatialPlan, ConvertError> {
    let factory = crate::pipeline::point_source::LasFactory::new(crate::las_source::FileBytes {
        path: input.to_owned(),
        len: std::fs::metadata(input).unwrap().len(),
    });
    crate::spatial::build_spatial_plan_from_source(
        &mut crate::pipeline::point_source::RangeSource::Las(
            crate::pipeline::point_source::LasPointSource::full(reader),
        ),
        &factory,
        source,
        options,
        store,
        &crate::pipeline::clock::SystemClock,
        &crate::executor::ThreadedExecutor,
        on_progress,
        collect,
    )
}

fn cluster_options(workers: usize) -> ConvertOptions {
    ConvertOptions::new(16, 0.01)
        .unwrap()
        .with_spatial_workers(workers)
        .unwrap()
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
    let options = ConvertOptions::new(16, 0.01)
        .unwrap()
        .with_spatial_divisions(4)
        .unwrap()
        .with_spatial_workers(2)
        .unwrap();
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
fn a_lower_branch_commit_error_wins_over_a_higher_branch_job_error() {
    // Root children 0 and 6 hold 2000 and 2001 points; 27_500_000 passes the batch check of the
    // single batch of 8 but fails a branch-local commit deep in branch 0 (found by a budget
    // probe). Branch 6 fails its first read at once (no other node has 2001 points), so its Job
    // error arrives first; the lower Commit error must still win (A8, R25).
    let tmp = test_dir();
    let input = tmp.path().join("clusters.las");
    sized_cluster_fixture(&input, [2000, 40, 40, 40, 40, 40, 2001, 40]);
    let quick = std::time::Duration::ZERO;
    let options = |budget| {
        ConvertOptions::with_memory_budget(16, 0.01, budget)
            .unwrap()
            .with_spatial_workers(8)
            .unwrap()
    };
    let (job_only, _) = failing_plan(
        &input,
        options(512 * 1024 * 1024),
        &[(2001, quick)],
        &mut |_| true,
    );
    assert_eq!(job_only.to_string(), "injected read failure (2001 points)");
    let (commit_only, _) = failing_plan(&input, options(27_500_000), &[], &mut |_| true);
    assert!(
        matches!(commit_only, ConvertError::MemoryBudgetExceeded { requested, limit }
            if requested < 27_500_000 && limit < requested),
        "{BUDGET_HINT}: {commit_only:?}"
    );
    for _ in 0..5 {
        let (error, stats) =
            failing_plan(&input, options(27_500_000), &[(2001, quick)], &mut |_| true);
        assert_eq!(error.to_string(), commit_only.to_string());
        assert_eq!((stats.live_bytes, stats.live_spools), (0, 0));
    }
}

#[test]
fn the_serial_executor_plans_like_the_ready_queue() {
    use crate::{
        las_source::{inspect_source, open_las_reader},
        pipeline::{
            clock::SystemClock,
            executor::SerialExecutor,
            point_source::{LasFactory, LasPointSource, MemBytes, RangeSource},
            spool_store::MemSpoolStore,
        },
        spatial::build_spatial_plan_from_source,
    };

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
        let shape = |plan: &crate::spatial::SpatialPlan| {
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

// --- Serial core path (`convert_serial` / `validate_output_serial`) against the native CLI path ---

struct SerialRun {
    geometry: Vec<u8>,
    attributes: Vec<u8>,
    plan: viewer_convert_core::PlanSummary,
}

fn run_serial(input: &std::path::Path, options: ConvertOptions) -> SerialRun {
    use std::io::Cursor;
    use viewer_convert_core::{LasFactory, MemBytes, MemSpoolStore, NoClock, convert_serial};
    let factory = LasFactory::new(MemBytes(fs::read(input).unwrap().into()));
    let store = MemSpoolStore::new(false);
    let output = convert_serial(
        &factory,
        &store,
        &NoClock,
        options,
        Cursor::new(Vec::new()),
        Cursor::new(Vec::new()),
        &mut |_| true,
    )
    .unwrap();
    let stats = store.stats();
    assert_eq!((stats.live_spools, stats.live_bytes), (0, 0));
    SerialRun {
        geometry: output.geometry.into_inner(),
        attributes: output.attributes.unwrap().into_inner(),
        plan: output.plan,
    }
}

fn assert_serial_matches_native(
    input: &std::path::Path,
    options: ConvertOptions,
    expect_multiple_blocks: bool,
    label: &str,
) -> SerialRun {
    let tmp = test_dir();
    let native = tmp.path().join("native.oct");
    convert_file(input, &native, options).unwrap();
    let (native_geometry, native_attributes) = output_pair_bytes(&native);
    let serial = run_serial(input, options);
    assert!(
        serial.geometry == native_geometry,
        "{label}: .oct bytes differ"
    );
    assert!(
        serial.attributes == native_attributes,
        "{label}: .octattrs bytes differ"
    );
    if expect_multiple_blocks {
        assert!(serial.plan.block_count > 1, "{label}: single block");
    }
    assert_eq!(serial.plan.block_count, serial.plan.block_ids.len() as u64);
    serial
}

fn workspace_path(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
}

#[test]
fn serial_core_matches_native_on_the_small_las() {
    let input = workspace_path("data/samples/fs_e430_n4008_18.las");
    assert_serial_matches_native(&input, ConvertOptions::default(), false, "default");
    for divisions in [2, 8] {
        let options = ConvertOptions::new(4096, 0.01)
            .unwrap()
            .with_spatial_divisions(divisions)
            .unwrap();
        assert_serial_matches_native(&input, options, true, &format!("4096/{divisions}"));
    }
}

#[test]
fn serial_core_matches_native_on_the_variable_chunk_laz() {
    let input = workspace_path("tools/laz_chunk_probe/fixtures/synthetic_variable_chunks.laz");
    let expected = [
        (2, "0b8a8016cf13", "c8e7008c70e4"),
        (8, "657f70173571", "d8478842254c"),
    ];
    for (divisions, oct, attrs) in expected {
        let options = ConvertOptions::new(4096, 0.01)
            .unwrap()
            .with_spatial_divisions(divisions)
            .unwrap();
        let serial =
            assert_serial_matches_native(&input, options, true, &format!("laz/{divisions}"));
        assert!(sha256_hex(&serial.geometry).starts_with(oct));
        assert!(sha256_hex(&serial.attributes).starts_with(attrs));
    }
}

/// Serial conversion over a `HybridSpoolStore` (RAM budget unlimited) against a `MemSpoolStore`.
fn assert_hybrid_matches_mem(
    input: &std::path::Path,
    options: ConvertOptions,
    budget: u64,
    label: &str,
) -> viewer_convert_core::HybridStats {
    use std::io::Cursor;
    use viewer_convert_core::{
        HybridConfig, HybridSpoolStore, LasFactory, MemBytes, NoClock, VecPageTier, VecRamTier,
        convert_serial,
    };
    let reference = run_serial(input, options);
    let factory = LasFactory::new(MemBytes(fs::read(input).unwrap().into()));
    let store = HybridSpoolStore::new(
        VecRamTier::new(),
        VecPageTier::new(1, None),
        HybridConfig::web(budget),
        false,
    )
    .unwrap();
    let output = convert_serial(
        &factory,
        &store,
        &NoClock,
        options,
        Cursor::new(Vec::new()),
        Cursor::new(Vec::new()),
        &mut |_| true,
    )
    .unwrap();
    assert!(
        output.geometry.into_inner() == reference.geometry,
        "{label}: .oct bytes differ"
    );
    assert!(
        output.attributes.unwrap().into_inner() == reference.attributes,
        "{label}: .octattrs bytes differ"
    );
    assert_eq!((store.live_ids(), store.segment_count()), (0, 0), "{label}");
    let stats = store.stats();
    assert_eq!((stats.live_payload_bytes, stats.ram_retained_bytes), (0, 0));
    assert_eq!(stats.pages_in_use, 0, "{label}: pages leaked");
    assert!(stats.ram_peak_bytes > 0, "{label}: nothing was spooled");
    stats
}

#[test]
fn hybrid_store_matches_mem_store_on_the_small_las_and_the_variable_chunk_laz() {
    for relative in [
        "data/samples/fs_e430_n4008_18.las",
        "tools/laz_chunk_probe/fixtures/synthetic_variable_chunks.laz",
    ] {
        let input = workspace_path(relative);
        for divisions in [2, 8] {
            let options = ConvertOptions::new(4096, 0.01)
                .unwrap()
                .with_spatial_divisions(divisions)
                .unwrap();
            let label = format!("{relative}/{divisions}");
            let stats = assert_hybrid_matches_mem(&input, options, u64::MAX, &label);
            assert_eq!(stats.spilled_spools, 0, "{label}");
            // A tiny budget and none at all both spill and still give the same bytes.
            for budget in [64 * 1024, 0] {
                let stats = assert_hybrid_matches_mem(&input, options, budget, &label);
                assert!(stats.spilled_spools > 0, "{label}/{budget}");
            }
        }
    }
}

#[test]
fn serial_core_matches_native_on_synthetic_fixtures_for_each_worker_count() {
    let tmp = test_dir();
    for compressed in [false, true] {
        let input = tmp.path().join(format!("source-{compressed}.las"));
        fixture(&input, compressed, true, 4_000);
        for workers in [1, 4] {
            let options = ConvertOptions::new(64, 0.01)
                .unwrap()
                .with_spatial_workers(workers)
                .unwrap();
            assert_serial_matches_native(
                &input,
                options,
                true,
                &format!("compressed={compressed} workers={workers}"),
            );
        }
    }
}

fn serial_validation(
    run: &SerialRun,
) -> Result<viewer_convert_core::ConversionWorkerStats, ConvertError> {
    use std::io::Cursor;
    viewer_convert_core::validate_output_serial(
        Cursor::new(run.geometry.as_slice()),
        Some(Cursor::new(run.attributes.as_slice())),
        &run.plan,
        &mut |_| true,
    )
}

#[test]
fn serial_validation_accepts_the_output_and_rejects_a_flipped_payload_byte_like_native() {
    let input = workspace_path("tools/laz_chunk_probe/fixtures/synthetic_variable_chunks.laz");
    let options = ConvertOptions::new(4096, 0.01).unwrap();
    let run = run_serial(&input, options);
    serial_validation(&run).unwrap();

    let tmp = test_dir();
    let native_validation = |run: &SerialRun| {
        let path = tmp.path().join("copy.oct");
        fs::write(&path, &run.geometry).unwrap();
        fs::write(output_attributes_path(&path), &run.attributes).unwrap();
        validate_output_pair(
            &path,
            Some(&output_attributes_path(&path)),
            &OutputValidation {
                total_points: run.plan.point_count,
                block_ids: &run.plan.block_ids,
                source_block_ids: &run.plan.source_block_ids,
                file_length: run.geometry.len() as u64,
                attributes_length: run.attributes.len() as u64,
                requested_workers: 1,
                memory_budget_bytes: options.max_memory_budget_bytes(),
                reserved_memory_bytes: run.plan.estimated_peak_memory_bytes,
            },
            &mut |_| true,
            None,
        )
    };
    native_validation(&run).unwrap();

    // Positions inside the file body, which is dominated by block payloads.
    for position in [run.geometry.len() / 3, run.geometry.len() / 2] {
        let mut corrupted = SerialRun {
            geometry: run.geometry.clone(),
            attributes: run.attributes.clone(),
            plan: run.plan.clone(),
        };
        corrupted.geometry[position] ^= 0xFF;
        let native = native_validation(&corrupted).unwrap_err();
        let serial = serial_validation(&corrupted).unwrap_err();
        assert_eq!(
            std::mem::discriminant(&native),
            std::mem::discriminant(&serial)
        );
        assert_eq!(native.to_string(), serial.to_string());
    }
}

#[test]
fn serial_validation_reports_progress_and_honours_cancellation() {
    use std::io::Cursor;
    use viewer_convert_core::{ConversionProgress, ConversionStage, validate_output_serial};
    let input = workspace_path("data/samples/fs_e430_n4008_18.las");
    let run = run_serial(&input, ConvertOptions::new(4096, 0.01).unwrap());
    let mut last = None;
    validate_output_serial(
        Cursor::new(run.geometry.as_slice()),
        Some(Cursor::new(run.attributes.as_slice())),
        &run.plan,
        &mut |progress: ConversionProgress| {
            assert_eq!(progress.stage, ConversionStage::ValidatingOutput);
            last = Some(progress.points_processed);
            true
        },
    )
    .unwrap();
    assert_eq!(last, Some(run.plan.point_count));
    let error = validate_output_serial(
        Cursor::new(run.geometry.as_slice()),
        Some(Cursor::new(run.attributes.as_slice())),
        &run.plan,
        &mut |_| false,
    )
    .unwrap_err();
    assert!(matches!(error, ConvertError::Cancelled { .. }));
}

// --- Root partition by ranges on `ThreadedExecutor` (one thread per range, files) ---

fn ranged(options: ConvertOptions, ranges: usize) -> ConvertOptions {
    options.with_root_partition_ranges(ranges).unwrap()
}

fn block_options(divisions: u8) -> ConvertOptions {
    ConvertOptions::new(4096, 0.01)
        .unwrap()
        .with_spatial_divisions(divisions)
        .unwrap()
}

fn convert_pair(input: &std::path::Path, options: ConvertOptions) -> (Vec<u8>, Vec<u8>) {
    let tmp = test_dir();
    let output = tmp.path().join("out.oct");
    convert_file(input, &output, options).unwrap();
    output_pair_bytes(&output)
}

fn assert_threaded_ranges_match_default(input: &std::path::Path, label: &str) {
    for divisions in [2, 4, 8] {
        let expected = convert_pair(input, block_options(divisions));
        for ranges in 1..=8 {
            let got = convert_pair(input, ranged(block_options(divisions), ranges));
            assert!(
                got == expected,
                "{label}: divisions {divisions} ranges {ranges} differ"
            );
        }
    }
}

#[test]
fn threaded_root_ranges_match_the_default_scan_on_las_and_laz() {
    let tmp = test_dir();
    let las = tmp.path().join("ranges.las");
    fixture(&las, false, true, 20_001);
    assert_threaded_ranges_match_default(&las, "las");
    let variable = workspace_path("tools/laz_chunk_probe/fixtures/synthetic_variable_chunks.laz");
    assert_threaded_ranges_match_default(&variable, "variable laz");
    // 110 007 points: chunks of 50 000, 50 000 and 10 007.
    let fixed = tmp.path().join("ranges.laz");
    fixture(&fixed, true, true, 110_007);
    for divisions in [2, 8] {
        let expected = convert_pair(&fixed, block_options(divisions));
        for ranges in [1, 2, 3, 8] {
            let got = convert_pair(&fixed, ranged(block_options(divisions), ranges));
            assert!(got == expected, "fixed laz {divisions}/{ranges}");
        }
    }
}

#[test]
fn threaded_root_ranges_report_the_lowest_start_error_of_two_failing_ranges() {
    let tmp = test_dir();
    let input = tmp.path().join("outliers.las");
    fixture(&input, false, true, 20_000);
    // Header max X below the points from about index 9 000 on: ranges 1 and 2 (of 3) both fail.
    let mut bytes = fs::read(&input).unwrap();
    let max_x: f64 = 1_000_000.0 + 9_000.0 * 0.25 - 1.0;
    bytes[179..187].copy_from_slice(&max_x.to_le_bytes());
    fs::write(&input, bytes).unwrap();
    let message = |options: ConvertOptions| {
        let output = tmp.path().join("out.oct");
        let error = convert_file(&input, &output, options).unwrap_err();
        assert!(!output.exists());
        error.to_string()
    };
    let serial = message(block_options(2));
    assert!(
        serial.contains("is outside the LAS/LAZ header bounds"),
        "{serial}"
    );
    for ranges in [2, 3, 8] {
        for _ in 0..10 {
            assert_eq!(message(ranged(block_options(2), ranges)), serial);
        }
    }
}

#[test]
fn threaded_root_ranges_report_a_corrupt_laz_chunk_like_the_default_scan() {
    let tmp = test_dir();
    let input = tmp.path().join("corrupt.laz");
    let mut bytes = fs::read(workspace_path(
        "tools/laz_chunk_probe/fixtures/synthetic_variable_chunks.laz",
    ))
    .unwrap();
    // Well inside the 50 000-point chunk; the small leading chunks occupy the first ~200 bytes.
    let at = bytes.len() / 2;
    for byte in &mut bytes[at..at + 300] {
        *byte ^= 0xA5;
    }
    fs::write(&input, bytes).unwrap();
    let message = |options: ConvertOptions| {
        let output = tmp.path().join("out.oct");
        convert_file(&input, &output, options)
            .unwrap_err()
            .to_string()
    };
    let serial = message(block_options(2));
    for ranges in [2, 3, 8] {
        assert_eq!(message(ranged(block_options(2), ranges)), serial);
    }
}
