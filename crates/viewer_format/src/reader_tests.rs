use std::io::Cursor;

use viewer_core::BlockId;

use crate::FormatReader;
use crate::test_support::{POINTS, v8_pair};

#[test]
fn v8_pair_reads_back_every_position() {
    let (primary, attributes) = v8_pair();
    let mut reader = FormatReader::new(Cursor::new(primary)).unwrap();
    reader.attach_attributes(Cursor::new(attributes)).unwrap();
    let points = reader.read_positions(BlockId::new(0)).unwrap();
    assert_eq!(points.points().len(), POINTS.len());
}
