use std::io::Cursor;

use viewer_core::{BlockId, DatasetSource};
use viewer_format::FormatReader;

#[test]
fn web_host_sample_is_a_readable_project_dataset() {
    let bytes = include_bytes!("../../../examples/web-host/sample.oct");
    let mut reader = FormatReader::new(Cursor::new(bytes.as_slice())).unwrap();

    assert_eq!(
        reader.metadata().hierarchy().stats().source_point_count,
        Some(4096)
    );
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
