use std::{fs, io::Write};

use las::{Builder, Color, Point, Writer, point::Format};
use viewer_core::{DatasetSource, HierarchySemantics, NodeId};
use viewer_format::{FormatReader, OctreeReader};

use super::*;

fn dir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn write_laz(path: &Path, position: [f64; 3]) {
    write_laz_points(path, &[position]);
}

fn write_laz_points(path: &Path, positions: &[[f64; 3]]) {
    let mut builder = Builder::from((1, 2));
    builder.point_format = Format::new(0).unwrap();
    builder.point_format.is_compressed = true;
    let mut writer = Writer::from_path(path, builder.into_header().unwrap()).unwrap();
    for position in positions {
        writer
            .write_point(Point {
                x: position[0],
                y: position[1],
                z: position[2],
                intensity: 0xAB00,
                classification: las::point::Classification::Ground,
                ..Point::default()
            })
            .unwrap();
    }
}

fn write_laz_with_rgb(path: &Path) {
    let mut builder = Builder::from((1, 2));
    builder.point_format = Format::new(2).unwrap();
    builder.point_format.is_compressed = true;
    let mut writer = Writer::from_path(path, builder.into_header().unwrap()).unwrap();
    writer
        .write_point(Point {
            x: 3.0,
            y: 3.0,
            z: 3.0,
            intensity: 0xAB00,
            color: Some(Color::new(0x1234, 0xABCD, 0xFEDC)),
            ..Point::default()
        })
        .unwrap();
}

fn write_manifest(root: &Path, data_type: &str, points: u64) {
    fs::write(
        root.join("ept.json"),
        format!(
            r#"{{"dataType":"{data_type}","hierarchyType":"json","points":{points},"bounds":[0,0,0,8,8,8],"schema":[{{"name":"X","type":"signed","size":4}},{{"name":"Y","type":"signed","size":4}},{{"name":"Z","type":"signed","size":4}},{{"name":"ScanAngleRank","type":"float","size":4}},{{"name":"GpsTime","type":"float","size":8}}]}}"#
        ),
    )
    .unwrap();
}

#[test]
fn schema_keeps_legacy_floating_alias_and_rejects_unknown_types() {
    for data_type in ["float", "floating", "unknown"] {
        let schema = serde_json::json!([
            { "name": "X", "type": "signed", "size": 4 },
            { "name": "Y", "type": "signed", "size": 4 },
            { "name": "Z", "type": "signed", "size": 4 },
            { "name": "GpsTime", "type": data_type, "size": 8 }
        ]);
        assert_eq!(validate_schema(&schema).is_ok(), data_type != "unknown");
    }
}

