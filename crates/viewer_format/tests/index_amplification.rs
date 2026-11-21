//! Measures how much heap `FormatReader` needs to parse a worst-case `v8` page, relative to the
//! page size on disk. The worst case is many nodes in one page, each carrying one
//! smallest-encoding range (`u8`, one component) per attribute; a `NodeAttributeRange` is two
//! `Vec`s in memory. The range total is bounded per page, so the fixture is one big root page.
//!
//! Reproduce the table with `cargo test -p viewer_format --profile fast --test
//! index_amplification -- --ignored --nocapture`.

mod common;
#[path = "common/v8_fixture.rs"]
mod v8_fixture;
#[path = "common/v8_tree.rs"]
mod v8_tree;

use std::alloc::{GlobalAlloc, Layout, System};
use std::io::Cursor;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use v8_tree::{tree, tree_dataset, write_tree_pair};
use viewer_core::{AttributeColumn, AttributeDataType, AttributeDescriptor, AttributeValues};
use viewer_format::{FormatError, FormatReader, FormatReaderLimits, PagedReader};

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
        // SAFETY: the caller upholds the `GlobalAlloc` contract for `layout`, and the call is forwarded unchanged to `System`.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        // SAFETY: `ptr` was allocated by this allocator, i.e. by `System` with the same `layout`, and the caller upholds the rest of the `GlobalAlloc` contract.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        grow(layout.size());
        // SAFETY: the caller upholds the `GlobalAlloc` contract for `layout`, and the call is forwarded unchanged to `System`.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        grow(new_size);
        // SAFETY: `ptr` was allocated by this allocator, i.e. by `System` with the same `layout`, and the caller upholds the rest of the `GlobalAlloc` contract.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: PeakAllocator = PeakAllocator;

const ATTRIBUTES: u32 = 256;

/// Worst-case page: 156 nodes (a root and three levels below it, five children per node), all in
/// the root page because its target is lifted, each node with a `u8` min/max range for every one of the 256
/// attributes (the default `max_attributes`).
fn worst_case_file() -> Vec<u8> {
    let schema: Vec<_> = (0..ATTRIBUTES)
        .map(|i| AttributeDescriptor::new(format!("a{i}"), AttributeDataType::U8, 1).unwrap())
        .collect();
    let nodes = tree(3, |_, octant| octant < 5);
    let (metadata, blocks) = tree_dataset(&nodes, &schema, |_, rows| {
        schema
            .iter()
            .map(|d| {
                let values = (0..rows).map(|row| (row % 2) as u8).collect();
                AttributeColumn::new(d.clone(), AttributeValues::U8(values), rows).unwrap()
            })
            .collect()
    });
    let metadata = metadata.with_lod_attribute_columns((0..ATTRIBUTES).collect());
    write_tree_pair(metadata, blocks, Some(usize::MAX / 2)).0
}

/// Encoded length of the root page, which is where the worst case lives.
fn index_len(file: &[u8]) -> usize {
    PagedReader::open(Cursor::new(file))
        .unwrap()
        .root_page()
        .encoded_len()
}

fn peak_of_open(file: &[u8], limits: FormatReaderLimits) -> (usize, Result<(), FormatError>) {
    let baseline = LIVE.load(Ordering::Relaxed);
    PEAK.store(baseline, Ordering::Relaxed);
    let result = FormatReader::with_limits(Cursor::new(file), limits).map(drop);
    (PEAK.load(Ordering::Relaxed) - baseline, result)
}

#[test]
#[ignore = "slow: reproduces the amplification table; run with --nocapture"]
fn worst_case_index_parse_amplification() {
    let _measure = MEASURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let file = worst_case_file();
    let index = index_len(&file);
    let (peak, result) = peak_of_open(&file, FormatReaderLimits::default());
    result.expect("worst-case page opens");
    println!(
        "page={index} B peak={peak} B factor={:.2}",
        peak as f64 / index as f64
    );
}

#[test]
fn range_bound_rejects_before_allocating_ranges() {
    let _measure = MEASURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let file = worst_case_file();
    let index = index_len(&file);

    let (open_peak, result) = peak_of_open(&file, FormatReaderLimits::default());
    result.expect("worst-case page below the default range bound opens");
    assert!(
        open_peak > 4 * index,
        "fixture no longer amplifies: {open_peak} B for a {index} B page"
    );

    // 156 nodes x 256 ranges = 39 936: the bound rejects the page after about 20 nodes, long
    // before the ~10 MB of ranges of a full parse.
    let limits = FormatReaderLimits::default()
        .with_max_attribute_ranges(5_000)
        .unwrap();
    let (peak, result) = peak_of_open(&file, limits);
    assert!(
        matches!(
            result,
            Err(FormatError::CountLimit {
                kind: "node attribute ranges",
                limit: 5_000,
                ..
            })
        ),
        "{result:?}"
    );
    assert!(peak < 4 * index, "peak {peak} B for a {index} B page");
    assert!(
        FormatReaderLimits::default()
            .with_max_attribute_ranges(0)
            .is_err()
    );
}
