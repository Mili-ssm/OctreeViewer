//! Ranges decoded from chunk offsets must equal the sequential read, for every worker count.

use std::io::{Cursor, Seek};
use std::ops::Range;
use std::path::{Path, PathBuf};

use laz::laszip::{LasZipCompressor, LasZipDecompressor, LazItemType, LazVlrBuilder};
use laz_chunk_probe::{
    ChunkIndex, OpenedLaz, balanced_split, decode_chunk_range, decode_sequential,
};

const SYNTHETIC_CHUNKS: [usize; 6] = [7, 13, 5, 11, 50_000, 3];
const POINT10: usize = 20;

/// Every way to cut `0..chunks` into `parts` non-empty contiguous ranges.
fn all_splits(chunks: usize, parts: usize) -> Vec<Vec<Range<usize>>> {
    fn rec(
        start: usize,
        chunks: usize,
        parts: usize,
        acc: &mut Vec<Range<usize>>,
        out: &mut Vec<Vec<Range<usize>>>,
    ) {
        if parts == 1 {
            acc.push(start..chunks);
            out.push(acc.clone());
            acc.pop();
            return;
        }
        for end in start + 1..=chunks - (parts - 1) {
            acc.push(start..end);
            rec(end, chunks, parts - 1, acc, out);
            acc.pop();
        }
    }
    let mut out = Vec::new();
    rec(0, chunks, parts, &mut Vec::new(), &mut out);
    out
}

/// Deterministic cuts that are not aligned to equal chunk counts.
fn lcg_split(chunks: usize, parts: usize, seed: u64) -> Vec<Range<usize>> {
    let mut state = seed;
    let mut cuts: Vec<usize> = (1..parts)
        .map(|_| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            1 + (state >> 33) as usize % (chunks - 1)
        })
        .collect();
    cuts.sort_unstable();
    cuts.dedup();
    let mut bounds = vec![0];
    bounds.extend(cuts);
    bounds.push(chunks);
    bounds.windows(2).map(|pair| pair[0]..pair[1]).collect()
}

fn assert_split_matches<R: std::io::Read + Seek>(
    src: &mut R,
    index: &ChunkIndex,
    expected: &[u8],
    split: &[Range<usize>],
) {
    let mut joined = Vec::new();
    for range in split {
        decode_chunk_range(src, index, range.clone(), &mut joined).unwrap();
    }
    assert!(
        joined == expected,
        "split {split:?} differs from the sequential read"
    );
}

fn data_file(name: &str) -> Option<PathBuf> {
    let home = std::env::var("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(|home| Path::new(&home).join(".cargo")))
        .ok()?;
    let pattern = format!(
        "{}/registry/src/*/las-0.11.2/tests/data/{name}",
        home.display()
    );
    glob_first(&pattern)
}

/// Minimal `*` expansion of one path component, to avoid a glob dependency.
fn glob_first(pattern: &str) -> Option<PathBuf> {
    let (head, tail) = pattern.split_once("/*/")?;
    let mut found: Vec<PathBuf> = std::fs::read_dir(head)
        .ok()?
        .flatten()
        .map(|entry| entry.path().join(tail))
        .filter(|path| path.exists())
        .collect();
    found.sort();
    found.pop()
}

/// Decodes the first `limit` chunks of `path` sequentially and by range for W = 1..=8.
fn check_file(path: &Path, limit: usize, min_chunks: usize) {
    let opened = OpenedLaz::open(path).unwrap();
    let chunks = opened.index.table.len().min(limit);
    assert!(
        chunks >= min_chunks,
        "need at least {min_chunks} chunks, got {chunks}"
    );
    let points = opened.points_in(chunks);
    let mut expected = Vec::new();
    decode_sequential(
        std::fs::File::open(path).unwrap(),
        &opened.index.vlr,
        opened.point_data_offset,
        points,
        &mut expected,
    )
    .unwrap();
    assert_eq!(
        expected.len() as u64,
        points * opened.index.record_size() as u64
    );
    let mut file = std::fs::File::open(path).unwrap();
    for parts in 1..=chunks.min(8) {
        let balanced = balanced_split(chunks, parts);
        assert_split_matches(&mut file, &opened.index, &expected, &balanced);
        let uneven = lcg_split(chunks, parts, parts as u64);
        assert_split_matches(&mut file, &opened.index, &expected, &uneven);
    }
    eprintln!(
        "{}: {chunks} chunks, {points} points, W=1..={} ok",
        path.display(),
        chunks.min(8)
    );
}

#[test]
fn autzen_copc_ranges_equal_sequential_read() {
    let Some(path) = data_file("autzen.copc.laz") else {
        eprintln!("SKIPPED: las-0.11.2 autzen.copc.laz not found in the cargo registry");
        return;
    };
    check_file(&path, usize::MAX, 1);
}

#[test]
fn autzen_fixed_chunk_laz_ranges_equal_sequential_read() {
    let Some(path) = data_file("autzen.laz") else {
        eprintln!("SKIPPED: las-0.11.2 autzen.laz not found in the cargo registry");
        return;
    };
    check_file(&path, usize::MAX, 1);
}

/// Needs the git-ignored Belvedere dataset; run with `-- --ignored`.
#[test]
#[ignore = "needs data/datasets/belv_2009_digital-aerial_pcd_copc.laz"]
fn belvedere_copc_first_chunks_ranges_equal_sequential_read() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../data/datasets/belv_2009_digital-aerial_pcd_copc.laz");
    assert!(path.exists(), "missing {}", path.display());
    check_file(&path, 256, 64);
}

