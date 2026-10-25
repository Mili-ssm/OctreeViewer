use std::io::Cursor;

use viewer_core::{BlockId, DatasetSource};
use viewer_format::{FormatReader, FormatVersion, OctreeReader};

#[test]
fn web_host_sample_is_a_readable_project_dataset() {
    let bytes = include_bytes!("../../../examples/web-host/sample.oct");
    let mut reader = FormatReader::new(Cursor::new(bytes.as_slice())).unwrap();

    assert_eq!(
        reader.metadata().hierarchy().stats().source_point_count,
        Some(4096)
    );
    reader
        .attach_attributes(Cursor::new(
            include_bytes!("../../../examples/web-host/sample.octattrs").as_slice(),
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
