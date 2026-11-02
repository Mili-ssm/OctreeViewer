//! The ranged root partition (`with_root_partition_ranges`) against the default scan: same
//! bytes, same errors. Runs on `SerialExecutor` with `MemSpoolStore` and `MemBytes`.

use std::io::Cursor;

use las::{Builder, Point, Writer, point::Format};
use viewer_core::SpatialBounds;

use crate::{
    ConvertError, ConvertOptions, LasFactory, MemBytes, MemSpoolStore, NoClock, SourceFactory,
    convert_serial,
    pipeline::{
        executor::{RangeRunner, SerialExecutor},
        node_job::{JobContext, RangeJob, merge_root_ranges},
        point_source::PointRange,
        spool_store::{SpoolId, SpoolStore},
    },
    source::inspect_header,
    spatial::SpoolLayout,
};

fn lcg(seed: u64) -> impl FnMut() -> f64 {
    let mut state = seed;
    move || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (state >> 33) as f64 / (1_u64 << 31) as f64
    }
}

/// `count` pseudo-random points in `[low, 100)³` with distinct intensities.
fn random_points(count: usize, low: f64) -> Vec<[f64; 3]> {
    let mut next = lcg(7);
    (0..count)
        .map(|_| std::array::from_fn(|_| (low + next() * (100.0 - low) * 1000.0).floor() / 1000.0))
        .collect()
}

fn las_bytes(points: &[[f64; 3]], compressed: bool, intensity: impl Fn(usize) -> u16) -> Vec<u8> {
    let mut builder = Builder::from((1, 2));
    builder.point_format = Format::new(0).unwrap();
    builder.point_format.is_compressed = compressed;
    let mut writer = Writer::new(Cursor::new(Vec::new()), builder.into_header().unwrap()).unwrap();
    for (index, [x, y, z]) in points.iter().copied().enumerate() {
        writer
            .write_point(Point {
                x,
                y,
                z,
                intensity: intensity(index),
                return_number: 1,
                number_of_returns: 1,
                ..Point::default()
            })
            .unwrap();
    }
    writer.into_inner().unwrap().into_inner()
}

fn options(points_per_block: u64, divisions: u8, ranges: usize) -> ConvertOptions {
    ConvertOptions::new(points_per_block, 0.01)
        .unwrap()
        .with_spatial_divisions(divisions)
        .unwrap()
        .with_root_partition_ranges(ranges)
        .unwrap()
}

type Pair = (Vec<u8>, Vec<u8>);

/// Converts and checks that no spool outlives the conversion.
fn convert(bytes: &[u8], options: ConvertOptions) -> Result<Pair, ConvertError> {
    let store = MemSpoolStore::new(false);
    let result = convert_serial(
        &LasFactory::new(MemBytes(bytes.to_vec().into())),
        &store,
        &NoClock,
        options,
        Cursor::new(Vec::new()),
        Cursor::new(Vec::new()),
        &mut |_| true,
    );
    let stats = store.stats();
    assert_eq!(
        (stats.live_spools, stats.live_bytes),
        (0, 0),
        "{}",
        result
            .as_ref()
            .err()
            .map_or(String::new(), ToString::to_string)
    );
    result.map(|output| (output.geometry.into_inner(), output.attributes.into_inner()))
}

fn assert_ranges_match_default(bytes: &[u8], points_per_block: u64, label: &str) {
    for divisions in [2, 4, 8] {
        let expected = convert(bytes, options(points_per_block, divisions, 0)).unwrap();
        for ranges in 1..=8 {
            let got = convert(bytes, options(points_per_block, divisions, ranges)).unwrap();
            assert!(
                got == expected,
                "{label}: divisions {divisions} ranges {ranges} differ"
            );
        }
    }
}

#[test]
fn las_record_ranges_match_the_default_scan() {
    // 20 001 points: the last range is a partial batch of 4096.
    let points = random_points(20_001, 0.0);
    assert_ranges_match_default(&las_bytes(&points, false, |i| i as u16), 4096, "las");
}

#[test]
fn laz_chunk_ranges_match_the_default_scan() {
    // Chunks [7, 13, 5, 11, 50000, 3]: tiny chunks, one dominant chunk, a short last chunk.
    let bytes = laz_chunk_probe::synthetic_variable_chunk_laz().unwrap();
    assert_ranges_match_default(&bytes, 4096, "variable laz");
}

