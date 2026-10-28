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
            user_data: index as u8 + 7,
            point_source_id: 100 + index as u16,
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
