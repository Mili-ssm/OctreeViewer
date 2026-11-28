//! Threaded root ranges and joined segments.

use super::*;

fn ranged(options: ConvertOptions, ranges: usize) -> ConvertOptions {
    options.with_root_partition_ranges(ranges).unwrap()
}

fn block_options() -> ConvertOptions {
    leaf_new(4096, 0.01).unwrap()
}

fn convert_pair(input: &std::path::Path, options: ConvertOptions) -> (Vec<u8>, Vec<u8>) {
    let tmp = test_dir();
    let output = tmp.path().join("out.oct");
    convert_file(input, &output, options).unwrap();
    output_pair_bytes(&output)
}

fn assert_threaded_ranges_match_default(input: &std::path::Path, label: &str) {
    let expected = convert_pair(input, block_options());
    for ranges in 1..=8 {
        let got = convert_pair(input, ranged(block_options(), ranges));
        assert!(got == expected, "{label}: ranges {ranges} differ");
    }
}

#[test]
fn threaded_root_ranges_match_the_default_scan_on_las_and_laz() {
    let tmp = test_dir();
    let las = tmp.path().join("ranges.las");
    fixture(&las, false, true, 20_001);
    assert_threaded_ranges_match_default(&las, "las");
    let variable = workspace_path("tools/laz_chunk_probe/fixtures/synthetic_variable_chunks.laz");
    assert_threaded_ranges_match_default(&variable, "variable laz");
    // 110 007 points: chunks of 50 000, 50 000 and 10 007.
    let fixed = tmp.path().join("ranges.laz");
    fixture(&fixed, true, true, 110_007);
    let expected = convert_pair(&fixed, block_options());
    for ranges in [1, 2, 3, 8] {
        let got = convert_pair(&fixed, ranged(block_options(), ranges));
        assert!(got == expected, "fixed laz {ranges}");
    }
}

#[test]
fn threaded_root_ranges_report_the_lowest_start_error_of_two_failing_ranges() {
    let tmp = test_dir();
    let input = tmp.path().join("outliers.las");
    fixture(&input, false, true, 20_000);
    // Header max X below the points from about index 9 000 on: ranges 1 and 2 (of 3) both fail.
    let mut bytes = fs::read(&input).unwrap();
    let max_x: f64 = 1_000_000.0 + 9_000.0 * 0.25 - 1.0;
    bytes[179..187].copy_from_slice(&max_x.to_le_bytes());
    fs::write(&input, bytes).unwrap();
    let message = |options: ConvertOptions| {
        let output = tmp.path().join("out.oct");
        let error = convert_file(&input, &output, options).unwrap_err();
        assert!(!output.exists());
        error.to_string()
    };
    let serial = message(block_options());
    assert!(
        serial.contains("is outside the LAS/LAZ header bounds"),
        "{serial}"
    );
    for ranges in [2, 3, 8] {
        for _ in 0..10 {
            assert_eq!(message(ranged(block_options(), ranges)), serial);
        }
    }
}

#[test]
fn threaded_root_ranges_report_a_corrupt_laz_chunk_like_the_default_scan() {
    let tmp = test_dir();
    let input = tmp.path().join("corrupt.laz");
    let mut bytes = fs::read(workspace_path(
        "tools/laz_chunk_probe/fixtures/synthetic_variable_chunks.laz",
    ))
    .unwrap();
    // Well inside the 50 000-point chunk; the small leading chunks occupy the first ~200 bytes.
    let at = bytes.len() / 2;
    for byte in &mut bytes[at..at + 300] {
        *byte ^= 0xA5;
    }
    fs::write(&input, bytes).unwrap();
    let message = |options: ConvertOptions| {
        let output = tmp.path().join("out.oct");
        convert_file(&input, &output, options)
            .unwrap_err()
            .to_string()
    };
    let serial = message(block_options());
    for ranges in [2, 3, 8] {
        assert_eq!(message(ranged(block_options(), ranges)), serial);
    }
}

#[test]
fn joined_segments_read_back_identically() {
    use std::io::Read;
    use viewer_convert_core::pipeline::spool_store::{SpoolExtent, SpoolId, SpoolStore};
    use viewer_convert_core::spatial::node_job::join_segments;

    const RECORD: usize = 42;
    let dir = test_dir();
    let store = crate::file_spool_store::FileSpoolStore::new(dir.path(), false);
    let mut expected = Vec::new();
    let mut ids = Vec::new();
    for (segment, count) in [(1_u64, 9_000_usize), (2, 17_000), (3, 3_001)] {
        let mut data = Vec::new();
        for index in 0..count {
            let mut record = [segment as u8; RECORD];
            record[..8].copy_from_slice(&(index as u64).to_le_bytes());
            data.extend_from_slice(&record);
        }
        let id = SpoolId(segment);
        store.create(id, RECORD).unwrap();
        // Chunks that do not end on record boundaries.
        for part in data.chunks(65_536) {
            store.append(id, part).unwrap();
        }
        store.close(id).unwrap();
        expected.extend_from_slice(&data);
        ids.push(id);
    }
    let target = SpoolId(9);
    let total = join_segments(&store, target, &ids, RECORD).unwrap();
    assert_eq!(total, expected.len() as u64);
    assert_eq!(store.len(target).unwrap(), expected.len() as u64);
    let mut out = Vec::new();
    store
        .reader(
            target,
            SpoolExtent {
                offset: 0,
                len: total,
            },
        )
        .unwrap()
        .read_to_end(&mut out)
        .unwrap();
    assert_eq!(out, expected);
}
