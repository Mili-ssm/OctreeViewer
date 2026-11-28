//! Serial core against the native converter, hybrid store and serial validation.

use super::*;

pub(crate) struct SerialRun {
    geometry: Vec<u8>,
    attributes: Vec<u8>,
    plan: viewer_convert_core::PlanSummary,
}

pub(crate) fn run_serial(input: &std::path::Path, options: ConvertOptions) -> SerialRun {
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
        attributes: output.attributes.into_inner(),
        plan: output.plan,
    }
}

pub(crate) fn assert_serial_matches_native(
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

#[test]
fn serial_core_matches_native_on_the_small_las() {
    let input = workspace_path("data/samples/fs_e430_n4008_18.las");
    assert_serial_matches_native(&input, ConvertOptions::default(), false, "default");
    let options = leaf_new(4096, 0.01).unwrap();
    assert_serial_matches_native(&input, options, true, "4096");
}

#[test]
fn serial_core_matches_native_on_the_variable_chunk_laz() {
    let input = workspace_path("tools/laz_chunk_probe/fixtures/synthetic_variable_chunks.laz");
    let options = leaf_new(4096, 0.01).unwrap();
    let serial = assert_serial_matches_native(&input, options, true, "laz");
    println!(
        "{} {}",
        sha256_hex(&serial.geometry),
        sha256_hex(&serial.attributes)
    );
    // Pinned from V8 output in D4 Task 5c (replaces the V5 hashes).
    assert!(sha256_hex(&serial.geometry).starts_with("e93c2ca17a13"));
    assert!(sha256_hex(&serial.attributes).starts_with("cd523afaad63"));
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
        output.attributes.into_inner() == reference.attributes,
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
        let options = leaf_new(4096, 0.01).unwrap();
        let label = relative;
        let stats = assert_hybrid_matches_mem(&input, options, u64::MAX, label);
        assert_eq!(stats.spilled_spools, 0, "{label}");
        // A tiny budget and none at all both spill and still give the same bytes.
        for budget in [64 * 1024, 0] {
            let stats = assert_hybrid_matches_mem(&input, options, budget, label);
            assert!(stats.spilled_spools > 0, "{label}/{budget}");
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
            let options = leaf_new(64, 0.01)
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
        Cursor::new(run.attributes.as_slice()),
        &run.plan,
        &mut |_| true,
    )
}

#[test]
fn serial_validation_accepts_the_output_and_rejects_a_flipped_payload_byte_like_native() {
    let input = workspace_path("tools/laz_chunk_probe/fixtures/synthetic_variable_chunks.laz");
    let options = leaf_new(4096, 0.01).unwrap();
    let run = run_serial(&input, options);
    serial_validation(&run).unwrap();

    let tmp = test_dir();
    let native_validation = |run: &SerialRun| {
        let path = tmp.path().join("copy.oct");
        fs::write(&path, &run.geometry).unwrap();
        fs::write(output_attributes_path(&path), &run.attributes).unwrap();
        // V8 numbers blocks when the pages are written, so the ids come from the file, as in
        // the converter (the plan's ids are the planner's, not the final ones).
        let (block_ids, source_block_ids) = output_block_ids(&path).unwrap();
        validate_output_pair(
            &path,
            &output_attributes_path(&path),
            &OutputValidation {
                total_points: run.plan.point_count,
                block_ids: &block_ids,
                source_block_ids: &source_block_ids,
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
    let run = run_serial(&input, leaf_new(4096, 0.01).unwrap());
    let mut last = None;
    validate_output_serial(
        Cursor::new(run.geometry.as_slice()),
        Cursor::new(run.attributes.as_slice()),
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
        Cursor::new(run.attributes.as_slice()),
        &run.plan,
        &mut |_| false,
    )
    .unwrap_err();
    assert!(matches!(error, ConvertError::Cancelled { .. }));
}
