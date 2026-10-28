//! Oracle for `preflight_las`: files written by the `las` crate itself must pass
//! the preflight and open with `las::Reader`.

use std::io::Cursor;

use las::{Builder, LazParallelism, Point, Reader, Writer, WriterOptions, point::Format};
use viewer_core::las::preflight_las;

fn write_laz(version: (u8, u8), format: u8, chunk_size: Option<u32>, points: usize) -> Vec<u8> {
    let mut builder = Builder::from(version);
    builder.point_format = Format::new(format).unwrap();
    builder.point_format.is_compressed = true;
    if let Some(chunk_size) = chunk_size {
        let mut probe = builder.clone().into_header().unwrap();
        probe.add_laz_vlr().unwrap();
        let mut vlr = probe
            .vlrs()
            .iter()
            .find(|vlr| vlr.user_id == "laszip encoded")
            .unwrap()
            .clone();
        vlr.data[12..16].copy_from_slice(&chunk_size.to_le_bytes());
        builder.vlrs.push(vlr);
    }
    let header = builder.into_header().unwrap();
    let options = WriterOptions::default().with_laz_parallelism(LazParallelism::No);
    let mut writer = Writer::with_options(Cursor::new(Vec::new()), header, options).unwrap();
    for i in 0..points {
        let mut point = Point {
            x: i as f64,
            y: (i * 2) as f64,
            z: 1.0,
            ..Point::default()
        };
        if format != 0 {
            point.gps_time = Some(0.0);
        }
        if matches!(format, 3 | 7) {
            point.color = Some(las::Color::new(0, 0, 0));
        }
        writer.write_point(point).unwrap();
    }
    writer.close().unwrap();
    writer.into_inner().unwrap().into_inner()
}

fn assert_valid(bytes: Vec<u8>) {
    let len = bytes.len() as u64;
    let mut cursor = Cursor::new(bytes);
    preflight_las(&mut cursor, len).unwrap();
    assert_eq!(cursor.position(), 0);
    Reader::new(cursor).unwrap();
}

#[test]
fn las_crate_laz_output_passes_preflight_and_opens() {
    for version in [(1, 2), (1, 3), (1, 4)] {
        for format in [0, 3] {
            for chunk_size in [None, Some(40), Some(0)] {
                assert_valid(write_laz(version, format, chunk_size, 130));
            }
        }
    }
    assert_valid(write_laz((1, 4), 6, Some(40), 130));
    assert_valid(write_laz((1, 4), 7, None, 130));
}

#[test]
fn empty_laz_passes_preflight() {
    assert_valid(write_laz((1, 2), 0, None, 0));
    assert_valid(write_laz((1, 4), 6, Some(0), 0));
}