/// Point10 records with non-constant, position-dependent values: a decoder that starts at the
/// wrong point cannot reproduce them.
fn synthetic_points(total: usize) -> Vec<u8> {
    let mut state = 0x5eed_u64;
    let mut out = Vec::with_capacity(total * POINT10);
    for index in 0..total as i32 {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let noise = (state >> 40) as i32;
        out.extend_from_slice(&(index.wrapping_mul(977) ^ noise).to_le_bytes());
        out.extend_from_slice(&(index.wrapping_mul(31) + noise % 1000).to_le_bytes());
        out.extend_from_slice(&(index / 3 + noise % 50).to_le_bytes());
        out.extend_from_slice(&(((state >> 20) & 0xffff) as u16).to_le_bytes());
        out.push(0b0001_1001); // return 1 of 3, no scan flags
        out.push(((index % 7) + 1) as u8);
        out.push((noise % 90) as i8 as u8);
        out.push((index % 251) as u8);
        out.extend_from_slice(&((index % 1000) as u16).to_le_bytes());
    }
    out
}

fn synthetic_laz() -> (Vec<u8>, Vec<u8>) {
    let total: usize = SYNTHETIC_CHUNKS.iter().sum();
    let points = synthetic_points(total);
    let items = laz::laszip::LazItemRecordBuilder::new()
        .add_item(LazItemType::Point10)
        .build();
    let vlr = LazVlrBuilder::default()
        .with_laz_items(items)
        .with_variable_chunk_size()
        .build();
    let mut cursor = Cursor::new(Vec::new());
    let mut compressor = LasZipCompressor::new(&mut cursor, vlr).unwrap();
    let mut at = 0;
    for (number, size) in SYNTHETIC_CHUNKS.iter().enumerate() {
        compressor
            .compress_many(&points[at * POINT10..(at + size) * POINT10])
            .unwrap();
        at += size;
        // `done` closes the last chunk itself; closing it here too adds an empty entry.
        if number + 1 < SYNTHETIC_CHUNKS.len() {
            compressor.finish_current_chunk().unwrap();
        }
    }
    compressor.done().unwrap();
    drop(compressor);
    (cursor.into_inner(), points)
}

fn synthetic_index(bytes: &[u8]) -> (ChunkIndex, laz::laszip::LazVlr) {
    let items = laz::laszip::LazItemRecordBuilder::new()
        .add_item(LazItemType::Point10)
        .build();
    let vlr = LazVlrBuilder::default()
        .with_laz_items(items)
        .with_variable_chunk_size()
        .build();
    let total = SYNTHETIC_CHUNKS.iter().sum::<usize>() as u64;
    let index = ChunkIndex::read(&mut Cursor::new(bytes), vlr.clone(), 0, total).unwrap();
    (index, vlr)
}

