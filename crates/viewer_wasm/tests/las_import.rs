use std::io::Cursor;

use las::{Builder, Color, Point, Writer, point::Format};
use viewer_wasm::{LasImportError, LasPointStream};

fn fixture(compressed: bool, colored: bool) -> Cursor<Vec<u8>> {
    let mut builder = Builder::from((1, 2));
    builder.point_format = Format::new(if colored { 2 } else { 0 }).unwrap();
    builder.point_format.is_compressed = compressed;
    let header = builder.into_header().unwrap();
    let mut writer = Writer::new(Cursor::new(Vec::new()), header).unwrap();

    for (index, x) in [1000.0, 1001.5, 1003.0].into_iter().enumerate() {
        let mut point = Point {
            x,
            y: 2000.0 + index as f64,
            z: 50.0 + index as f64,
            intensity: (index as u16 + 1) * 10_000,
            ..Point::default()
        };
        if colored {
            point.color = Some(Color::new(0x1200, 0xAB00, 0xFF00));
        }
        writer.write_point(point).unwrap();
    }

    writer.into_inner().unwrap()
}

#[test]
fn reads_las_in_bounded_batches_with_local_coordinates_and_rgb() {
    let mut stream = LasPointStream::new(fixture(false, true)).unwrap();
    assert_eq!(stream.total_points(), 3);
    assert_eq!(stream.origin(), [1000.0, 2000.0, 50.0]);

    let first = stream.read_next(2).unwrap();
    assert_eq!(first.positions, [0.0, 0.0, 0.0, 1.5, 1.0, 1.0]);
    assert_eq!(first.colors, [0x12, 0xAB, 0xFF, 0x12, 0xAB, 0xFF]);
    assert_eq!(first.processed_points, 2);
    assert_eq!(first.total_points, 3);
    assert!(!first.done);

    let last = stream.read_next(2).unwrap();
    assert_eq!(last.positions, [3.0, 2.0, 2.0]);
    assert_eq!(last.processed_points, 3);
    assert!(last.done);
    assert!(stream.read_next(2).unwrap().positions.is_empty());
}

#[test]
fn reads_laz_and_uses_intensity_when_rgb_is_absent() {
    let mut stream = LasPointStream::new(fixture(true, false)).unwrap();
    let chunk = stream.read_next(10).unwrap();

    assert_eq!(chunk.positions.len(), 9);
    assert_eq!(chunk.colors, [39, 39, 39, 78, 78, 78, 117, 117, 117]);
    assert_eq!(chunk.processed_points, 3);
    assert!(chunk.done);
}

#[test]
fn rejects_zero_sized_batches_without_consuming_points() {
    let mut stream = LasPointStream::new(fixture(false, false)).unwrap();
    assert!(matches!(
        stream.read_next(0),
        Err(LasImportError::EmptyBatch)
    ));
    assert_eq!(stream.processed_points(), 0);
    assert_eq!(stream.read_next(1).unwrap().processed_points, 1);
}
