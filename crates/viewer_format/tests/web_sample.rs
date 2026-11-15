use std::io::Cursor;

use viewer_core::{BlockId, DatasetSource, HierarchySemantics};
use viewer_format::{FormatReader, FormatVersion, OctreeReader};

#[test]
fn web_host_legacy_v5_sample_is_a_readable_project_dataset() {
    let bytes = include_bytes!("../../../examples/web-host/sample-v5.oct");
    let mut reader = FormatReader::new(Cursor::new(bytes.as_slice())).unwrap();

    assert_eq!(
        reader.metadata().hierarchy().stats().source_point_count,
        Some(4096)
    );
    reader
        .attach_attributes(Cursor::new(
            include_bytes!("../../../examples/web-host/sample-v5.octattrs").as_slice(),
        ))
        .unwrap();
    assert_eq!(reader.attribute_range(0, 0).unwrap(), Some([0.0, 65016.0]));
    assert_eq!(reader.attribute_range(1, 0).unwrap(), Some([0.0, 3.0]));
    let intensity = reader.read_attribute(BlockId::new(1), 0).unwrap();
    assert_eq!(intensity.descriptor().name(), "intensity");
    assert_eq!(intensity.values().len(), 4096);
    let block = reader.read_block(BlockId::new(1)).unwrap();
    assert_eq!(block.points().len(), 4096);
    assert_eq!(block.id(), BlockId::new(1));
    assert!(block.points().points().iter().all(|point| {
        point
            .position
            .iter()
            .all(|coordinate| coordinate.is_finite())
    }));
}

#[test]
fn web_host_v7_sample_is_a_readable_split_dataset() {
    let bytes = include_bytes!("../../../examples/web-host/sample-v7.oct");
    let mut reader = FormatReader::new(Cursor::new(bytes.as_slice())).unwrap();

    assert_eq!(reader.format_version(), FormatVersion::V7_PRE1);
    reader
        .attach_attributes(Cursor::new(
            include_bytes!("../../../examples/web-host/sample-v7.octattrs").as_slice(),
        ))
        .unwrap();
    assert_eq!(reader.attribute_range(0, 0).unwrap(), Some([0.0, 65016.0]));
    assert_eq!(reader.attribute_range(1, 0).unwrap(), Some([0.0, 3.0]));
    let block = reader.read_block(BlockId::new(1)).unwrap();
    assert_eq!(block.points().len(), 4096);
    assert_eq!(block.id(), BlockId::new(1));
    assert!(block.points().points().iter().all(|point| {
        point
            .position
            .iter()
            .all(|coordinate| coordinate.is_finite())
    }));
}

#[test]
fn web_host_default_sample_is_a_v7_dataset_with_attributes() {
    let bytes = include_bytes!("../../../examples/web-host/sample.oct");
    let mut reader = FormatReader::new(Cursor::new(bytes.as_slice())).unwrap();

    assert_eq!(reader.format_version(), FormatVersion::V7);
    // 768 x 768 grid at 64/768 m spacing, see `viewer_convert/examples/create_web_sample.rs`.
    assert_eq!(
        reader.metadata().hierarchy().stats().source_point_count,
        Some(768 * 768)
    );
    reader
        .attach_attributes(Cursor::new(
            include_bytes!("../../../examples/web-host/sample.octattrs").as_slice(),
        ))
        .unwrap();
    // intensity = (x * 1000 + y * 8) as u16 with x, y in 0..=63.9167.
    assert_eq!(reader.attribute_range(0, 0).unwrap(), Some([0.0, 64428.0]));
    assert_eq!(reader.attribute_range(1, 0).unwrap(), Some([0.0, 3.0]));
    // The conditions of the GPU cells path (`LodDataset::supports_cells`).
    assert!(reader.format_metadata().global_frame().is_some());
    assert_eq!(
        reader.metadata().hierarchy().semantics(),
        HierarchySemantics::Replacement
    );
    let blocks = reader.metadata().hierarchy().block_ids().to_vec();
    assert!(!blocks.is_empty());
    for id in blocks {
        let block = reader.read_block(id).unwrap();
        assert!(block.points().points().iter().all(|point| {
            point
                .position
                .iter()
                .all(|coordinate| coordinate.is_finite())
        }));
    }
    // Replacement semantics: every source point lives in exactly one leaf, so the leaves add up
    // to the generated count (summing all blocks would also count the LOD parents).
    let hierarchy = reader.metadata().hierarchy();
    let mut stack = vec![hierarchy.root_id()];
    let mut visited = 0;
    let mut leaf_points = 0;
    while let Some(id) = stack.pop() {
        visited += 1;
        if hierarchy.is_leaf(id) {
            leaf_points += hierarchy.node(id).unwrap().point_count();
        }
        stack.extend_from_slice(hierarchy.child_ids(id));
    }
    assert_eq!(visited, hierarchy.stats().node_count);
    assert_eq!(leaf_points, 768 * 768);
}