#[test]
fn synthetic_variable_chunks_every_split_equals_sequential_read() {
    let (bytes, points) = synthetic_laz();
    let (index, vlr) = synthetic_index(&bytes);
    let sizes: Vec<u64> = index
        .table
        .as_ref()
        .iter()
        .map(|entry| entry.point_count)
        .collect();
    assert_eq!(sizes, SYNTHETIC_CHUNKS.map(|size| size as u64));

    let mut sequential = Vec::new();
    decode_sequential(
        Cursor::new(bytes.clone()),
        &vlr,
        0,
        points.len() as u64 / POINT10 as u64,
        &mut sequential,
    )
    .unwrap();
    assert!(
        sequential == points,
        "laz must round-trip the synthetic points"
    );

    let mut src = Cursor::new(&bytes);
    let mut checked = 0;
    for parts in 1..=SYNTHETIC_CHUNKS.len().min(8) {
        for split in all_splits(SYNTHETIC_CHUNKS.len(), parts) {
            assert_split_matches(&mut src, &index, &points, &split);
            checked += 1;
        }
    }
    // Compositions of 6 chunks into 1..=6 parts: 2^5.
    assert_eq!(checked, 32);
}

/// Negative control: `LasZipDecompressor::seek` computes `point_idx % chunk.point_count`, which
/// is only right when every chunk has the same size. This is why the probe never seeks.
#[test]
fn laz_seek_lands_on_the_wrong_point_with_variable_chunks() {
    let (bytes, points) = synthetic_laz();
    let (_, vlr) = synthetic_index(&bytes);
    let record = |index: usize| &points[index * POINT10..(index + 1) * POINT10];
    // Chunks start at points 0, 7, 20, 25, 36, 50036. Point 9 is the 3rd of chunk 1 (13 points),
    // so delta must be 2; `laz` uses 9 % 13 = 9 and returns point 7 + 9 = 16.
    let mut decompressor = LasZipDecompressor::new(Cursor::new(bytes), vlr).unwrap();
    decompressor.seek(9).unwrap();
    let mut got = [0u8; POINT10];
    decompressor.decompress_one(&mut got).unwrap();
    assert_ne!(
        got,
        record(9),
        "seek unexpectedly became correct: the bug may be fixed"
    );
    assert_eq!(
        got,
        record(16),
        "seek returned neither the right point nor 7 + 9 % 13"
    );
}

/// Fixed-size chunks (`chunk_size` 10, partial last chunk): the table has no point counts, so
/// the index derives them from the header total.
#[test]
fn synthetic_fixed_chunks_every_split_equals_sequential_read() {
    const TOTAL: usize = 45;
    let points = synthetic_points(TOTAL);
    let items = laz::laszip::LazItemRecordBuilder::new()
        .add_item(LazItemType::Point10)
        .build();
    let vlr = LazVlrBuilder::default()
        .with_laz_items(items)
        .with_fixed_chunk_size(10)
        .build();
    let mut cursor = Cursor::new(Vec::new());
    let mut compressor = LasZipCompressor::new(&mut cursor, vlr.clone()).unwrap();
    compressor.compress_many(&points).unwrap();
    compressor.done().unwrap();
    drop(compressor);
    let bytes = cursor.into_inner();
    let index = ChunkIndex::read(&mut Cursor::new(&bytes), vlr, 0, TOTAL as u64).unwrap();
    let sizes: Vec<u64> = index
        .table
        .as_ref()
        .iter()
        .map(|entry| entry.point_count)
        .collect();
    assert_eq!(sizes, [10, 10, 10, 10, 5]);
    let mut src = Cursor::new(&bytes);
    for parts in 1..=5 {
        for split in all_splits(5, parts) {
            assert_split_matches(&mut src, &index, &points, &split);
        }
    }
}

fn committed_fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/synthetic_variable_chunks.laz")
}

/// The committed file is what the generator emits, so M2/M3a parity SHAs stay reproducible.
#[test]
fn committed_synthetic_fixture_is_reproducible() {
    let generated = laz_chunk_probe::synthetic_variable_chunk_laz().unwrap();
    assert!(
        std::fs::read(committed_fixture()).unwrap() == generated,
        "regenerate with `laz_chunk_probe make-synthetic`"
    );
}

/// The same fixture as a real LAS file: header, VLR and chunk table parsed by `las`.
#[test]
fn committed_synthetic_fixture_ranges_equal_sequential_read() {
    check_file(&committed_fixture(), usize::MAX, 6);
}
