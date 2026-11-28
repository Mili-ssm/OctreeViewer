//! Conversion diagnostics and spool accounting.

use super::*;

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
    // Spool record sizes re-pinned for V8 in D4 Task 5c (one byte more per record than V5).
    assert_eq!(spools.source_payload_written_bytes, 24 * 34);
    assert_eq!(spools.source_payload_scanned_bytes, 0);
    assert_eq!(spools.child_payload_written_bytes, 0);
    assert_eq!(spools.child_payload_scanned_bytes, 0);
    assert_eq!(spools.lod_payload_written_bytes, 0);
    assert_eq!(
        diagnostics.block_stream_source_payload_read_bytes,
        Some(24 * 34)
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
fn codec_diagnostics_preserve_output_and_separate_winner_from_attempts() {
    let tmp = test_dir();
    let dir = tmp.path().to_owned();
    let input = dir.join("source.las");
    let baseline = dir.join("baseline.oct");
    let profiled = dir.join("profiled.oct");
    fixture(&input, false, true, 24);
    let options = leaf_new(8, 0.01).unwrap();
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
    let winners: u64 = candidates
        .iter()
        .map(|candidate| candidate.selected_winners)
        .sum();
    let attempts: u64 = candidates.iter().map(|candidate| candidate.attempts).sum();
    assert!(winners > 0);
    assert!(attempts > winners);
    assert!(diagnostics.occupancy_builds.is_some());
}

#[test]
fn diagnostics_count_partition_and_lod_spool_payloads() {
    let tmp = test_dir();
    let dir = tmp.path().to_owned();
    let input = dir.join("source.las");
    let serial_output = dir.join("serial.oct");
    let parallel_output = dir.join("parallel.oct");
    fixture(&input, false, false, 200);
    let options = leaf_new(16, 0.01).unwrap().with_spatial_workers(1).unwrap();

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
        Some(200 * 28)
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
    assert!(spools.child_payload_written_bytes >= 200 * 28);
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

#[test]
fn spool_diagnostics_account_written_and_live_bytes() {
    let tmp = test_dir();
    let dir = tmp.path().to_owned();
    let input = dir.join("source.las");
    fixture(&input, false, false, 4_000);
    let options = leaf_new(64, 0.01)
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
    let options = leaf_new(64, 0.01).unwrap();

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
        }
        // V8 nodes may take over several child spools of their parent (a node's input is the sum
        // of the spools it absorbs), so compare totals: the inputs of a node's children add up to
        // the spools it wrote. Re-derived for V8 in D4 Task 5c.
        for parent in &events {
            let inputs: u64 = events
                .iter()
                .filter(|event| event.parent_id == Some(parent.node_id))
                .map(|event| event.input_spool_bytes)
                .sum();
            assert_eq!(inputs, parent.child_spool_bytes.iter().sum::<u64>());
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
