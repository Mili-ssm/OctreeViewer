//! Measures how much heap `FormatReader` needs to parse a worst-case index, relative to the
//! index size on disk. The worst case is many nodes, each carrying one smallest-encoding range
//! (`u8`, one component) per attribute; a `NodeAttributeRange` is two `Vec`s in memory.
//!
//! Reproduce the table with `cargo test -p viewer_format --profile fast --test
//! index_amplification -- --ignored --nocapture`.

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::io::Cursor;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use viewer_core::{
    AttributeColumn, AttributeDataType, AttributeDescriptor, AttributeValues, CoordinateFrame,
    DatasetMetadata, NodeId, SpatialBounds, SpatialHierarchy, SpatialNode,
};
use viewer_format::{
    FormatError, FormatMetadata, FormatReader, FormatReaderLimits, FormatVersion, FormatWriter,
    NodeAttributeRange, SourcePointBlock,
};

/// Process-wide live and peak heap bytes. Tests that measure hold `MEASURE` so concurrent
/// tests (e.g. under `--include-ignored`) cannot disturb the shared counters.
struct PeakAllocator;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static MEASURE: Mutex<()> = Mutex::new(());

fn grow(size: usize) {
    let live = LIVE.fetch_add(size, Ordering::Relaxed) + size;
    PEAK.fetch_max(live, Ordering::Relaxed);
}

// SAFETY: every call is forwarded unchanged to the system allocator.
unsafe impl GlobalAlloc for PeakAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        grow(layout.size());
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        grow(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        grow(new_size);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: PeakAllocator = PeakAllocator;

const ATTRIBUTES: u32 = 256;

/// Worst-case index: a root plus `nodes - 1` children, each with a `u8` min/max range for
/// every one of the 256 attributes (the default `max_attributes`).
fn worst_case_file(nodes: u64) -> Vec<u8> {
    let schema: Vec<_> = (0..ATTRIBUTES)
        .map(|i| AttributeDescriptor::new(format!("a{i}"), AttributeDataType::U8, 1).unwrap())
        .collect();
    let schema_for_block = schema.clone();
    let bounds = SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap();
    let mut hierarchy_nodes = Vec::new();
    let mut ranges = Vec::new();
    for id in 0..nodes {
        let parent = (id != 0).then_some(NodeId::new(0));
        let level = u32::from(id != 0);
        hierarchy_nodes.push(
            SpatialNode::new(
                NodeId::new(id),
                parent,
                level,
                bounds,
                4,
                0.0,
                vec![common::BLOCK_ID],
            )
            .unwrap(),
        );
        ranges.push((
            NodeId::new(id),
            (0..ATTRIBUTES)
                .map(|i| {
                    NodeAttributeRange::new(
                        i,
                        AttributeValues::U8(vec![0]),
                        AttributeValues::U8(vec![1]),
                    )
                })
                .collect(),
        ));
    }
    let hierarchy = SpatialHierarchy::new(hierarchy_nodes, schema, Some(4)).unwrap();
    let meta = FormatMetadata::new(
        DatasetMetadata::new(CoordinateFrame::new([0.0; 3]).unwrap(), hierarchy),
        None,
        None,
        ranges,
    )
    .unwrap();
    let version = FormatVersion::DEFAULT_OUTPUT;
    let mut writer =
        FormatWriter::new_for_version(version, Cursor::new(Vec::new()), meta, common::options())
            .unwrap();
    let columns = schema_for_block
        .into_iter()
        .map(|d| AttributeColumn::new(d, AttributeValues::U8(vec![0, 1, 0, 1]), 4).unwrap())
        .collect();
    let block = SourcePointBlock::new(
        common::BLOCK_ID,
        common::POINTS.to_vec(),
        common::COLORS.to_vec(),
        columns,
    );
    writer.write_block(block).unwrap();
    writer.finish().unwrap().into_inner()
}

fn index_len(file: &[u8]) -> usize {
    let footer = file.len() - 40;
    u64::from_le_bytes(file[footer + 16..footer + 24].try_into().unwrap()) as usize
}

#[test]
#[ignore = "slow: ~70 s; run to reproduce the amplification table"]
fn worst_case_index_parse_amplification() {
    let _measure = MEASURE.lock().unwrap_or_else(|e| e.into_inner());
    for target_mib in [4_usize, 32, 128] {
        // About 1.6 KiB of index per node at 256 attributes.
        let nodes = (target_mib * 1024 * 1024 / 1_630) as u64;
        let file = worst_case_file(nodes);
        let index = index_len(&file);
        let baseline = LIVE.load(Ordering::Relaxed);
        PEAK.store(baseline, Ordering::Relaxed);
        // The 128 MiB case exceeds the default range total; measure the unbounded parse.
        let limits = FormatReaderLimits::default()
            .with_max_attribute_ranges(u64::MAX)
            .unwrap();
        let reader = FormatReader::with_limits(Cursor::new(file.as_slice()), limits)
            .expect("worst-case index opens with the range bound lifted");
        let peak = PEAK.load(Ordering::Relaxed) - baseline;
        let retained = LIVE.load(Ordering::Relaxed) - baseline;
        drop(reader);
        let factor = peak as f64 / index as f64;
        println!(
            "nodes={nodes} index={index} B peak={peak} B retained={retained} B factor={factor:.2}"
        );
    }
}

/// 4 MiB worst-case file: 2573 nodes x 256 ranges = 658 688 ranges.
fn small_worst_case() -> Vec<u8> {
    worst_case_file(4 * 1024 * 1024 / 1_630)
}

fn peak_of_open(file: &[u8], limits: FormatReaderLimits) -> (usize, Result<(), FormatError>) {
    let baseline = LIVE.load(Ordering::Relaxed);
    PEAK.store(baseline, Ordering::Relaxed);
    let result = FormatReader::with_limits(Cursor::new(file), limits).map(drop);
    (PEAK.load(Ordering::Relaxed) - baseline, result)
}

#[test]
fn range_bound_rejects_before_allocating_ranges() {
    let _measure = MEASURE.lock().unwrap_or_else(|e| e.into_inner());
    let file = small_worst_case();
    let index = index_len(&file);

    let (open_peak, result) = peak_of_open(&file, FormatReaderLimits::default());
    result.expect("worst-case index below the default range bound opens");
    assert!(
        open_peak > 4 * index,
        "fixture no longer amplifies: {open_peak}"
    );

    let limits = FormatReaderLimits::default()
        .with_max_attribute_ranges(100_000)
        .unwrap();
    let (peak, result) = peak_of_open(&file, limits);
    assert!(
        matches!(
            result,
            Err(FormatError::CountLimit {
                kind: "node attribute ranges",
                limit: 100_000,
                ..
            })
        ),
        "{result:?}"
    );
    // Rejected after ~390 nodes, long before the ~50 MB of ranges of a full parse.
    assert!(peak < 4 * index, "peak {peak} B for index {index} B");
    assert!(
        FormatReaderLimits::default()
            .with_max_attribute_ranges(0)
            .is_err()
    );
}