#[test]
fn fixed_size_laz_chunks_with_a_partial_last_chunk_match_the_default_scan() {
    // `las` writes 50 000-point chunks: 50 000 + 50 000 + 10 007.
    let points = random_points(110_007, 0.0);
    let bytes = las_bytes(&points, true, |i| (i * 31) as u16);
    for divisions in [2, 8] {
        let expected = convert(&bytes, options(4096, divisions, 0)).unwrap();
        for ranges in [1, 2, 3, 8] {
            let got = convert(&bytes, options(4096, divisions, ranges)).unwrap();
            assert!(got == expected, "divisions {divisions} ranges {ranges}");
        }
    }
}

#[test]
fn a_duplicate_point_across_a_range_boundary_keeps_the_earlier_one() {
    // Two identical points at the centre of voxel 0 (distance 0) with different intensities: one
    // in each of the two ranges `[0, 8192)` and `[8192, 10 000)`. The corner point is farther.
    let mut points = random_points(10_000, 20.0);
    points[0] = [0.0; 3];
    points[8_000] = [6.25; 3];
    points[9_000] = [6.25; 3];
    points[9_999] = [100.0; 3];
    let bytes = las_bytes(&points, false, |i| match i {
        8_000 => 111,
        9_000 => 222,
        _ => 5,
    });
    let expected = convert(&bytes, options(4096, 2, 0)).unwrap();
    for ranges in [1, 2, 3] {
        assert!(convert(&bytes, options(4096, 2, ranges)).unwrap() == expected);
    }
}

fn context(info: &crate::source::SourceInfo, divisions: u8, limit: u64) -> JobContext {
    JobContext {
        point_limit: limit,
        spatial_divisions: divisions,
        sample_grid: crate::options::DEFAULT_SAMPLE_GRID,
        frame: info.frame,
        layout: SpoolLayout::source(info),
        collect_diagnostics: false,
    }
}

#[test]
fn children_with_points_only_in_the_last_range_or_in_none_create_no_spool() {
    // Octants of a divisions-2 root over [0, 100]³: keys 0 and 7 are used, 7 only after the
    // first range, and keys 1..=6 never.
    let mut points = vec![[10.0; 3]; 8_192];
    points.extend((0..1_808).map(|i| if i % 2 == 0 { [90.0; 3] } else { [10.0; 3] }));
    points[0] = [0.0; 3];
    points[9_999] = [100.0; 3];
    let bytes = las_bytes(&points, false, |i| i as u16);
    let factory = LasFactory::new(MemBytes(bytes.into()));
    let ranges = factory.split(2, 4096).unwrap();
    assert_eq!(ranges.len(), 2);
    let first = factory.open(ranges[0]).unwrap();
    let info = inspect_header(crate::pipeline::point_source::PointSource::header(&first)).unwrap();
    let ctx = context(&info, 2, 4096);
    let bounds = SpatialBounds::new([0.0; 3], [100.0; 3]).unwrap();
    let job = RangeJob {
        ctx: &ctx,
        info: &info,
        bounds,
        batch: 4096,
    };
    let store = MemSpoolStore::new(false);
    drop(first);
    let ranges = vec![(ranges[0], SpoolId(100)), (ranges[1], SpoolId(108))];
    let results =
        SerialExecutor.run_ranges(&job, &factory, ranges, &store, &NoClock, &mut |_, _| Ok(()));
    let first_range = results[0].as_ref().unwrap();
    assert!(first_range.segments.iter().all(|segment| segment.key != 7));
    for key in 1..7 {
        assert!(store.len(SpoolId(100 + key)).is_err() && store.len(SpoolId(108 + key)).is_err());
    }
    let partition = merge_root_ranges(&ctx, bounds, SpoolId(1), results, &store).unwrap();
    let keys = partition.children.iter().map(|c| c.key).collect::<Vec<_>>();
    assert_eq!(keys, [0, 7], "ascending key order");
    assert_eq!(partition.children[0].spool, SpoolId(1));
    assert_eq!(partition.children[1].spool, SpoolId(8));
    assert_eq!(
        partition
            .children
            .iter()
            .map(|c| c.point_count)
            .sum::<u64>(),
        10_000
    );
    for key in 1..7 {
        assert!(store.len(SpoolId(1 + key)).is_err());
    }
    // Segments were deleted by the join; only the two joined children remain.
    assert_eq!(store.stats().live_spools, 2);
}