#[test]
fn converts_local_laszip_ept_pages_as_additive_hierarchy() {
    for version in [
        viewer_format::FormatVersion::V6_PRE1,
        viewer_format::FormatVersion::V6_PRE2,
        viewer_format::FormatVersion::V6_PRE3,
        viewer_format::FormatVersion::V6_PRE4,
        viewer_format::FormatVersion::V7_PRE1,
        viewer_format::FormatVersion::V7_PRE2,
        viewer_format::FormatVersion::V7_PRE3,
    ] {
        let tmp = dir();
        let root = tmp.path().to_owned();
        fs::create_dir_all(root.join("ept-data")).unwrap();
        fs::create_dir_all(root.join("ept-hierarchy")).unwrap();
        write_manifest(&root, "laszip", 3);
        fs::write(
            root.join("ept-hierarchy/0-0-0-0.json"),
            r#"{"0-0-0-0":1,"1-0-0-0":-1}"#,
        )
        .unwrap();
        fs::write(
            root.join("ept-hierarchy/1-0-0-0.json"),
            r#"{"1-0-0-0":1,"2-0-0-0":1}"#,
        )
        .unwrap();
        write_laz(&root.join("ept-data/0-0-0-0.laz"), [7.0, 7.0, 7.0]);
        write_laz(&root.join("ept-data/1-0-0-0.laz"), [3.0, 3.0, 3.0]);
        write_laz(&root.join("ept-data/2-0-0-0.laz"), [1.0, 1.0, 1.0]);

        let output = root.join("result.oct");
        let mut progress = Vec::new();
        let options = ConvertOptions::default()
            .with_format_version(version)
            .unwrap();
        let report = crate::convert_file_with_progress(&root, &output, options, |event| {
            progress.push(event);
            true
        })
        .unwrap();
        let pipelined_output = root.join("pipelined.oct");
        crate::convert_file(
            &root,
            &pipelined_output,
            options.with_pipelined_writes(true),
        )
        .unwrap();
        assert_eq!(
            fs::read(&output).unwrap(),
            fs::read(&pipelined_output).unwrap()
        );
        assert_eq!(
            fs::read(output_attributes_path(&output)).unwrap(),
            fs::read(output_attributes_path(&pipelined_output)).unwrap()
        );
        if matches!(
            version,
            viewer_format::FormatVersion::V7_PRE1
                | viewer_format::FormatVersion::V7_PRE2
                | viewer_format::FormatVersion::V7_PRE3
        ) {
            let diagnostic_output = root.join("diagnostic.oct");
            let (diagnostic_report, diagnostics) =
                crate::convert_file_with_diagnostics(&root, &diagnostic_output, options, |_| true)
                    .unwrap();
            assert_eq!(diagnostic_report.point_count, report.point_count);
            assert_eq!(
                fs::read(&output).unwrap(),
                fs::read(&diagnostic_output).unwrap()
            );
            assert_eq!(
                fs::read(output_attributes_path(&output)).unwrap(),
                fs::read(output_attributes_path(&diagnostic_output)).unwrap()
            );
            assert!(diagnostics.spatial_planning.is_some());
            assert!(diagnostics.block_stream.is_some());
            assert!(diagnostics.finalize_and_sync_elapsed_ns.is_some());
        }
        let constrained_options = ConvertOptions::with_memory_budget(
            options.points_per_block(),
            options.max_quantization_error(),
            report.estimated_peak_memory_bytes,
        )
        .unwrap()
        .with_format_version(version)
        .unwrap()
        .with_pipelined_writes(true);
        let constrained_output = root.join("constrained.oct");
        crate::convert_file(&root, &constrained_output, constrained_options).unwrap();
        assert_eq!(
            fs::read(&output).unwrap(),
            fs::read(&constrained_output).unwrap()
        );
        assert_eq!(
            fs::read(output_attributes_path(&output)).unwrap(),
            fs::read(output_attributes_path(&constrained_output)).unwrap()
        );
        if matches!(
            version,
            viewer_format::FormatVersion::V7_PRE1
                | viewer_format::FormatVersion::V7_PRE2
                | viewer_format::FormatVersion::V7_PRE3
        ) {
            let cancelled_output = root.join("cancelled.oct");
            let cancelled = crate::convert_file_with_progress(
                &root,
                &cancelled_output,
                options.with_pipelined_writes(true),
                |event| event.stage != ConversionStage::ReadingAndWritingBlocks,
            );
            assert!(matches!(cancelled, Err(ConvertError::Cancelled { .. })));
            assert!(!cancelled_output.exists());
            assert!(!output_attributes_path(&cancelled_output).exists());
        }
        assert_eq!(report.point_count, 3);
        assert_eq!(report.block_preparation, None);
        assert!(report.output_validation.worker_limit > 0);
        assert_eq!(report.block_count, 3);
        assert_eq!(
            progress.first().unwrap().stage,
            ConversionStage::InspectingInput
        );
        assert_eq!(progress.last().unwrap().stage, ConversionStage::Complete);

        let mut reader = FormatReader::new(File::open(&output).unwrap()).unwrap();
        assert_eq!(reader.format_version(), version);
        reader
            .attach_attributes(File::open(output_attributes_path(&output)).unwrap())
            .unwrap();
        let hierarchy = reader.metadata().hierarchy();
        assert_eq!(reader.format_metadata().visual_scale(), 12.5);
        assert_eq!(hierarchy.semantics(), HierarchySemantics::Additive);
        assert_eq!(hierarchy.stats().node_count, 3);
        assert_eq!(hierarchy.stats().source_point_count, Some(3));
        assert_eq!(hierarchy.node(NodeId::new(0)).unwrap().point_count(), 1);
        assert_eq!(hierarchy.node(NodeId::new(1)).unwrap().point_count(), 1);
        assert_eq!(hierarchy.node(NodeId::new(2)).unwrap().point_count(), 1);
        let block = reader.read_block(BlockId::new(2)).unwrap();
        assert_eq!(block.points().len(), 1);
        let viewer_core::AttributeValues::U16(intensity) = block.attributes()[0].values() else {
            panic!("EPT intensity must remain u16");
        };
        assert_eq!(intensity, &[0xAB00]);
        let viewer_core::AttributeValues::U8(classification) = block.attributes()[1].values()
        else {
            panic!("EPT classification must remain u8");
        };
        assert_eq!(classification, &[2]);
    }
}

