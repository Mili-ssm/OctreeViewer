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

// --- decoded chunk-table entries (native parallel decompressor) ---

use laz::laszip::{ChunkTable, ChunkTableEntry, LazVlr};
use viewer_convert::{ConvertError, ConvertOptions, convert_file};

/// Rewrites the chunk table of a 1.2 format-0 LAZ with `chunks` copies of one entry.
fn craft(variable: bool, point_count: u64, byte_count: u64, chunks: usize) -> Vec<u8> {
    let mut bytes = write_laz((1, 2), 0, None, 200);
    let data_offset = u32::from_le_bytes(bytes[96..100].try_into().unwrap()) as usize;
    let table = i64::from_le_bytes(bytes[data_offset..data_offset + 8].try_into().unwrap());
    let record = 227 + 54;
    if variable {
        bytes[record + 12..record + 16].copy_from_slice(&u32::MAX.to_le_bytes());
    }
    let length = u16::from_le_bytes(bytes[227 + 20..227 + 22].try_into().unwrap()) as usize;
    let vlr = LazVlr::read_from(&bytes[record..record + length]).unwrap();
    let mut entries = ChunkTable::with_capacity(chunks);
    for _ in 0..chunks {
        entries.push(ChunkTableEntry {
            point_count,
            byte_count,
        });
    }
    let mut encoded = Vec::new();
    entries.write_to(&mut encoded, &vlr).unwrap();
    bytes.truncate(table as usize);
    bytes.extend_from_slice(&encoded);
    bytes
}

fn convert_bytes(bytes: &[u8]) -> Result<(), ConvertError> {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("in.laz");
    std::fs::write(&input, bytes).unwrap();
    convert_file(
        &input,
        &dir.path().join("out.oct"),
        ConvertOptions::default(),
    )
    .map(|_| ())
}

fn assert_rejected(bytes: Vec<u8>, field: &str) {
    let len = bytes.len() as u64;
    preflight_las(&mut Cursor::new(&bytes), len).expect("header preflight alone accepts it");
    match convert_bytes(&bytes) {
        Err(ConvertError::LazChunkTable { field: got, .. }) => assert_eq!(got, field),
        other => panic!("expected LazChunkTable({field}), got {other:?}"),
    }
}

#[test]
fn negative_variable_chunk_point_count_is_rejected() {
    assert_rejected(
        craft(true, u64::MAX, 10, 1), // i32 -1 sign-extended
        "chunk point_count",
    );
}

#[test]
fn negative_fixed_chunk_byte_count_is_rejected() {
    assert_rejected(craft(false, 50_000, u64::MAX, 1), "chunk byte_count");
}

#[test]
fn huge_fixed_chunk_byte_count_is_rejected() {
    assert_rejected(craft(false, 50_000, i32::MAX as u64, 1), "chunk byte_count");
}

#[test]
fn las_crate_laz_output_still_converts() {
    for version in [(1, 2), (1, 4)] {
        for chunk_size in [None, Some(40), Some(0)] {
            convert_bytes(&write_laz(version, 3, chunk_size, 130)).unwrap();
        }
    }
}

#[test]
fn huge_fixed_chunk_size_is_rejected_by_header_preflight() {
    let mut bytes = write_laz((1, 2), 0, None, 200);
    bytes[227 + 54 + 12..227 + 54 + 16].copy_from_slice(&(u32::MAX - 1).to_le_bytes());
    let len = bytes.len() as u64;
    assert!(matches!(
        preflight_las(&mut Cursor::new(&bytes), len),
        Err(viewer_core::las::LasPreflightError::ChunkTooLarge { .. })
    ));
    assert!(matches!(
        convert_bytes(&bytes),
        Err(ConvertError::LasPreflight(_))
    ));
}

#[test]
fn variable_chunk_trusting_forged_header_count_is_rejected() {
    // Header point count u32::MAX makes `point_count <= header` true for i32::MAX.
    let mut bytes = craft(true, i32::MAX as u64, 10, 1);
    bytes[107..111].copy_from_slice(&u32::MAX.to_le_bytes());
    let len = bytes.len() as u64;
    preflight_las(&mut Cursor::new(&bytes), len).expect("header preflight alone accepts it");
    assert!(matches!(
        convert_bytes(&bytes),
        Err(ConvertError::LasPreflight(
            viewer_core::las::LasPreflightError::ChunkTooLarge { .. }
        ))
    ));
}

// Found by fuzzing: a table that decodes fewer points than the header declares made the
// parallel decompressor trip `debug_assert!(num_points >= requested)` in `laz`.
#[test]
fn variable_chunk_table_covering_fewer_points_than_declared_is_rejected() {
    assert_rejected(craft(true, 1, 10, 1), "chunk point_count total");
}

#[test]
fn fixed_chunk_table_covering_fewer_points_than_declared_is_rejected() {
    // 200 declared points, default chunk size 50_000: zero chunks cannot hold them.
    assert_rejected(craft(false, 0, 10, 0), "chunk point_count total");
}

// Found by fuzzing (fuzz_las_preflight). Both reached `laz` through the guarded path, so
// the header preflight (shared with the browser converter) and `convert_file` must reject them.
use viewer_core::las::LasPreflightError;

// Header record length 34 B, laszip items 146 B: `laz` asserted `out.len() % point_size == 0`.
const RECORD_LENGTH_MISMATCH: &[u8] = include_bytes!("fixtures/laz_record_length_mismatch.laz");
// LAS 1.4 format 8 layered chunk declaring a 4 GiB layer: `laz` allocated it unchecked.
const LAYER_SIZE_OOM: &[u8] = include_bytes!("fixtures/laz_layer_size_oom.laz");

#[test]
fn laz_record_length_differing_from_laszip_items_is_rejected() {
    let len = RECORD_LENGTH_MISMATCH.len() as u64;
    assert!(matches!(
        preflight_las(&mut Cursor::new(RECORD_LENGTH_MISMATCH), len),
        Err(LasPreflightError::LazRecordLengthMismatch { .. })
    ));
    assert!(matches!(
        convert_bytes(RECORD_LENGTH_MISMATCH),
        Err(ConvertError::LasPreflight(
            LasPreflightError::LazRecordLengthMismatch { .. }
        ))
    ));
}

#[test]
fn laz_layer_size_beyond_its_chunk_is_rejected() {
    let len = LAYER_SIZE_OOM.len() as u64;
    assert!(matches!(
        preflight_las(&mut Cursor::new(LAYER_SIZE_OOM), len),
        Err(LasPreflightError::LazLayerExceedsChunk { .. })
    ));
    assert!(matches!(
        convert_bytes(LAYER_SIZE_OOM),
        Err(ConvertError::LasPreflight(
            LasPreflightError::LazLayerExceedsChunk { .. }
        ))
    ));
}