/// Sets the header bounds of a LAS 1.2 file (offsets 179..227: max/min of x, y, z).
fn with_header_bounds(mut bytes: Vec<u8>, min: [f64; 3], max: [f64; 3]) -> Vec<u8> {
    for axis in 0..3 {
        let at = 179 + axis * 16;
        bytes[at..at + 8].copy_from_slice(&max[axis].to_le_bytes());
        bytes[at + 8..at + 16].copy_from_slice(&min[axis].to_le_bytes());
    }
    bytes
}

fn error_text(result: Result<Pair, ConvertError>) -> String {
    match result.unwrap_err() {
        ConvertError::LasDecoderPanicked { point_index, .. } => {
            format!("decoder panic at {point_index:?}")
        }
        other => other.to_string(),
    }
}

#[test]
fn an_outlier_in_a_later_range_reports_the_serial_error_and_the_lowest_start_wins() {
    let mut points = random_points(20_000, 10.0);
    points[0] = [0.0; 3];
    points[19_999] = [100.0; 3];
    // Two outliers beyond the header bounds, in ranges 1 and 2 of three.
    points[9_000] = [100.0, 50.0, 50.0];
    points[17_000] = [100.0, 60.0, 60.0];
    let bytes = with_header_bounds(
        las_bytes(&points, false, |i| i as u16),
        [0.0; 3],
        [95.0, 100.0, 100.0],
    );
    let serial = error_text(convert(&bytes, options(4096, 2, 0)));
    assert!(serial.contains("point 9000 "), "{serial}");
    for ranges in 1..=8 {
        assert_eq!(
            error_text(convert(&bytes, options(4096, 2, ranges))),
            serial
        );
    }
    // Only the later outlier left: the error comes from range 2.
    points[9_000] = [50.0; 3];
    let bytes = with_header_bounds(
        las_bytes(&points, false, |i| i as u16),
        [0.0; 3],
        [95.0, 100.0, 100.0],
    );
    let serial = error_text(convert(&bytes, options(4096, 2, 0)));
    assert!(serial.contains("point 17000 "), "{serial}");
    assert_eq!(error_text(convert(&bytes, options(4096, 2, 3))), serial);
}

#[test]
fn a_truncated_las_fails_like_the_default_scan() {
    let points = random_points(20_000, 0.0);
    let mut bytes = las_bytes(&points, false, |i| i as u16);
    bytes.truncate(bytes.len() - 5_000 * 20 - 7);
    let serial = error_text(convert(&bytes, options(4096, 2, 0)));
    for ranges in [1, 3, 8] {
        assert_eq!(
            error_text(convert(&bytes, options(4096, 2, ranges))),
            serial
        );
    }
}

#[test]
fn a_corrupt_laz_chunk_in_a_later_range_fails_like_the_default_scan() {
    let mut bytes = laz_chunk_probe::synthetic_variable_chunk_laz().unwrap();
    // Inside the 50 000-point chunk (index 4), which every split with 2+ ranges puts after range 0.
    let header = las::Reader::new(Cursor::new(bytes.clone()))
        .unwrap()
        .header()
        .clone();
    let offset = las::raw::Header::read_from(Cursor::new(&bytes))
        .unwrap()
        .offset_to_point_data;
    let index = crate::pipeline::laz_chunks::ChunkIndex::read(
        &mut Cursor::new(&bytes),
        &header,
        offset.into(),
    )
    .unwrap();
    let before: u64 = index.table.as_ref()[..4].iter().map(|e| e.byte_count).sum();
    let at = (index.data_start + before) as usize + 20_000;
    for byte in &mut bytes[at..at + 300] {
        *byte ^= 0xA5;
    }
    let serial = convert(&bytes, options(4096, 2, 0));
    assert!(serial.is_err(), "the corruption must be detected");
    let serial = error_text(serial);
    // With `laz-parallel` unified in by another crate the default scan decodes chunks in
    // parallel, and the corrupt data can end as a later error than the sequential decoder's
    // panic (or the reverse). Both fail with one of the two outcomes of this corruption, and a
    // panic carries the batch-aligned index of the serial scan.
    let known = |text: &str| {
        text == "decoder panic at Some(0)" || text.ends_with("is outside the LAS/LAZ header bounds")
    };
    for ranges in [2, 3, 8] {
        let ranged = error_text(convert(&bytes, options(4096, 2, ranges)));
        assert!(
            ranged == serial || (known(&ranged) && known(&serial)),
            "ranges {ranges}: {ranged} vs {serial}"
        );
    }
}

