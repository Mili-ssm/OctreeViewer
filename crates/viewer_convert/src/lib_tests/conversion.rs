//! Conversion basics: attributes, failures and cancellation.

use super::*;

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
#[ignore = "issue #114: a cloud whose bounds are a single point cannot be converted to V8"]
fn a_single_point_converts() {
    let tmp = test_dir();
    let input = tmp.path().join("one.las");
    fixture(&input, false, false, 1);
    let output = tmp.path().join("one.oct");
    let report = convert_file(&input, &output, ConvertOptions::default()).unwrap();
    assert_eq!(report.point_count, 1);
}

#[test]
fn converts_laz_in_bounded_blocks_and_preserves_intensity_and_rgb16() {
    let tmp = test_dir();
    let dir = tmp.path().to_owned();
    let input = dir.join("source.laz");
    let output = dir.join("result.oct");
    fixture(&input, true, true, 5);
    let mut progress = Vec::new();
    let report = convert_file_with_progress(&input, &output, leaf_new(2, 0.01).unwrap(), |event| {
        progress.push(event);
        true
    })
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
        let source_position = [
            f64::from(index) * 0.25,
            f64::from(index % 3),
            f64::from(index % 5),
        ];
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
        // V8 stores only some columns in LOD blocks; the full set is in leaf blocks.
        if !has_attribute(&block, "user_data") {
            continue;
        }
        let intensity = attribute_values(&block, "intensity");
        let AttributeValues::U16(values) = intensity else {
            panic!("intensity must remain u16");
        };
        let AttributeValues::U8(classification) = attribute_values(&block, "classification") else {
            panic!("classification must remain u8");
        };
        assert!(classification.iter().all(|value| *value == 2));
        let AttributeValues::U8(return_numbers) = attribute_values(&block, "return_number") else {
            panic!("return number must remain u8");
        };
        assert!(return_numbers.iter().all(|value| *value == 2));
        let AttributeValues::U8(number_of_returns) = attribute_values(&block, "number_of_returns")
        else {
            panic!("number of returns must remain u8");
        };
        assert!(number_of_returns.iter().all(|value| *value == 3));
        let AttributeValues::U8(user_data) = attribute_values(&block, "user_data") else {
            panic!("user data must remain u8");
        };
        let AttributeValues::U16(source_ids) = attribute_values(&block, "point_source_id") else {
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
    let report = convert_file(&input, &output, leaf_new(2, 0.01).unwrap()).unwrap();

    let mut reader = FormatReader::new(fs::File::open(&output).unwrap()).unwrap();
    reader
        .attach_attributes(fs::File::open(output_attributes_path(&output)).unwrap())
        .unwrap();
    let mut found_last_point = false;
    for block_number in 0..report.block_count {
        let block = reader.read_block(BlockId::new(block_number)).unwrap();
        let AttributeValues::U16(intensity) = attribute_values(&block, "intensity") else {
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
    let mut gps = gps.clone();
    gps.sort_by(f64::total_cmp);
    assert_eq!(gps, [123.5, 124.5]);
    let AttributeValues::U16(nir) = value("nir") else {
        panic!("NIR must remain u16");
    };
    let mut nir = nir.clone();
    nir.sort_unstable();
    assert_eq!(nir, [0xBEEF, 0xCAFE]);
    let AttributeValues::U16(rgb) = value("rgb") else {
        panic!("RGB must remain u16");
    };
    assert_eq!(rgb, &[0x1200, 0xAB00, 0xFF00, 0x1200, 0xAB00, 0xFF00]);
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
    let options = leaf_budget(2, 0.01, 1).unwrap();

    assert!(matches!(
        convert_file(&input, &output, options),
        Err(ConvertError::MemoryBudgetExceeded { limit: 1, .. })
    ));
    assert!(!output.exists());
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
}

#[test]
#[ignore = "issue #114: identical positions with small leaves fail in the V8 paged index"]
fn identical_positions_remain_full_resolution_without_deep_empty_splits() {
    let tmp = test_dir();
    let dir = tmp.path().to_owned();
    let input = dir.join("identical.las");
    let output = dir.join("result.oct");
    coincident_fixture(&input);

    let report = convert_file(&input, &output, leaf_new(2, 0.01).unwrap()).unwrap();
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
        let AttributeValues::U16(values) = attribute_values(&block, "intensity") else {
            panic!("intensity must remain u16");
        };
        intensities.extend_from_slice(values);
    }
    intensities.sort_unstable();
    assert_eq!(intensities, [1, 2, 3, 4, 5]);
}

fn assert_spools_are_released_before_validation(shared_spool: bool) {
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
        let options = leaf_new(2, 0.01)
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

#[test]
fn lod_spools_are_released_before_validation() {
    assert_spools_are_released_before_validation(false);
}

#[test]
#[ignore = "issue #114: coincident points with small leaves fail in the V8 paged index"]
fn shared_leaf_spools_are_released_before_validation() {
    assert_spools_are_released_before_validation(true);
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
            [0.051, 0.051, 0.051],
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

    convert_file(&input, &output, leaf_new(1, 0.01).unwrap()).unwrap();
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
    let AttributeValues::U16(intensity) = attribute_values(&root_sample, "intensity") else {
        panic!("intensity must remain u16");
    };
    assert_eq!(intensity[averaged_sample], 20);
    // V8 keeps only the columns the viewer draws in LOD blocks: the source ID and the RGB
    // column are not stored there (the display colour above is the voxel average).
    assert!(!has_attribute(&root_sample, "point_source_id"));
    assert!(!has_attribute(&root_sample, "rgb"));
}

#[test]
fn cancellation_removes_the_temporary_file_without_publishing_output() {
    let tmp = test_dir();
    let dir = tmp.path().to_owned();
    let input = dir.join("source.las");
    let output = dir.join("result.oct");
    fixture(&input, false, false, 5);
    let mut cancelled = false;
    let result = convert_file_with_progress(&input, &output, leaf_new(2, 0.01).unwrap(), |event| {
        if event.stage == ConversionStage::ReadingAndWritingBlocks && event.points_processed >= 2 {
            cancelled = true;
            false
        } else {
            true
        }
    });

    assert!(cancelled);
    assert!(matches!(
        result,
        Err(ConvertError::Cancelled {
            points_processed: 2
        })
    ));
    assert!(!output.exists());
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);

    let rebuilt = convert_file(&input, &output, leaf_new(2, 0.01).unwrap())
        .expect("a cancelled conversion can be rebuilt from its source");
    assert_eq!(rebuilt.point_count, 5);
    assert!(output.exists());
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
        assert!(data.attributes().is_empty(), "{:?}", data.attributes());
        assert!(
            data.points()
                .points()
                .iter()
                .all(|p| p.color == viewer_core::ColorRgb::new(160, 160, 160))
        );
    }
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