#[test]
fn ept_checkpoint_resumes_within_laz_and_matches_clean_output() {
    let tmp = dir();
    let root = tmp.path().to_owned();
    fs::create_dir_all(root.join("ept-data")).unwrap();
    fs::create_dir_all(root.join("ept-hierarchy")).unwrap();
    write_manifest(&root, "laszip", 2);
    fs::write(root.join("ept-hierarchy/0-0-0-0.json"), r#"{"0-0-0-0":2}"#).unwrap();
    write_laz_points(
        &root.join("ept-data/0-0-0-0.laz"),
        &[[1.0, 1.0, 1.0], [2.0, 2.0, 2.0]],
    );
    let options = ConvertOptions::new(1, 0.01)
        .unwrap()
        .with_format_version(viewer_format::FormatVersion::V7_PRE1)
        .unwrap()
        .with_pipelined_writes(true);
    let output = root.join("resumed.oct");
    let stopped = crate::convert_file_with_checkpoint(
        &root,
        &output,
        options,
        CheckpointMode::Create,
        |progress| {
            !(progress.stage == ConversionStage::ReadingAndWritingBlocks
                && progress.points_processed == 1)
        },
    );
    assert!(matches!(
        stopped,
        Err(ConvertError::Cancelled {
            points_processed: 1
        })
    ));
    assert!(!output.exists());
    crate::convert_file_with_checkpoint(&root, &output, options, CheckpointMode::Resume, |_| true)
        .unwrap();

    let clean = root.join("clean.oct");
    crate::convert_file(&root, &clean, options).unwrap();
    assert_eq!(fs::read(&output).unwrap(), fs::read(&clean).unwrap());
    assert_eq!(
        fs::read(output_attributes_path(&output)).unwrap(),
        fs::read(output_attributes_path(&clean)).unwrap()
    );
    assert!(!output.with_extension("oct.checkpoint").exists());
}

#[test]
fn ept_v72_and_v73_checkpoints_resume_between_blocks_and_match_clean_output() {
    for (version, label, other_version) in [
        (
            viewer_format::FormatVersion::V7_PRE2,
            "v72",
            viewer_format::FormatVersion::V7_PRE3,
        ),
        (
            viewer_format::FormatVersion::V7_PRE3,
            "v73",
            viewer_format::FormatVersion::V7_PRE2,
        ),
    ] {
        let tmp = dir();
        let root = tmp.path().to_owned();
        fs::create_dir_all(root.join("ept-data")).unwrap();
        fs::create_dir_all(root.join("ept-hierarchy")).unwrap();
        write_manifest(&root, "laszip", 2);
        fs::write(root.join("ept-hierarchy/0-0-0-0.json"), r#"{"0-0-0-0":2}"#).unwrap();
        write_laz_points(
            &root.join("ept-data/0-0-0-0.laz"),
            &[[1.0, 1.0, 1.0], [2.0, 2.0, 2.0]],
        );
        let options = ConvertOptions::new(1, 0.01)
            .unwrap()
            .with_format_version(version)
            .unwrap();
        let output = root.join(format!("resumed-{label}.oct"));
        let stopped = crate::convert_file_with_checkpoint(
            &root,
            &output,
            options,
            CheckpointMode::Create,
            |progress| {
                !(progress.stage == ConversionStage::ReadingAndWritingBlocks
                    && progress.points_processed == 1)
            },
        );
        assert!(matches!(
            stopped,
            Err(ConvertError::Cancelled {
                points_processed: 1
            })
        ));
        let checkpoint_root = output.with_extension("oct.checkpoint");
        let candidate = checkpoint_root.join("candidate.oct");
        let prefix_before = fs::read(&candidate).unwrap();
        let candidate_attributes = checkpoint_root.join("candidate.octattrs");
        let attributes_before = fs::read(&candidate_attributes).unwrap();
        let wrong_revision = options.with_format_version(other_version).unwrap();
        assert!(matches!(
            crate::convert_file_with_checkpoint(
                &root,
                &output,
                wrong_revision,
                CheckpointMode::Resume,
                |_| true,
            ),
            Err(ConvertError::Checkpoint(crate::CheckpointError::Mismatch))
        ));
        assert_eq!(fs::read(&candidate).unwrap(), prefix_before);
        assert_eq!(fs::read(&candidate_attributes).unwrap(), attributes_before);
        crate::convert_file_with_checkpoint(
            &root,
            &output,
            options,
            CheckpointMode::Resume,
            |_| true,
        )
        .unwrap();

        let clean = root.join(format!("clean-{label}.oct"));
        crate::convert_file(&root, &clean, options).unwrap();
        assert_eq!(fs::read(&output).unwrap(), fs::read(&clean).unwrap());
        assert_eq!(
            fs::read(output_attributes_path(&output)).unwrap(),
            fs::read(output_attributes_path(&clean)).unwrap()
        );
    }
}

#[test]
fn ept_checkpoint_recovers_after_output_validation_before_publication() {
    let tmp = dir();
    let root = tmp.path().to_owned();
    fs::create_dir_all(root.join("ept-data")).unwrap();
    fs::create_dir_all(root.join("ept-hierarchy")).unwrap();
    write_manifest(&root, "laszip", 1);
    fs::write(root.join("ept-hierarchy/0-0-0-0.json"), r#"{"0-0-0-0":1}"#).unwrap();
    write_laz(&root.join("ept-data/0-0-0-0.laz"), [1.0, 1.0, 1.0]);
    let options = ConvertOptions::default()
        .with_format_version(viewer_format::FormatVersion::V7_PRE1)
        .unwrap();
    let output = root.join("recovered.oct");
    let stopped = crate::convert_file_with_checkpoint(
        &root,
        &output,
        options,
        CheckpointMode::Create,
        |progress| progress.stage != ConversionStage::PublishingOutput,
    );
    assert!(matches!(stopped, Err(ConvertError::Cancelled { .. })));
    assert!(!output.exists());
    assert!(output.with_extension("oct.checkpoint").exists());

    crate::convert_file_with_checkpoint(&root, &output, options, CheckpointMode::Resume, |_| true)
        .unwrap();
    assert!(output.exists());
    assert!(output_attributes_path(&output).exists());
    assert!(!output.with_extension("oct.checkpoint").exists());
}

#[test]
fn ept_checkpoint_rejects_changed_source_metadata_without_touching_prefix() {
    let tmp = dir();
    let root = tmp.path().to_owned();
    fs::create_dir_all(root.join("ept-data")).unwrap();
    fs::create_dir_all(root.join("ept-hierarchy")).unwrap();
    write_manifest(&root, "laszip", 2);
    fs::write(root.join("ept-hierarchy/0-0-0-0.json"), r#"{"0-0-0-0":2}"#).unwrap();
    let source = root.join("ept-data/0-0-0-0.laz");
    write_laz_points(&source, &[[1.0, 1.0, 1.0], [2.0, 2.0, 2.0]]);
    let options = ConvertOptions::new(1, 0.01)
        .unwrap()
        .with_format_version(viewer_format::FormatVersion::V7_PRE1)
        .unwrap()
        .with_pipelined_writes(true);
    let output = root.join("mismatch.oct");
    let stopped = crate::convert_file_with_checkpoint(
        &root,
        &output,
        options,
        CheckpointMode::Create,
        |progress| {
            !(progress.stage == ConversionStage::ReadingAndWritingBlocks
                && progress.points_processed == 1)
        },
    );
    assert!(matches!(stopped, Err(ConvertError::Cancelled { .. })));
    let checkpoint_root = output.with_extension("oct.checkpoint");
    let candidate = checkpoint_root.join("candidate.oct");
    let candidate_attributes = checkpoint_root.join("candidate.octattrs");
    let primary_before = fs::read(&candidate).unwrap();
    let attributes_before = fs::read(&candidate_attributes).unwrap();
    OpenOptions::new()
        .append(true)
        .open(&source)
        .unwrap()
        .write_all(&[0])
        .unwrap();

    assert!(matches!(
        crate::convert_file_with_checkpoint(
            &root,
            &output,
            options,
            CheckpointMode::Resume,
            |_| true,
        ),
        Err(ConvertError::Checkpoint(crate::CheckpointError::Mismatch))
    ));
    assert_eq!(fs::read(&candidate).unwrap(), primary_before);
    assert_eq!(fs::read(&candidate_attributes).unwrap(), attributes_before);
    fs::remove_dir_all(checkpoint_root).unwrap();
}

#[test]
fn v7_ept_rgb_conversion_preserves_display_and_typed_values() {
    let tmp = dir();
    let root = tmp.path().to_owned();
    fs::create_dir_all(root.join("ept-data")).unwrap();
    fs::create_dir_all(root.join("ept-hierarchy")).unwrap();
    fs::write(
        root.join("ept.json"),
        r#"{"dataType":"laszip","hierarchyType":"json","points":1,"bounds":[0,0,0,8,8,8],"schema":[{"name":"X","type":"signed","size":4},{"name":"Y","type":"signed","size":4},{"name":"Z","type":"signed","size":4},{"name":"Red","type":"unsigned","size":2},{"name":"Green","type":"unsigned","size":2},{"name":"Blue","type":"unsigned","size":2}]}"#,
    )
    .unwrap();
    fs::write(root.join("ept-hierarchy/0-0-0-0.json"), r#"{"0-0-0-0":1}"#).unwrap();
    write_laz_with_rgb(&root.join("ept-data/0-0-0-0.laz"));

    let output = root.join("result.oct");
    crate::convert_file(
        &root,
        &output,
        ConvertOptions::default()
            .with_format_version(viewer_format::FormatVersion::V7_PRE1)
            .unwrap(),
    )
    .unwrap();
    let mut reader = FormatReader::new(File::open(&output).unwrap()).unwrap();
    reader
        .attach_attributes(File::open(output_attributes_path(&output)).unwrap())
        .unwrap();
    assert_eq!(
        reader.read_display_colors(BlockId::new(0)).unwrap(),
        vec![ColorRgb::new(0x12, 0xAB, 0xFE)]
    );
    let block = reader.read_block(BlockId::new(0)).unwrap();
    let rgb = block
        .attributes()
        .iter()
        .find(|column| column.descriptor().name() == "rgb")
        .unwrap();
    let AttributeValues::U16(rgb) = rgb.values() else {
        panic!("EPT RGB must remain u16");
    };
    assert_eq!(rgb, &[0x1234, 0xABCD, 0xFEDC]);
}

#[test]
fn rejects_unsupported_ept_codec_before_reading_hierarchy() {
    let tmp = dir();
    let root = tmp.path().to_owned();
    write_manifest(&root, "zstandard", 0);
    assert!(matches!(
        convert(
            &root,
            &root.join("result.oct"),
            ConvertOptions::default(),
            None,
            &mut |_| true,
            None,
        ),
        Err(ConvertError::UnsupportedEpt(message)) if message.contains("laszip")
    ));
}

#[test]
fn rejects_checkpoint_and_repartition_options_for_ept() {
    let tmp = dir();
    let root = tmp.path().to_owned();
    write_manifest(&root, "laszip", 0);
    let output = root.join("result.oct");
    assert!(matches!(
        crate::convert_file_with_checkpoint(
            &root,
            &output,
            ConvertOptions::default(),
            crate::CheckpointMode::Create,
            |_| true,
        ),
        Err(ConvertError::Checkpoint(crate::CheckpointError::Invalid(message)))
            if message.contains("v7.1-pre")
    ));
    let repartitioned = ConvertOptions::default().with_spatial_divisions(4).unwrap();
    assert!(matches!(
        convert(&root, &output, repartitioned, None, &mut |_| true, None),
        Err(ConvertError::UnsupportedEpt(message)) if message.contains("topology")
    ));
    assert!(!output.exists());
}

#[test]
fn rejects_ept_count_mismatch_and_missing_node_files() {
    let tmp = dir();
    let root = tmp.path().to_owned();
    fs::create_dir_all(root.join("ept-hierarchy")).unwrap();
    write_manifest(&root, "laszip", 2);
    fs::write(root.join("ept-hierarchy/0-0-0-0.json"), r#"{"0-0-0-0":1}"#).unwrap();
    assert!(matches!(
        convert(
            &root,
            &root.join("result.oct"),
            ConvertOptions::default(),
            None,
            &mut |_| true,
            None,
        ),
        Err(ConvertError::InvalidEpt(message)) if message.contains("sums to 1")
    ));

    write_manifest(&root, "laszip", 1);
    assert!(matches!(
        convert(
            &root,
            &root.join("result.oct"),
            ConvertOptions::default(),
            None,
            &mut |_| true,
            None,
        ),
        Err(ConvertError::InvalidEpt(message)) if message.contains("is missing")
    ));
}

#[test]
fn rejects_ept_memory_budget_before_planning_all_nodes_or_creating_output() {
    let tmp = dir();
    let root = tmp.path().to_owned();
    fs::create_dir_all(root.join("ept-data")).unwrap();
    fs::create_dir_all(root.join("ept-hierarchy")).unwrap();
    write_manifest(&root, "laszip", 2);
    fs::write(
        root.join("ept-hierarchy/0-0-0-0.json"),
        r#"{"0-0-0-0":1,"1-0-0-0":1}"#,
    )
    .unwrap();
    write_laz(&root.join("ept-data/0-0-0-0.laz"), [7.0, 7.0, 7.0]);
    let output = root.join("result.oct");
    let options = ConvertOptions::with_memory_budget(1, 0.01, 1).unwrap();

    assert!(matches!(
        convert(&root, &output, options, None, &mut |_| true, None),
        Err(ConvertError::MemoryBudgetExceeded { limit: 1, .. })
    ));
    assert!(!output.exists());
    assert!(!output_attributes_path(&output).exists());
}

#[test]
fn rejects_ept_memory_budget_while_reading_large_hierarchy_page() {
    let tmp = dir();
    let root = tmp.path().to_owned();
    fs::create_dir_all(root.join("ept-hierarchy")).unwrap();
    write_manifest(&root, "laszip", 0);
    let mut page = serde_json::Map::new();
    page.insert("0-0-0-0".to_owned(), serde_json::json!(0));
    for index in 0..1_500_u32 {
        let x = index % 32;
        let y = (index / 32) % 32;
        let z = index / 1_024;
        page.insert(format!("5-{x}-{y}-{z}"), serde_json::json!(-1));
    }
    fs::write(
        root.join("ept-hierarchy/0-0-0-0.json"),
        serde_json::to_vec(&serde_json::Value::Object(page)).unwrap(),
    )
    .unwrap();
    let output = root.join("result.oct");
    let options = ConvertOptions::with_memory_budget(1, 0.01, 17 * 1024 * 1024).unwrap();

    assert!(matches!(
        convert(&root, &output, options, None, &mut |_| true, None),
        Err(ConvertError::MemoryBudgetExceeded {
            limit: 17_825_792,
            ..
        })
    ));
    assert!(!output.exists());
    assert!(!output_attributes_path(&output).exists());
}

fn assert_rejected_by_size_cap(root: &Path) {
    let output = root.join("result.oct");
    let options = ConvertOptions::with_memory_budget(1, 0.01, 1 << 30).unwrap();
    match convert(root, &output, options, None, &mut |_| true, None) {
        Err(ConvertError::InvalidEpt(message)) => {
            assert!(message.contains("exceeding"), "{message}");
        }
        other => panic!("expected a size-cap rejection, got {other:?}"),
    }
    assert!(!output.exists());
}

#[test]
fn rejects_oversized_manifest_before_parsing() {
    let tmp = dir();
    let root = tmp.path().to_owned();
    // Sparse and not valid JSON: only a size check can produce the "exceeding" message.
    fs::File::create(root.join("ept.json"))
        .unwrap()
        .set_len(MAX_EPT_MANIFEST_BYTES + 1)
        .unwrap();
    assert_rejected_by_size_cap(&root);
}

#[test]
fn rejects_oversized_hierarchy_page_before_parsing() {
    let tmp = dir();
    let root = tmp.path().to_owned();
    fs::create_dir_all(root.join("ept-hierarchy")).unwrap();
    write_manifest(&root, "laszip", 0);
    fs::File::create(root.join("ept-hierarchy/0-0-0-0.json"))
        .unwrap()
        .set_len(MAX_EPT_HIERARCHY_PAGE_BYTES + 1)
        .unwrap();
    assert_rejected_by_size_cap(&root);
}