#[test]
fn an_all_equal_root_and_a_root_that_does_not_divide_match_the_default_scan() {
    // Every point equal inside wider header bounds: the root splits, then becomes a leaf.
    let equal = with_header_bounds(
        las_bytes(&vec![[50.0; 3]; 10_000], false, |i| i as u16),
        [0.0; 3],
        [100.0; 3],
    );
    // Fewer points than a block: the root is a leaf and ranges are ignored.
    let small = las_bytes(&random_points(3_000, 0.0), false, |i| i as u16);
    // Degenerate header bounds: the root cube has no size and cannot divide.
    let flat = las_bytes(&vec![[5.0; 3]; 10_000], false, |i| i as u16);
    for (label, bytes) in [("all equal", equal), ("small", small), ("flat", flat)] {
        let expected = convert(&bytes, options(4096, 2, 0)).unwrap();
        for ranges in [1, 2, 4, 8] {
            assert!(
                convert(&bytes, options(4096, 2, ranges)).unwrap() == expected,
                "{label} ranges {ranges}"
            );
        }
    }
}

#[test]
fn root_ranges_are_validated_and_split_keeps_record_batches() {
    assert!(matches!(
        ConvertOptions::default().with_root_partition_ranges(9),
        Err(ConvertError::InvalidRootRanges(9))
    ));
    let ranges = PointRange::split(20_001, 4096, 3);
    assert!(ranges.iter().all(|range| range.start % 4096 == 0));
}

fn estimate(bytes: &[u8], options: ConvertOptions) -> u64 {
    convert_serial(
        &LasFactory::new(MemBytes(bytes.to_vec().into())),
        &MemSpoolStore::new(false),
        &NoClock,
        options,
        Cursor::new(Vec::new()),
        Cursor::new(Vec::new()),
        &mut |_| true,
    )
    .unwrap()
    .plan
    .estimated_peak_memory_bytes
}

#[test]
fn ranges_are_admitted_by_the_memory_budget_and_reported_in_the_estimate() {
    let bytes = las_bytes(&random_points(20_000, 0.0), false, |i| i as u16);
    let single = |options: ConvertOptions| options.with_spatial_workers(1).unwrap();
    let default = estimate(&bytes, single(options(4096, 2, 0)));
    assert!(estimate(&bytes, single(options(4096, 2, 4))) > default);
    // A budget with no room for a second range keeps the estimate of one range.
    let tight = |ranges| {
        single(ConvertOptions::with_memory_budget(4096, 0.01, default + 1_000_000).unwrap())
            .with_root_partition_ranges(ranges)
            .unwrap()
    };
    assert_eq!(estimate(&bytes, tight(8)), default);
}

mod spool_hints {
    use std::{collections::BTreeMap, sync::Mutex};

    use super::*;
    use crate::{
        pipeline::spool_store::{LedgerTotals, SpoolAccounting, SpoolExtent},
        spatial::{MAX_SPATIAL_DEPTH, consume_rank},
    };

    #[derive(Default)]
    struct Log {
        /// Hinted rank of every spool still queued (hinted, not yet read).
        queued: BTreeMap<SpoolId, u32>,
        retained: Vec<SpoolId>,
        deleted: Vec<SpoolId>,
        /// Ranks at which a queued spool was consumed (scanned, or committed as a leaf block), and the queue minimum at that moment.
        consumptions: Vec<(u32, u32)>,
    }

    impl Log {
        fn consume(&mut self, id: SpoolId) {
            if let Some(rank) = self.queued.remove(&id) {
                let minimum = self
                    .queued
                    .values()
                    .copied()
                    .min()
                    .map_or(rank, |m| m.min(rank));
                self.consumptions.push((rank, minimum));
            }
        }
    }

    /// Forwards everything to a `MemSpoolStore` and logs hints and consumptions.
    struct Recording {
        inner: MemSpoolStore,
        log: Mutex<Log>,
    }

    impl Recording {
        fn new() -> Self {
            Self {
                inner: MemSpoolStore::new(false),
                log: Mutex::default(),
            }
        }
    }

    impl SpoolStore for Recording {
        type Reader = <MemSpoolStore as SpoolStore>::Reader;
        fn create(&self, id: SpoolId) -> Result<(), ConvertError> {
            self.inner.create(id)
        }
        fn append(&self, id: SpoolId, bytes: &[u8]) -> Result<(), ConvertError> {
            self.inner.append(id, bytes)
        }
        fn close(&self, id: SpoolId) -> Result<(), ConvertError> {
            self.inner.close(id)
        }
        fn len(&self, id: SpoolId) -> Result<u64, ConvertError> {
            self.inner.len(id)
        }
        fn reader(&self, id: SpoolId, extent: SpoolExtent) -> Result<Self::Reader, ConvertError> {
            let mut log = self.log.lock().unwrap();
            log.consume(id);
            drop(log);
            self.inner.reader(id, extent)
        }
        fn delete(&self, id: SpoolId) -> Result<(), ConvertError> {
            let mut log = self.log.lock().unwrap();
            log.queued.remove(&id);
            log.deleted.push(id);
            drop(log);
            self.inner.delete(id)
        }
        fn hint_retained(&self, id: SpoolId) {
            let mut log = self.log.lock().unwrap();
            log.retained.push(id);
            // A leaf child is never scanned: committing it is when the planner reaches it.
            log.consume(id);
        }
        fn hint_consume_rank(&self, id: SpoolId, rank: u32) {
            assert!(self.log.lock().unwrap().queued.insert(id, rank).is_none());
        }
    }

    impl SpoolAccounting for Recording {
        fn next_event_seq(&self) -> Option<u64> {
            None
        }
        fn ledger_totals(&self) -> Option<LedgerTotals> {
            None
        }
    }

    fn run_recording(bytes: &[u8], options: ConvertOptions) -> (Pair, Recording) {
        let store = Recording::new();
        let output = convert_serial(
            &LasFactory::new(MemBytes(bytes.to_vec().into())),
            &store,
            &NoClock,
            options,
            Cursor::new(Vec::new()),
            Cursor::new(Vec::new()),
            &mut |_| true,
        )
        .unwrap();
        let pair = (output.geometry.into_inner(), output.attributes.into_inner());
        (pair, store)
    }

    fn fixtures() -> Vec<(&'static str, Vec<u8>, ConvertOptions)> {
        let points = random_points(3_000, 0.0);
        let bytes = las_bytes(&points, false, |i| i as u16);
        vec![
            ("five-ish nodes", bytes.clone(), options(1_000, 2, 0)),
            ("deep divisions 2", bytes.clone(), options(64, 2, 0)),
            (
                "divisions 8",
                las_bytes(&random_points(20_000, 0.0), false, |i| i as u16),
                options(8, 8, 0),
            ),
        ]
    }

    #[test]
    fn consume_rank_orders_deeper_first_then_by_key() {
        assert!(consume_rank(2, 60_000) < consume_rank(1, 0));
        assert!(consume_rank(1, 3) < consume_rank(1, 4));
        assert_eq!(consume_rank(MAX_SPATIAL_DEPTH, 0), 1 << 16);
    }

    #[test]
    fn every_block_spool_is_hinted_retained_exactly_once() {
        for (label, bytes, options) in fixtures() {
            let (_, store) = run_recording(&bytes, options);
            let log = store.log.lock().unwrap();
            let mut unique = log.retained.clone();
            unique.sort();
            unique.dedup();
            assert!(!unique.is_empty(), "{label}: no retained hint");
            assert_eq!(unique.len(), log.retained.len(), "{label}: duplicate hint");
            // Block spools are exactly the ones the conversion deletes after planning.
            assert!(
                log.retained.iter().all(|id| log.deleted.contains(id)),
                "{label}: a retained spool was never released"
            );
        }
    }

    #[test]
    fn each_consumption_takes_the_minimum_rank_among_queued_spools() {
        for (label, bytes, options) in fixtures() {
            let (_, store) = run_recording(&bytes, options);
            let log = store.log.lock().unwrap();
            assert!(log.consumptions.len() > 4, "{label}: tree too small");
            assert!(
                log.consumptions
                    .iter()
                    .all(|(rank, minimum)| rank == minimum),
                "{label}: out-of-order consumption {:?}",
                log.consumptions
            );
            if label != "five-ish nodes" {
                assert!(
                    log.consumptions
                        .iter()
                        .any(|(rank, _)| rank >> 16 < MAX_SPATIAL_DEPTH),
                    "{label}: no consumption below depth 1"
                );
            }
        }
    }

    #[test]
    fn hints_do_not_change_output_bytes() {
        for (label, bytes, options) in fixtures() {
            let expected = convert(&bytes, options).unwrap();
            let (got, _) = run_recording(&bytes, options);
            assert!(got == expected, "{label}: bytes differ");
        }
    }
}
