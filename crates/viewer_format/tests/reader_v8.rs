//! Reader behaviour that does not depend on the layout, checked on `v8` files: budgeted reads,
//! parallel readers, read plans, selective column reads, checksums before expansion and the
//! attribute codecs. Ported from the retired `v6` tests; `v8` has no monolithic layout, so every
//! file is a `.oct` / `.octattrs` pair.

mod common;
#[path = "common/v8_fixture.rs"]
mod v8_fixture;

use std::io::{Cursor, Read, Seek, SeekFrom};

use common::{
    COLORS, POINTS, descriptors, fixture_attributes, global_metadata, prefix_of,
    repair_index_and_footer,
};
use v8_fixture::{V8_BLOCK, v8_crs, v8_fixture_block, v8_metadata, with_v8_extras, write_v8_split};
use viewer_core::{
    AttributeColumn, AttributeDataType, AttributeDescriptor, AttributeValues, BlockId, ColorRgb,
    DatasetSource,
};
#[cfg(feature = "experimental")]
use viewer_core::{OccupiedOctreeRoot, OccupiedPointBlock, VoxelCoord};
use viewer_format::{
    BlockCodec, BlockPayloadKind, FormatError, FormatMetadata, FormatReader, FormatVersion,
    OctreeReader, ReadSelection, ReadSource, SelectedRead, SourcePointBlock,
};
use viewer_format::{GeometryEncoding, quanta_to_units, quantize_coordinate};
#[cfg(feature = "experimental")]
use viewer_format::{QuantizedOccupancy, quantize_local_positions};

const BLOCK_ID: BlockId = V8_BLOCK;

/// The shared four-point, ten-column fixture as a `v8` pair.
fn fixture_pair() -> (Vec<u8>, Vec<u8>) {
    write_v8_split(
        v8_fixture_block(),
        v8_metadata(POINTS.len() as u64, descriptors()),
    )
}

fn open_pair(primary: Vec<u8>, attributes: Vec<u8>) -> FormatReader<Cursor<Vec<u8>>> {
    let mut reader = FormatReader::new(Cursor::new(primary)).unwrap();
    reader.attach_attributes(Cursor::new(attributes)).unwrap();
    reader
}

fn assert_values_bits_eq(actual: &AttributeValues, expected: &AttributeValues) {
    match (actual, expected) {
        (AttributeValues::U8(a), AttributeValues::U8(e)) => assert_eq!(a, e),
        (AttributeValues::I8(a), AttributeValues::I8(e)) => assert_eq!(a, e),
        (AttributeValues::U16(a), AttributeValues::U16(e)) => assert_eq!(a, e),
        (AttributeValues::I16(a), AttributeValues::I16(e)) => assert_eq!(a, e),
        (AttributeValues::U32(a), AttributeValues::U32(e)) => assert_eq!(a, e),
        (AttributeValues::I32(a), AttributeValues::I32(e)) => assert_eq!(a, e),
        (AttributeValues::U64(a), AttributeValues::U64(e)) => assert_eq!(a, e),
        (AttributeValues::I64(a), AttributeValues::I64(e)) => assert_eq!(a, e),
        (AttributeValues::F32(a), AttributeValues::F32(e)) => {
            assert_eq!(
                a.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                e.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
            );
        }
        (AttributeValues::F64(a), AttributeValues::F64(e)) => {
            assert_eq!(
                a.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
                e.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
            );
        }
        _ => panic!("attribute type changed"),
    }
}

#[derive(Clone)]
struct CountingReader {
    cursor: Cursor<Vec<u8>>,
    bytes_read: std::rc::Rc<std::cell::Cell<usize>>,
}

impl CountingReader {
    fn new(bytes: Vec<u8>) -> Self {
        Self {
            cursor: Cursor::new(bytes),
            bytes_read: std::rc::Rc::new(std::cell::Cell::new(0)),
        }
    }
}

impl Read for CountingReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let read = self.cursor.read(buffer)?;
        self.bytes_read.set(self.bytes_read.get() + read);
        Ok(read)
    }
}

impl Seek for CountingReader {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        self.cursor.seek(position)
    }
}

#[derive(Clone)]
struct SwitchableReader {
    cursor: Cursor<Vec<u8>>,
    fail: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Read for SwitchableReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self.fail.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(std::io::Error::other("injected read failure"));
        }
        self.cursor.read(buffer)
    }
}

impl Seek for SwitchableReader {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        self.cursor.seek(position)
    }
}

#[derive(Clone)]
struct PanicSwitchReader {
    cursor: Cursor<Vec<u8>>,
    panic: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Read for PanicSwitchReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        assert!(
            !self.panic.load(std::sync::atomic::Ordering::SeqCst),
            "injected reader panic"
        );
        self.cursor.read(buffer)
    }
}

impl Seek for PanicSwitchReader {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        self.cursor.seek(position)
    }
}

#[derive(Clone)]
struct AtomicCountingReader {
    cursor: Cursor<Vec<u8>>,
    bytes_read: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl AtomicCountingReader {
    fn new(bytes: Vec<u8>) -> Self {
        Self {
            cursor: Cursor::new(bytes),
            bytes_read: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }
}

impl Read for AtomicCountingReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let read = self.cursor.read(buffer)?;
        self.bytes_read
            .fetch_add(read, std::sync::atomic::Ordering::SeqCst);
        Ok(read)
    }
}

impl Seek for AtomicCountingReader {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        self.cursor.seek(position)
    }
}

struct ReadOverlapGate {
    enabled: std::sync::atomic::AtomicBool,
    released: std::sync::Mutex<bool>,
    condition: std::sync::Condvar,
    entered: std::sync::mpsc::Sender<usize>,
}

impl ReadOverlapGate {
    fn release(&self) {
        *self
            .released
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        self.condition.notify_all();
    }
}

struct GatedReader {
    cursor: Cursor<Vec<u8>>,
    gate: std::sync::Arc<ReadOverlapGate>,
    reader_id: usize,
    waited: bool,
}

impl GatedReader {
    fn new(bytes: Vec<u8>, gate: std::sync::Arc<ReadOverlapGate>, reader_id: usize) -> Self {
        Self {
            cursor: Cursor::new(bytes),
            gate,
            reader_id,
            waited: false,
        }
    }
}

impl Read for GatedReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self.gate.enabled.load(std::sync::atomic::Ordering::SeqCst) && !self.waited {
            self.waited = true;
            self.gate
                .entered
                .send(self.reader_id)
                .map_err(std::io::Error::other)?;
            let mut released = self
                .gate
                .released
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            while !*released {
                released = self
                    .gate
                    .condition
                    .wait(released)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
        }
        self.cursor.read(buffer)
    }
}

impl Seek for GatedReader {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        self.cursor.seek(position)
    }
}

/// A primary with one reader per count: the first opens the pair and the rest fork it.
fn parallel_readers(
    primary: &[u8],
    attributes: &[u8],
    count: usize,
) -> Vec<FormatReader<Cursor<Vec<u8>>>> {
    let mut readers = vec![open_pair(primary.to_vec(), attributes.to_vec())];
    for _ in 1..count {
        let mut fork = readers[0].fork_reader(Cursor::new(primary.to_vec()));
        fork.attach_attributes(Cursor::new(attributes.to_vec()))
            .unwrap();
        readers.push(fork);
    }
    readers
}

#[test]
fn small_encoded_geometry_cannot_bypass_the_logical_reader_limit() {
    // v8 differs from v6 only in the byte counts: the logical size is what the limit sees.
    let count = 1000;
    let block = SourcePointBlock::new(
        V8_BLOCK,
        vec![[0.5; 3]; count],
        vec![ColorRgb::new(10, 20, 30); count],
        vec![],
    );
    let (primary, _) = write_v8_split(block, v8_metadata(count as u64, vec![]));
    let limits =
        viewer_format::FormatReaderLimits::new(1 << 20, 8000, 10, 10, 10, 10, 100).unwrap();
    assert!(matches!(
        FormatReader::with_limits(Cursor::new(primary), limits),
        Err(FormatError::BlockExceedsLimit {
            requested: 15000,
            limit: 8000
        })
    ));
}

#[test]
fn v8_preserves_geometry_colors_order_and_typed_attribute_bits() {
    let expected_attributes = fixture_attributes();
    let (primary, attributes) = fixture_pair();
    let mut reader = open_pair(primary, attributes);
    assert_eq!(reader.format_version(), FormatVersion::V8);

    let points = reader.read_geometry(BLOCK_ID).unwrap();
    assert_eq!(
        points
            .points()
            .iter()
            .map(|point| point.position)
            .collect::<Vec<_>>(),
        POINTS.map(|p| p.map(|v| v as f32))
    );
    assert_eq!(
        points
            .points()
            .iter()
            .map(|point| point.color)
            .collect::<Vec<_>>(),
        COLORS
    );
    for (index, expected) in expected_attributes.iter().enumerate() {
        let actual = reader.read_attribute(BLOCK_ID, index as u32).unwrap();
        assert_eq!(actual.descriptor(), expected.descriptor());
        assert_values_bits_eq(actual.values(), expected.values());
    }
}

#[test]
fn read_positions_without_companion_returns_neutral_gray() {
    let (primary, _) = fixture_pair();
    let mut reader = FormatReader::new(Cursor::new(primary)).unwrap();
    assert!(!reader.has_attributes());
    let points = reader.read_positions(BLOCK_ID).unwrap();
    assert!(
        points
            .points()
            .iter()
            .all(|point| point.color == ColorRgb::new(160, 160, 160))
    );
}

#[test]
fn retained_budgeted_output_blocks_a_fork_until_drop_then_resumes() {
    let (primary, attributes) = fixture_pair();
    let source = CountingReader::new(primary.clone());
    let primary_counter = source.bytes_read.clone();
    let mut reader = FormatReader::new(source).unwrap();
    let first_attributes = CountingReader::new(attributes.clone());
    reader.attach_attributes(first_attributes).unwrap();

    let companion = CountingReader::new(attributes);
    let companion_counter = companion.bytes_read.clone();
    let mut fork = reader.fork_reader(CountingReader::new(primary));
    fork.attach_attributes(companion).unwrap();
    let selection = ReadSelection::Attribute { schema_index: 6 };
    let output_bytes = reader
        .plan_read(BLOCK_ID, selection)
        .unwrap()
        .memory()
        .output_bytes();
    let peak_bytes = reader
        .plan_read(BLOCK_ID, selection)
        .unwrap()
        .memory()
        .peak_bytes();
    assert!(peak_bytes > output_bytes);
    let budget = viewer_format::ReadBudget::new(peak_bytes + output_bytes - 1);

    primary_counter.set(0);
    let held = reader
        .try_read_selected(BLOCK_ID, selection, &budget)
        .unwrap()
        .unwrap();
    assert_eq!(held.reserved_bytes(), output_bytes);
    assert_eq!(budget.reserved_bytes(), output_bytes);
    assert!(primary_counter.get() > 0 || budget.peak_reserved_bytes() == peak_bytes);
    let moved = held;
    assert!(matches!(moved.selected(), SelectedRead::Attribute(_)));
    assert_eq!(budget.reserved_bytes(), output_bytes);

    companion_counter.set(0);
    assert!(
        fork.try_read_selected(BLOCK_ID, selection, &budget)
            .unwrap()
            .is_none()
    );
    assert_eq!(companion_counter.get(), 0);
    assert_eq!(budget.reserved_bytes(), output_bytes);

    drop(moved);
    assert_eq!(budget.reserved_bytes(), 0);
    assert!(
        fork.try_read_selected(BLOCK_ID, selection, &budget)
            .unwrap()
            .is_some()
    );
    assert!(companion_counter.get() > 0);
}

#[test]
fn reserved_reads_reject_oversize_or_insufficient_quota_before_io() {
    let (primary, attributes) = fixture_pair();
    let source = CountingReader::new(primary);
    let primary_counter = source.bytes_read.clone();
    let companion = CountingReader::new(attributes);
    let companion_counter = companion.bytes_read.clone();
    let mut reader = FormatReader::new(source).unwrap();
    reader.attach_attributes(companion).unwrap();
    let selection = ReadSelection::Attribute { schema_index: 6 };
    let plan = reader.plan_read(BLOCK_ID, selection).unwrap();
    let peak = plan.memory().peak_bytes();

    primary_counter.set(0);
    companion_counter.set(0);
    let too_small = viewer_format::ReadBudget::new(peak - 1);
    assert!(matches!(
        reader.try_read_selected(BLOCK_ID, selection, &too_small),
        Err(FormatError::ReadBudgetExceeded { requested, limit })
            if requested == peak && limit == peak - 1
    ));
    assert_eq!(primary_counter.get(), 0);
    assert_eq!(companion_counter.get(), 0);
    assert_eq!(too_small.reserved_bytes(), 0);

    let insufficient = viewer_format::ReadBudget::new(peak);
    let reservation = insufficient.try_reserve(peak - 1).unwrap().unwrap();
    assert!(matches!(
        reader.read_reserved(BLOCK_ID, selection, reservation),
        Err(FormatError::ReadBudgetExceeded { requested, limit })
            if requested == peak && limit == peak - 1
    ));
    assert_eq!(primary_counter.get(), 0);
    assert_eq!(companion_counter.get(), 0);
    assert_eq!(insufficient.reserved_bytes(), 0);

    let enough = viewer_format::ReadBudget::new(peak);
    let reservation = enough.try_reserve(peak).unwrap().unwrap();
    let completed = reader
        .read_reserved(BLOCK_ID, selection, reservation)
        .unwrap();
    assert!(matches!(completed.selected(), SelectedRead::Attribute(_)));
    assert_eq!(completed.reserved_bytes(), plan.memory().output_bytes());
    assert_eq!(enough.reserved_bytes(), plan.memory().output_bytes());
    drop(completed);
    assert_eq!(enough.reserved_bytes(), 0);
}

#[test]
fn budgeted_read_errors_release_quota_after_crc_and_io_failures() {
    let (original, attributes) = fixture_pair();
    let selection = ReadSelection::Positions;
    let probe = FormatReader::new(Cursor::new(original.clone())).unwrap();
    let plan = probe.plan_read(BLOCK_ID, selection).unwrap();
    let range = plan.ranges()[0];
    let mut corrupt = original.clone();
    corrupt[range.offset() as usize] ^= 1;
    let source = CountingReader::new(corrupt);
    let mut reader = FormatReader::new(source).unwrap();
    let budget = viewer_format::ReadBudget::new(plan.memory().peak_bytes());
    assert!(matches!(
        reader.try_read_selected(BLOCK_ID, selection, &budget),
        Err(FormatError::InvalidBlockChecksum(_))
    ));
    assert_eq!(budget.reserved_bytes(), 0);

    // The same release holds when the corrupt payload is in the companion.
    let mut corrupt_attributes = attributes;
    let colors = open_pair(original.clone(), corrupt_attributes.clone())
        .plan_read(BLOCK_ID, ReadSelection::DisplayColors)
        .unwrap();
    corrupt_attributes[colors.ranges()[0].offset() as usize] ^= 1;
    let mut reader = open_pair(original.clone(), corrupt_attributes);
    let budget = viewer_format::ReadBudget::new(colors.memory().peak_bytes());
    assert!(matches!(
        reader.try_read_selected(BLOCK_ID, ReadSelection::DisplayColors, &budget),
        Err(FormatError::InvalidBlockChecksum(_))
    ));
    assert_eq!(budget.reserved_bytes(), 0);

    let fail = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut reader = FormatReader::new(SwitchableReader {
        cursor: Cursor::new(original),
        fail: std::sync::Arc::clone(&fail),
    })
    .unwrap();
    let budget = viewer_format::ReadBudget::new(
        reader
            .plan_read(BLOCK_ID, selection)
            .unwrap()
            .memory()
            .peak_bytes(),
    );
    fail.store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(matches!(
        reader.try_read_selected(BLOCK_ID, selection, &budget),
        Err(FormatError::Io(_))
    ));
    assert_eq!(budget.reserved_bytes(), 0);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn parallel_reads_deliver_in_order_with_one_or_two_readers() {
    use std::ops::ControlFlow;
    use viewer_format::{ParallelReadStatus, read_selected_parallel};

    let (bytes, attributes) = fixture_pair();
    let requests = [
        (BLOCK_ID, ReadSelection::DisplayColors),
        (BLOCK_ID, ReadSelection::Positions),
        (BLOCK_ID, ReadSelection::Attribute { schema_index: 6 }),
        (BLOCK_ID, ReadSelection::Geometry),
    ];
    for reader_count in [1, 2] {
        let mut readers = parallel_readers(&bytes, &attributes, reader_count);
        let budget = viewer_format::ReadBudget::new(u64::MAX);
        let mut delivered = Vec::new();
        let progress = read_selected_parallel(&mut readers, &requests, &budget, |index, result| {
            delivered.push(index);
            match (index, result) {
                (0, SelectedRead::DisplayColors(colors)) => {
                    assert_eq!(colors.as_slice(), COLORS.as_slice());
                }
                (1, SelectedRead::Positions(points)) => {
                    assert_eq!(points.len(), POINTS.len());
                }
                (2, SelectedRead::Attribute(column)) => {
                    assert_values_bits_eq(column.values(), fixture_attributes()[6].values());
                }
                (3, SelectedRead::Geometry(points)) => {
                    assert_eq!(points.len(), POINTS.len());
                    assert_eq!(
                        points
                            .points()
                            .iter()
                            .map(|point| point.color)
                            .collect::<Vec<_>>(),
                        COLORS
                    );
                }
                _ => panic!("parallel read returned the wrong variant at {index}"),
            }
            ControlFlow::Continue(())
        })
        .unwrap();
        assert_eq!(delivered, vec![0, 1, 2, 3]);
        assert_eq!(progress.delivered, requests.len());
        assert_eq!(progress.status, ParallelReadStatus::Complete);
        assert_eq!(budget.reserved_bytes(), 0);
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn parallel_reader_workers_overlap_payload_io_before_the_gate_opens() {
    use std::ops::ControlFlow;
    use std::time::{Duration, Instant};
    use viewer_format::{ParallelReadStatus, read_selected_parallel};

    let (bytes, _) = fixture_pair();
    let (entered, notices) = std::sync::mpsc::channel();
    let gate = std::sync::Arc::new(ReadOverlapGate {
        enabled: std::sync::atomic::AtomicBool::new(false),
        released: std::sync::Mutex::new(false),
        condition: std::sync::Condvar::new(),
        entered,
    });
    // Open and fork before enabling the gate, so only payload reads can block here.
    let root = FormatReader::new(GatedReader::new(bytes.clone(), gate.clone(), 0)).unwrap();
    let fork = root.fork_reader(GatedReader::new(bytes, gate.clone(), 1));
    let mut readers = vec![root, fork];
    let requests = [(BLOCK_ID, ReadSelection::Positions); 2];
    let budget = viewer_format::ReadBudget::new(u64::MAX);
    gate.enabled
        .store(true, std::sync::atomic::Ordering::SeqCst);

    let (both_entered_before_release, progress) = std::thread::scope(|scope| {
        let handle = scope.spawn(|| {
            read_selected_parallel(&mut readers, &requests, &budget, |_, result| {
                assert!(matches!(result, SelectedRead::Positions(_)));
                ControlFlow::Continue(())
            })
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        let first = notices.recv_timeout(deadline.saturating_duration_since(Instant::now()));
        let second = notices.recv_timeout(deadline.saturating_duration_since(Instant::now()));
        let overlapped = matches!((first, second), (Ok(a), Ok(b)) if a != b);
        // Always wake workers before joining, including the timeout/regression path.
        gate.release();
        (overlapped, handle.join().unwrap())
    });

    assert!(both_entered_before_release, "payload reads did not overlap");
    let progress = progress.unwrap();
    assert_eq!(progress.status, ParallelReadStatus::Complete);
    assert_eq!(progress.delivered, 2);
    assert_eq!(budget.reserved_bytes(), 0);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn parallel_quota_blocks_or_reduces_window_and_callback_break_counts_result() {
    use std::ops::ControlFlow;
    use viewer_format::{ParallelReadStatus, read_selected_parallel};

    let (bytes, attributes) = fixture_pair();
    let base = FormatReader::new(Cursor::new(bytes.clone())).unwrap();
    let peak = base
        .plan_read(BLOCK_ID, ReadSelection::Positions)
        .unwrap()
        .memory()
        .peak_bytes();
    let requests = [(BLOCK_ID, ReadSelection::Positions); 2];

    let mut readers = parallel_readers(&bytes, &attributes, 2);
    let budget = viewer_format::ReadBudget::new(peak);
    let mut delivered = Vec::new();
    let progress = read_selected_parallel(&mut readers, &requests, &budget, |index, result| {
        assert!(matches!(result, SelectedRead::Positions(_)));
        delivered.push(index);
        ControlFlow::Continue(())
    })
    .unwrap();
    assert_eq!(progress.status, ParallelReadStatus::Complete);
    assert_eq!(progress.delivered, 2);
    assert_eq!(delivered, [0, 1]);
    assert_eq!(budget.peak_reserved_bytes(), peak);
    assert_eq!(budget.reserved_bytes(), 0);

    let mut readers = parallel_readers(&bytes, &attributes, 2);
    let budget = viewer_format::ReadBudget::new(peak);
    let external = budget.try_reserve(peak).unwrap().unwrap();
    let blocked = read_selected_parallel(&mut readers, &requests[..1], &budget, |_, _| {
        ControlFlow::Continue(())
    })
    .unwrap();
    assert_eq!(blocked.status, ParallelReadStatus::Blocked);
    assert_eq!(blocked.delivered, 0);
    drop(external);
    let resumed = read_selected_parallel(&mut readers, &requests[..1], &budget, |_, _| {
        ControlFlow::Continue(())
    })
    .unwrap();
    assert_eq!(resumed.status, ParallelReadStatus::Complete);
    assert_eq!(resumed.delivered, 1);
    assert_eq!(budget.reserved_bytes(), 0);

    let mut readers = parallel_readers(&bytes, &attributes, 2);
    let budget = viewer_format::ReadBudget::new(u64::MAX);
    let cancelled = read_selected_parallel(&mut readers, &requests, &budget, |index, _| {
        assert_eq!(index, 0);
        ControlFlow::Break(())
    })
    .unwrap();
    assert_eq!(cancelled.status, ParallelReadStatus::Cancelled);
    assert_eq!(cancelled.delivered, 1);
    assert_eq!(budget.reserved_bytes(), 0);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn parallel_read_rejects_no_readers_and_independent_indexes_before_io() {
    use std::ops::ControlFlow;
    use viewer_format::{ParallelReadError, read_selected_parallel};

    let (bytes, _) = fixture_pair();
    let requests = [(BLOCK_ID, ReadSelection::Positions)];
    let budget = viewer_format::ReadBudget::new(u64::MAX);
    let no_readers: &mut [FormatReader<Cursor<Vec<u8>>>] = &mut [];
    assert!(matches!(
        read_selected_parallel(
            no_readers,
            &requests,
            &budget,
            |_, _| ControlFlow::Continue(())
        ),
        Err(ParallelReadError::NoReaders)
    ));

    let first = AtomicCountingReader::new(bytes.clone());
    let first_counter = first.bytes_read.clone();
    let second = AtomicCountingReader::new(bytes.clone());
    let second_counter = second.bytes_read.clone();
    let mut readers = vec![
        FormatReader::new(first).unwrap(),
        FormatReader::new(second).unwrap(),
    ];
    first_counter.store(0, std::sync::atomic::Ordering::SeqCst);
    second_counter.store(0, std::sync::atomic::Ordering::SeqCst);
    assert!(matches!(
        read_selected_parallel(&mut readers, &requests, &budget, |_, _| {
            ControlFlow::Continue(())
        }),
        Err(ParallelReadError::DifferentIndex { reader_index: 1 })
    ));
    assert_eq!(first_counter.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert_eq!(second_counter.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert_eq!(budget.reserved_bytes(), 0);

    // A bad request discovered while reserving a window must release earlier admissions
    // without launching them or reporting them as delivered.
    let fork = readers[0].fork_reader(AtomicCountingReader::new(bytes));
    readers.truncate(1);
    readers.push(fork);
    assert!(matches!(
        read_selected_parallel(
            &mut readers,
            &[
                (BLOCK_ID, ReadSelection::Positions),
                (BLOCK_ID, ReadSelection::Attribute { schema_index: 999 }),
            ],
            &budget,
            |_, _| panic!("a rejected window must not deliver results"),
        ),
        Err(ParallelReadError::Read {
            request_index: 1,
            delivered: 0,
            source: FormatError::UnknownAttribute(999),
        })
    ));
    assert_eq!(first_counter.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert_eq!(budget.reserved_bytes(), 0);
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn parallel_crc_worker_and_callback_panics_release_every_reservation() {
    use std::ops::ControlFlow;
    use viewer_format::{ParallelReadError, read_selected_parallel};

    let (original, attributes) = fixture_pair();
    let probe = open_pair(original.clone(), attributes.clone());
    let range = probe
        .plan_read(BLOCK_ID, ReadSelection::Positions)
        .unwrap()
        .ranges()[0];
    let mut corrupt = original.clone();
    corrupt[range.offset() as usize] ^= 1;
    let mut readers = parallel_readers(&corrupt, &attributes, 2);
    let budget = viewer_format::ReadBudget::new(u64::MAX);
    assert!(matches!(
        read_selected_parallel(
            &mut readers,
            &[(BLOCK_ID, ReadSelection::Positions); 2],
            &budget,
            |_, _| ControlFlow::Continue(())
        ),
        Err(ParallelReadError::Read {
            request_index: 0,
            delivered: 0,
            source: FormatError::InvalidBlockChecksum(_),
        })
    ));
    assert_eq!(budget.reserved_bytes(), 0);

    let color_range = probe
        .plan_read(BLOCK_ID, ReadSelection::DisplayColors)
        .unwrap()
        .ranges()[0];
    let bad_colors = original;
    let mut bad_attributes = attributes;
    bad_attributes[color_range.offset() as usize] ^= 1;
    let mut readers = parallel_readers(&bad_colors, &bad_attributes, 2);
    let budget = viewer_format::ReadBudget::new(u64::MAX);
    let mut delivered_indices = Vec::new();
    assert!(matches!(
        read_selected_parallel(
            &mut readers,
            &[
                (BLOCK_ID, ReadSelection::Positions),
                (BLOCK_ID, ReadSelection::DisplayColors),
                (BLOCK_ID, ReadSelection::Positions),
            ],
            &budget,
            |index, _| {
                delivered_indices.push(index);
                ControlFlow::Continue(())
            },
        ),
        Err(ParallelReadError::Read {
            request_index: 1,
            delivered: 1,
            source: FormatError::InvalidBlockChecksum(_),
        })
    ));
    assert_eq!(delivered_indices, [0]);
    assert_eq!(budget.reserved_bytes(), 0);

    let (bytes, attributes) = fixture_pair();
    let single_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let single = FormatReader::new(PanicSwitchReader {
        cursor: Cursor::new(bytes.clone()),
        panic: std::sync::Arc::clone(&single_flag),
    })
    .unwrap();
    single_flag.store(true, std::sync::atomic::Ordering::SeqCst);
    let mut readers = vec![single];
    let budget = viewer_format::ReadBudget::new(u64::MAX);
    assert!(matches!(
        read_selected_parallel(
            &mut readers,
            &[(BLOCK_ID, ReadSelection::Positions)],
            &budget,
            |_, _| ControlFlow::Continue(())
        ),
        Err(ParallelReadError::WorkerPanicked {
            request_index: 0,
            delivered: 0,
        })
    ));
    assert_eq!(budget.reserved_bytes(), 0);

    let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let root = FormatReader::new(PanicSwitchReader {
        cursor: Cursor::new(bytes.clone()),
        panic: std::sync::Arc::clone(&flag),
    })
    .unwrap();
    let fork = root.fork_reader(PanicSwitchReader {
        cursor: Cursor::new(bytes.clone()),
        panic: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    });
    flag.store(true, std::sync::atomic::Ordering::SeqCst);
    let mut readers = vec![root, fork];
    let budget = viewer_format::ReadBudget::new(u64::MAX);
    assert!(matches!(
        read_selected_parallel(
            &mut readers,
            &[(BLOCK_ID, ReadSelection::Positions); 2],
            &budget,
            |_, _| ControlFlow::Continue(())
        ),
        Err(ParallelReadError::WorkerPanicked {
            request_index: 0,
            delivered: 0,
        })
    ));
    assert_eq!(budget.reserved_bytes(), 0);

    let mut readers = parallel_readers(&bytes, &attributes, 2);
    let budget = viewer_format::ReadBudget::new(u64::MAX);
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = read_selected_parallel(
            &mut readers,
            &[(BLOCK_ID, ReadSelection::Positions); 2],
            &budget,
            |_, _| -> ControlFlow<()> { panic!("injected callback panic") },
        );
    }));
    assert!(panic.is_err());
    assert_eq!(budget.reserved_bytes(), 0);
}
#[test]
fn read_plans_are_selective_io_free_and_match_selected_payload_ranges() {
    // Payload order of a pair: geometry (primary), display colour, then one entry per column.
    let selections = [
        (ReadSelection::Positions, vec![0]),
        (ReadSelection::Geometry, vec![0, 1]),
        (ReadSelection::DisplayColors, vec![1]),
        (ReadSelection::Attribute { schema_index: 6 }, vec![8]),
    ];
    let (primary_bytes, attribute_bytes) = fixture_pair();
    let primary = CountingReader::new(primary_bytes.clone());
    let attributes = CountingReader::new(attribute_bytes.clone());
    let primary_counter = primary.bytes_read.clone();
    let attribute_counter = attributes.bytes_read.clone();
    let mut reader = FormatReader::new(primary).unwrap();
    reader.attach_attributes(attributes).unwrap();
    let payload_stats = reader.block_payload_storage_stats().unwrap();
    for (selection, payload_indices) in &selections {
        primary_counter.set(0);
        attribute_counter.set(0);
        let plan = reader.plan_read(BLOCK_ID, *selection).unwrap();
        assert_eq!(plan.block_id(), BLOCK_ID);
        assert_eq!(plan.selection(), *selection);
        assert_eq!(plan.ranges().len(), payload_indices.len());
        assert_eq!(primary_counter.get(), 0, "plan performed primary I/O");
        assert_eq!(attribute_counter.get(), 0, "plan performed companion I/O");

        let mut expected_primary_bytes = 0_u64;
        let mut expected_attribute_bytes = 0_u64;
        for (range, &payload_index) in plan.ranges().iter().zip(payload_indices) {
            let stats = &payload_stats[payload_index];
            let is_attribute = payload_index > 0;
            let source = if is_attribute {
                ReadSource::Attributes
            } else {
                ReadSource::Primary
            };
            // The companion starts after the header with the payloads that follow geometry.
            let start = usize::from(payload_index != 0);
            let preceding_bytes = payload_stats[start..payload_index]
                .iter()
                .map(|payload| payload.stored_bytes)
                .sum::<u64>();
            let expected_offset = 24 + preceding_bytes;
            assert_eq!(range.source(), source);
            assert_eq!(range.offset(), expected_offset);
            assert_eq!(range.stored_bytes(), stats.stored_bytes);
            assert_eq!(range.decoded_bytes(), stats.raw_bytes);
            assert_eq!(range.codec(), stats.codec);
            let source_bytes = if is_attribute {
                &attribute_bytes
            } else {
                &primary_bytes
            };
            let start = expected_offset as usize;
            let end = start + stats.stored_bytes as usize;
            assert_eq!(
                range.checksum(),
                viewer_format::crc32(&source_bytes[start..end])
            );
            if is_attribute {
                expected_attribute_bytes += stats.stored_bytes;
            } else {
                expected_primary_bytes += stats.stored_bytes;
            }
        }

        let output_bytes = match selection {
            ReadSelection::Positions | ReadSelection::Geometry => {
                (POINTS.len() * std::mem::size_of::<viewer_core::Point>()) as u64
            }
            ReadSelection::DisplayColors => (POINTS.len() * std::mem::size_of::<ColorRgb>()) as u64,
            ReadSelection::Attribute { schema_index: 6 } => {
                (POINTS.len() * std::mem::size_of::<f64>()) as u64
            }
            ReadSelection::Attribute { .. } => unreachable!(),
        };
        assert_eq!(plan.memory().output_bytes(), output_bytes);
        assert!(plan.memory().peak_bytes() >= output_bytes);

        match reader
            .read_selected(BLOCK_ID, *selection, plan.memory().peak_bytes())
            .unwrap()
        {
            SelectedRead::Positions(points) if *selection == ReadSelection::Positions => {
                assert_eq!(points.len(), POINTS.len());
                assert!(
                    points
                        .points()
                        .iter()
                        .all(|point| { point.color == ColorRgb::new(160, 160, 160) })
                );
            }
            SelectedRead::Geometry(points) if *selection == ReadSelection::Geometry => {
                assert_eq!(points.len(), POINTS.len());
                assert_eq!(
                    points
                        .points()
                        .iter()
                        .map(|point| point.color)
                        .collect::<Vec<_>>(),
                    COLORS
                );
            }
            SelectedRead::DisplayColors(colors) if *selection == ReadSelection::DisplayColors => {
                assert_eq!(colors, COLORS);
            }
            SelectedRead::Attribute(column)
                if *selection == (ReadSelection::Attribute { schema_index: 6 }) =>
            {
                assert_values_bits_eq(column.values(), fixture_attributes()[6].values());
            }
            _ => panic!("wrong selected read result for {selection:?}"),
        }
        assert_eq!(primary_counter.get() as u64, expected_primary_bytes);
        assert_eq!(attribute_counter.get() as u64, expected_attribute_bytes);
        assert!(plan.ranges().len() <= 2);
    }
}

#[test]
fn selected_read_budget_is_checked_before_any_payload_io() {
    let (primary, attributes) = fixture_pair();
    let source = CountingReader::new(primary);
    let companion = CountingReader::new(attributes);
    let primary_counter = source.bytes_read.clone();
    let companion_counter = companion.bytes_read.clone();
    let mut reader = FormatReader::new(source).unwrap();
    reader.attach_attributes(companion).unwrap();
    let plan = reader
        .plan_read(BLOCK_ID, ReadSelection::Attribute { schema_index: 6 })
        .unwrap();
    let peak = plan.memory().peak_bytes();
    assert!(peak > 0);

    primary_counter.set(0);
    companion_counter.set(0);
    assert!(matches!(
        reader.read_selected(BLOCK_ID, ReadSelection::Attribute { schema_index: 6 }, peak - 1),
        Err(FormatError::ReadBudgetExceeded { requested, limit })
            if requested == peak && limit == peak - 1
    ));
    assert_eq!(primary_counter.get(), 0);
    assert_eq!(companion_counter.get(), 0);

    let column = reader
        .read_selected(BLOCK_ID, ReadSelection::Attribute { schema_index: 6 }, peak)
        .unwrap();
    assert!(matches!(column, SelectedRead::Attribute(_)));
    assert_eq!(primary_counter.get(), 0);
    assert!(companion_counter.get() > 0);
}

#[test]
fn read_plans_reject_invalid_requests_before_io_and_preserve_missing_companion_behavior() {
    let (primary, _) = fixture_pair();
    let source = CountingReader::new(primary);
    let primary_counter = source.bytes_read.clone();
    let mut reader = FormatReader::new(source).unwrap();

    primary_counter.set(0);
    assert!(matches!(
        reader.plan_read(BlockId::new(999), ReadSelection::Positions),
        Err(FormatError::UnknownBlock(_))
    ));
    assert!(matches!(
        reader.plan_read(BLOCK_ID, ReadSelection::Attribute { schema_index: 999 }),
        Err(FormatError::UnknownAttribute(999))
    ));
    assert!(matches!(
        reader.read_selected(
            BLOCK_ID,
            ReadSelection::Attribute { schema_index: 999 },
            u64::MAX
        ),
        Err(FormatError::UnknownAttribute(999))
    ));
    assert_eq!(primary_counter.get(), 0);

    let position_plan = reader
        .plan_read(BLOCK_ID, ReadSelection::Positions)
        .unwrap();
    let geometry_plan = reader.plan_read(BLOCK_ID, ReadSelection::Geometry).unwrap();
    assert_eq!(position_plan.ranges().len(), 1);
    assert_eq!(geometry_plan.ranges().len(), 1);
    assert_eq!(geometry_plan.ranges()[0].source(), ReadSource::Primary);
    assert!(matches!(
        reader.plan_read(BLOCK_ID, ReadSelection::DisplayColors),
        Err(FormatError::AttributesUnavailable)
    ));
    assert!(matches!(
        reader.plan_read(BLOCK_ID, ReadSelection::Attribute { schema_index: 0 }),
        Err(FormatError::AttributesUnavailable)
    ));

    primary_counter.set(0);
    let geometry = reader
        .read_selected(
            BLOCK_ID,
            ReadSelection::Geometry,
            geometry_plan.memory().peak_bytes(),
        )
        .unwrap();
    let SelectedRead::Geometry(points) = geometry else {
        panic!("expected the geometry result")
    };
    assert!(
        points
            .points()
            .iter()
            .all(|point| { point.color == ColorRgb::new(160, 160, 160) })
    );
    assert!(primary_counter.get() > 0);
    assert!(matches!(
        reader.read_selected(BLOCK_ID, ReadSelection::DisplayColors, u64::MAX),
        Err(FormatError::AttributesUnavailable)
    ));
}

#[test]
fn split_payload_reads_are_selective_for_geometry_and_one_attribute_column() {
    let (primary, attributes) = fixture_pair();
    let primary = CountingReader::new(primary);
    let attribute_file = CountingReader::new(attributes);
    let primary_counter = primary.bytes_read.clone();
    let attributes_counter = attribute_file.bytes_read.clone();
    let mut reader = FormatReader::new(primary).unwrap();
    reader.attach_attributes(attribute_file).unwrap();

    primary_counter.set(0);
    attributes_counter.set(0);
    reader.read_positions(BLOCK_ID).unwrap();
    assert!(primary_counter.get() > 0);
    assert_eq!(attributes_counter.get(), 0);

    primary_counter.set(0);
    attributes_counter.set(0);
    reader.read_attribute(BLOCK_ID, 6).unwrap();
    assert_eq!(primary_counter.get(), 0);
    assert!(attributes_counter.get() > 0);
}

#[test]
fn split_companions_never_serve_another_files_payloads_and_forks_keep_the_detected_version() {
    // A companion of another revision (e.g. v8.1-pre) is refused at attach by identity (see
    // `v8_lod.rs`). A companion of another V8 file with a different shape is refused at attach;
    // one with the same shape attaches, because the paged footer CRC covers only the header and
    // the root page, and fails at the first read through the per-payload checksum (issue #115).
    let (primary, attributes) = fixture_pair();
    let (other_block, other_meta, _) = codec_fixture();
    let (_, other_shape) = write_v8_split(other_block, other_meta);
    let mut reader = FormatReader::new(Cursor::new(primary.clone())).unwrap();
    assert!(reader.attach_attributes(Cursor::new(other_shape)).is_err());

    let same_shape = SourcePointBlock::new(
        V8_BLOCK,
        POINTS.to_vec(),
        COLORS.map(|c| ColorRgb::new(c.r ^ 1, c.g, c.b)).to_vec(),
        fixture_attributes(),
    );
    let (_, other_attributes) =
        write_v8_split(same_shape, v8_metadata(POINTS.len() as u64, descriptors()));
    assert_ne!(attributes, other_attributes);
    let mut reader = FormatReader::new(Cursor::new(primary.clone())).unwrap();
    // Current behaviour pinned (issue #115): attach succeeds, the first read fails.
    reader
        .attach_attributes(Cursor::new(other_attributes))
        .expect("issue #115: a same-shape companion attaches today");
    assert!(matches!(
        reader.read_display_colors(BLOCK_ID),
        Err(FormatError::InvalidBlockChecksum(_))
    ));

    let reader = FormatReader::new(Cursor::new(primary.clone())).unwrap();
    let mut fork = reader.fork_reader(Cursor::new(primary));
    assert_eq!(fork.format_version(), FormatVersion::V8);
    fork.attach_attributes(Cursor::new(attributes)).unwrap();
    assert_eq!(fork.read_geometry(BLOCK_ID).unwrap().len(), POINTS.len());
}

#[cfg(feature = "experimental")]
#[test]
fn occupied_cells_keep_rows_and_attribute_bits_from_v8_readers() {
    let a = VoxelCoord::new(-1, 0, 0);
    let b = VoxelCoord::new(0, -1, 0);
    let cells = [a, b, a, b];
    for attach in [false, true] {
        let (primary, attributes) = fixture_pair();
        let mut reader = FormatReader::new(Cursor::new(primary)).unwrap();
        if attach {
            reader.attach_attributes(Cursor::new(attributes)).unwrap();
        }
        let source = reader.read_block(BLOCK_ID).unwrap();
        let occupied =
            OccupiedPointBlock::new(OccupiedOctreeRoot::new([-2; 3], 2).unwrap(), source, &cells)
                .unwrap();
        let mut all_rows = Vec::new();
        for (cell, rows) in [(a, [0, 2]), (b, [1, 3])] {
            assert_eq!(occupied.rows_for_cell(cell), Some(rows.as_slice()));
            let selected = occupied.select_cell(cell).unwrap().unwrap();
            assert_eq!(selected.id(), BLOCK_ID);
            assert_eq!(selected.points().len(), rows.len());
            for (point, row) in selected.points().points().iter().zip(rows) {
                let expected = occupied.source().points().points()[row];
                assert_eq!(
                    point.position.map(f32::to_bits),
                    expected.position.map(f32::to_bits)
                );
                assert_eq!(point.color, expected.color);
            }
            assert_eq!(
                selected.attributes().len(),
                occupied.source().attributes().len()
            );
            for (column, original) in selected
                .attributes()
                .iter()
                .zip(occupied.source().attributes())
            {
                assert_eq!(column.descriptor(), original.descriptor());
                assert_values_bits_eq(
                    column.values(),
                    &original.values().select_rows(&rows, 1).unwrap(),
                );
            }
            all_rows.extend(rows);
        }
        all_rows.sort_unstable();
        assert_eq!(all_rows, vec![0, 1, 2, 3]);
        assert_eq!(reader.format_version(), FormatVersion::V8);
    }
}

#[cfg(feature = "experimental")]
#[test]
fn quantized_occupancy_preserves_row_identity_for_geometry_rgb_and_all_attribute_types() {
    let original = quantize_local_positions(&POINTS, 0.001).unwrap();
    let mut attributes = fixture_attributes();
    attributes.push(
        AttributeColumn::new(
            AttributeDescriptor::new("pair64", viewer_core::AttributeDataType::U64, 2).unwrap(),
            AttributeValues::U64(vec![u64::MAX, 1, 2, 3, 4, 5, 6, 9_007_199_254_740_993]),
            POINTS.len(),
        )
        .unwrap(),
    );
    for bits in [0, 1, 16, 24, 31, 32] {
        let candidate = QuantizedOccupancy::new(&original, bits).unwrap();
        let reconstructed = candidate.reconstruct();
        assert_eq!(reconstructed.coordinates(), original.coordinates());
        let expected = original.decode_local().unwrap();
        let source = viewer_core::PointBlock::new(
            BLOCK_ID,
            reconstructed
                .decode_local()
                .unwrap()
                .into_iter()
                .zip(COLORS)
                .map(|(position, color)| viewer_core::Point::new(position, color))
                .collect::<Vec<_>>()
                .into(),
            attributes.clone(),
        );
        let mut cells = vec![VoxelCoord::new(0, 0, 0); POINTS.len()];
        for (leaf, cell) in candidate.index().topology().cells().enumerate() {
            for &row in candidate.index().rows_for_leaf(leaf).unwrap() {
                cells[row] = cell;
            }
        }
        let bound =
            OccupiedPointBlock::new(candidate.index().topology().root(), source, &cells).unwrap();
        let mut seen = Vec::new();
        for cell in candidate.index().topology().cells() {
            let rows = candidate.index().rows_for_cell(cell).unwrap();
            assert_eq!(bound.rows_for_cell(cell), Some(rows));
            let selected = bound.select_cell(cell).unwrap().unwrap();
            for (point, &row) in selected.points().points().iter().zip(rows) {
                assert_eq!(
                    point.position.map(f32::to_bits),
                    expected[row].map(f32::to_bits)
                );
                assert_eq!(point.color, COLORS[row]);
            }
            for (actual, original) in selected.attributes().iter().zip(&attributes) {
                assert_eq!(actual.descriptor(), original.descriptor());
                assert_values_bits_eq(
                    actual.values(),
                    &original
                        .values()
                        .select_rows(rows, original.descriptor().component_count() as usize)
                        .unwrap(),
                );
            }
            seen.extend_from_slice(rows);
        }
        seen.sort_unstable();
        assert_eq!(seen, vec![0, 1, 2, 3]);
    }
}

#[test]
fn v8_preserves_every_row_of_a_large_block_up_to_leaf_order() {
    let count = 4096;
    let mut state = 0x1234_5678_u32;
    let positions: Vec<_> = (0..count)
        .map(|_| {
            std::array::from_fn(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let cell = (state >> 28) % 4;
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (f64::from(cell) + f64::from(state) / f64::from(u32::MAX) * 0.01) / 4.0
            })
        })
        .collect();
    let colors: Vec<_> = (0..count)
        .map(|i| ColorRgb::new(i as u8, (i >> 8) as u8, 23))
        .collect();
    let descriptor = AttributeDescriptor::new("row64", AttributeDataType::U64, 1).unwrap();
    let values = AttributeValues::U64(
        (0..count)
            .map(|i| 9_007_199_254_740_993 + i as u64)
            .collect(),
    );
    let column = AttributeColumn::new(descriptor.clone(), values.clone(), count).unwrap();
    let block = SourcePointBlock::new(V8_BLOCK, positions.clone(), colors.clone(), vec![column]);
    let (primary, attrs) = write_v8_split(block, v8_metadata(count as u64, vec![descriptor]));
    let mut reader = open_pair(primary, attrs);
    let geometry_plan = reader
        .plan_read(BLOCK_ID, ReadSelection::Positions)
        .unwrap();
    let geometry_payload = reader.block_payload_storage_stats().unwrap()[0].clone();
    assert_eq!(geometry_plan.ranges().len(), 1);
    assert_eq!(
        geometry_plan.ranges()[0].decoded_bytes(),
        geometry_payload.raw_bytes
    );
    assert_eq!(
        geometry_plan.memory().decoded_bytes(),
        geometry_payload.raw_bytes
    );
    assert_eq!(
        geometry_plan.memory().output_bytes(),
        count as u64 * std::mem::size_of::<viewer_core::Point>() as u64
    );
    assert!(
        geometry_plan.memory().peak_bytes()
            >= geometry_payload.raw_bytes + geometry_plan.memory().output_bytes()
    );
    // v8 re-sorts the rows of a leaf: the colour is unique per input row, so it names the row.
    // v8 stores positions on the global 2^-15 grid: the expected value is the grid point.
    let quanta: Vec<[i64; 3]> = positions
        .iter()
        .map(|p| p.map(|c| quantize_coordinate(c).unwrap()))
        .collect();
    let expected_positions: Vec<[f32; 3]> = quanta
        .iter()
        .map(|q| q.map(|v| quanta_to_units(v) as f32))
        .collect();
    let block = reader.read_block(BLOCK_ID).unwrap();
    assert_eq!(block.points().len(), count);
    let AttributeValues::U64(read_values) = block.attributes()[0].values() else {
        panic!("attribute type changed")
    };
    let mut seen = vec![false; count];
    let mut order = Vec::with_capacity(count);
    for (row, point) in block.points().points().iter().enumerate() {
        let input = usize::from(point.color.r) | usize::from(point.color.g) << 8;
        assert!(!std::mem::replace(&mut seen[input], true), "row repeated");
        assert_eq!(point.color, colors[input]);
        assert_eq!(read_values[row], 9_007_199_254_740_993 + input as u64);
        // Bit-exact: the stored position is the quantised input one.
        assert_eq!(
            point.position.map(f32::to_bits),
            expected_positions[input].map(f32::to_bits),
            "row {input}"
        );
        order.push(input);
    }
    assert!(seen.into_iter().all(|row| row));
    // The hybrid row order: when the block is stored as Morton-sorted delta, rows are ordered by
    // (Morton key of the quantised cell, input row).
    assert_eq!(
        reader.block_storage_stats().unwrap()[0].geometry_encoding,
        GeometryEncoding::DeltaVarint
    );
    let mut sorted: Vec<usize> = (0..count).collect();
    // Blocks are 2^32 quanta wide, so the low 32 bits of a coordinate are its block-local one.
    sorted.sort_by_key(|&row| (morton_key(quanta[row].map(|v| v as u32)), row));
    assert_eq!(order, sorted);
}

/// 96-bit Morton key, x in the high bit of each triple (the writer's hybrid sort key).
fn morton_key(coordinate: [u32; 3]) -> u128 {
    let mut key = 0_u128;
    for bit in 0..32 {
        for (axis, value) in coordinate.iter().enumerate() {
            key |= u128::from((value >> bit) & 1) << (3 * bit + (2 - axis));
        }
    }
    key
}

/// 4096 rows drawn from the four fixture rows, so every column has few distinct values (Dictionary
/// candidates), plus a constant two-component NaN vector (Constant).
fn codec_fixture() -> (SourcePointBlock, FormatMetadata, Vec<usize>) {
    let rows = 4096;
    let mut state = 0x91ab_032f_u32;
    let indices: Vec<usize> = (0..rows)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state % 4) as usize
        })
        .collect();
    let mut columns: Vec<AttributeColumn> = fixture_attributes()
        .into_iter()
        .map(|col| {
            AttributeColumn::new(
                col.descriptor().clone(),
                col.values().select_rows(&indices, 1).unwrap(),
                rows,
            )
            .unwrap()
        })
        .collect();
    columns.push(
        AttributeColumn::new(
            AttributeDescriptor::new("constant_nan_vector", AttributeDataType::F64, 2).unwrap(),
            AttributeValues::F64(
                (0..rows)
                    .flat_map(|_| [f64::from_bits(0x7ff8_0000_0000_0042), -0.0])
                    .collect(),
            ),
            rows,
        )
        .unwrap(),
    );
    let schema = columns.iter().map(|c| c.descriptor().clone()).collect();
    (
        SourcePointBlock::new(
            V8_BLOCK,
            indices.iter().map(|i| POINTS[*i]).collect(),
            indices.iter().map(|i| COLORS[*i]).collect(),
            columns,
        ),
        v8_metadata(rows as u64, schema),
        indices,
    )
}

/// `v8` re-sorts the rows of a leaf, so a column read back is compared in the order of the read
/// positions: each output row is one of the four fixture points, whose first input row carries
/// the expected tuple of every column.
fn expected_in_output_order<R: Read + Seek>(
    reader: &mut FormatReader<R>,
    columns: &[AttributeColumn],
    indices: &[usize],
    column: usize,
) -> AttributeValues {
    let rows: Vec<usize> = reader
        .read_positions(BLOCK_ID)
        .unwrap()
        .points()
        .iter()
        .map(|point| {
            let fixture = POINTS
                .iter()
                .position(|p| p.map(|v| v as f32) == point.position)
                .expect("output row is a fixture point");
            indices.iter().position(|&i| i == fixture).unwrap()
        })
        .collect();
    let components = columns[column].descriptor().component_count() as usize;
    columns[column]
        .values()
        .select_rows(&rows, components)
        .unwrap()
}

#[test]
fn attribute_codecs_preserve_typed_bits_and_read_only_the_requested_column() {
    let (block, meta, indices) = codec_fixture();
    let expected = block.attributes().to_vec();
    let (primary, attributes) = write_v8_split(block, meta);
    let primary = CountingReader::new(primary);
    let attributes = CountingReader::new(attributes);
    let p_count = primary.bytes_read.clone();
    let a_count = attributes.bytes_read.clone();
    let mut reader = FormatReader::new(primary).unwrap();
    reader.attach_attributes(attributes).unwrap();
    let stats = reader.block_payload_storage_stats().unwrap();
    assert!(stats.iter().any(|p| p.codec == BlockCodec::Dictionary));
    assert_eq!(stats.last().unwrap().codec, BlockCodec::Constant);
    // The display colour is a plain LZ4 payload in v8 (v6.pre3 stored it as a dictionary).
    assert_eq!(stats[1].codec, BlockCodec::Lz4);
    let (dictionary_index, dictionary_stats) = stats
        .iter()
        .find_map(|payload| match &payload.kind {
            BlockPayloadKind::Attribute { schema_index, .. }
                if payload.codec == BlockCodec::Dictionary =>
            {
                Some((*schema_index, payload))
            }
            _ => None,
        })
        .unwrap();
    p_count.set(0);
    a_count.set(0);
    let dictionary_plan = reader
        .plan_read(
            BLOCK_ID,
            ReadSelection::Attribute {
                schema_index: dictionary_index,
            },
        )
        .unwrap();
    assert_eq!(p_count.get(), 0);
    assert_eq!(a_count.get(), 0);
    assert_eq!(dictionary_plan.ranges().len(), 1);
    assert_eq!(dictionary_plan.ranges()[0].codec(), BlockCodec::Dictionary);
    assert!(
        dictionary_plan.memory().peak_bytes()
            >= dictionary_stats.stored_bytes + dictionary_stats.raw_bytes * 3
    );
    let selected = reader
        .read_selected(
            BLOCK_ID,
            ReadSelection::Attribute {
                schema_index: dictionary_index,
            },
            dictionary_plan.memory().peak_bytes(),
        )
        .unwrap();
    let SelectedRead::Attribute(selected) = selected else {
        panic!("expected selected dictionary attribute")
    };
    assert_values_bits_eq(
        selected.values(),
        &expected_in_output_order(&mut reader, &expected, &indices, dictionary_index as usize),
    );
    for index in 0..expected.len() {
        let in_order = expected_in_output_order(&mut reader, &expected, &indices, index);
        p_count.set(0);
        a_count.set(0);
        let actual = reader.read_attribute(BLOCK_ID, index as u32).unwrap();
        assert_values_bits_eq(actual.values(), &in_order);
        let payload = stats.iter().find(|p| matches!(p.kind, BlockPayloadKind::Attribute { schema_index, .. } if schema_index == index as u32)).unwrap();
        assert_eq!(p_count.get(), 0);
        assert_eq!(a_count.get() as u64, payload.stored_bytes);
    }
    assert_eq!(reader.read_display_colors(BLOCK_ID).unwrap().len(), 4096);

    // A companion of another revision (v8.1-pre) is refused at attach by identity, as v6.pre2 was
    // for a pre3 sidecar.
    let (primary, mut attributes) = fixture_pair();
    attributes_as_pre1(&mut attributes);
    let mut wrong = FormatReader::new(Cursor::new(primary)).unwrap();
    assert!(matches!(
        wrong.attach_attributes(Cursor::new(attributes)),
        Err(FormatError::InvalidHeader)
    ));
}

/// Relabels a final `v8` attributes file as `v8.1-pre`: header revision and checksum, footer magic
/// and footer checksum (see `v8_lod.rs` for the full pair retag).
fn attributes_as_pre1(attributes: &mut [u8]) {
    attributes[16..20].copy_from_slice(&1_u32.to_le_bytes());
    let crc = viewer_format::crc32(&attributes[..20]);
    attributes[20..24].copy_from_slice(&crc.to_le_bytes());
    let footer = attributes.len() - 40;
    assert_eq!(&attributes[footer..footer + 8], b"OCTVEND8");
    attributes[footer..footer + 8].copy_from_slice(b"OCTVE81P");
    let crc = viewer_format::crc32(&attributes[footer..footer + 36]);
    attributes[footer + 36..].copy_from_slice(&crc.to_le_bytes());
}

#[test]
fn dictionary_and_constant_columns_preserve_typed_bits() {
    let (block, meta, indices) = codec_fixture();
    let expected = block.attributes().to_vec();
    let (primary, attributes) = write_v8_split(block, meta);
    let mut reader = open_pair(primary, attributes);
    let stats = reader.block_payload_storage_stats().unwrap();
    assert!(
        stats
            .iter()
            .any(|payload| payload.codec == BlockCodec::Dictionary)
    );
    assert_eq!(stats.last().unwrap().codec, BlockCodec::Constant);
    for index in 0..expected.len() {
        let in_order = expected_in_output_order(&mut reader, &expected, &indices, index);
        assert_values_bits_eq(
            reader
                .read_attribute(BLOCK_ID, index as u32)
                .unwrap()
                .values(),
            &in_order,
        );
    }
}

#[test]
fn attribute_checksum_is_checked_before_expansion() {
    let (block, meta, _) = codec_fixture();
    let (primary, mut attributes) = write_v8_split(block, meta);
    let reader = FormatReader::new(Cursor::new(primary.clone())).unwrap();
    let stats = reader.block_payload_storage_stats().unwrap();
    let constant_start = 24
        + stats[1..stats.len() - 1]
            .iter()
            .map(|p| p.stored_bytes as usize)
            .sum::<usize>();
    attributes[constant_start] ^= 1;
    let mut reader = FormatReader::new(Cursor::new(primary)).unwrap();
    reader.attach_attributes(Cursor::new(attributes)).unwrap();
    assert!(matches!(
        reader.read_attribute(BLOCK_ID, 10),
        Err(FormatError::InvalidBlockChecksum(BLOCK_ID))
    ));
}

#[test]
fn byte_planes_are_selective_and_exact() {
    let rows = 4096;
    let mut state = 1234_u32;
    let values = AttributeValues::F64(
        (0..rows)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                f64::from_bits(0x7ff8_0000_0000_0000 | u64::from(state))
            })
            .collect(),
    );
    let descriptor = AttributeDescriptor::new("nan_payload", AttributeDataType::F64, 1).unwrap();
    let column = AttributeColumn::new(descriptor.clone(), values.clone(), rows).unwrap();
    let block = SourcePointBlock::new(
        V8_BLOCK,
        (0..rows).map(|i| [i as f64 / 8192.0, 0.5, 0.25]).collect(),
        vec![ColorRgb::new(1, 2, 3); rows],
        vec![column],
    );
    let (primary, attrs) =
        write_v8_split(block, v8_metadata(rows as u64, vec![descriptor.clone()]));
    let p = CountingReader::new(primary);
    let a = CountingReader::new(attrs);
    let pc = p.bytes_read.clone();
    let ac = a.bytes_read.clone();
    let mut reader = FormatReader::new(p).unwrap();
    reader.attach_attributes(a).unwrap();
    let stats = reader.block_payload_storage_stats().unwrap();
    let payload = stats
        .iter()
        .find(|p| {
            matches!(
                p.kind,
                BlockPayloadKind::Attribute {
                    schema_index: 0,
                    ..
                }
            )
        })
        .unwrap();
    assert_eq!(payload.codec, BlockCodec::BytePlanes);
    assert!(payload.stored_bytes < payload.raw_bytes);
    pc.set(0);
    ac.set(0);
    assert_values_bits_eq(
        reader.read_attribute(BLOCK_ID, 0).unwrap().values(),
        &values,
    );
    assert_eq!(pc.get(), 0);
    assert_eq!(ac.get() as u64, payload.stored_bytes);

    // A byte-plane companion of another file: a different shape is refused at attach; the same
    // shape attaches and fails at the first read through the payload checksum (issue #115).
    let nan_column = |seed: u32| {
        let mut state = seed;
        let values = AttributeValues::F64(
            (0..rows)
                .map(|_| {
                    state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    f64::from_bits(0x7ff8_0000_0000_0000 | u64::from(state))
                })
                .collect(),
        );
        AttributeColumn::new(descriptor.clone(), values, rows).unwrap()
    };
    let other_block = |column| {
        SourcePointBlock::new(
            V8_BLOCK,
            (0..rows).map(|i| [i as f64 / 8192.0, 0.5, 0.25]).collect(),
            vec![ColorRgb::new(1, 2, 3); rows],
            vec![column],
        )
    };
    let (primary, _) = write_v8_split(
        other_block(nan_column(1234)),
        v8_metadata(rows as u64, vec![descriptor.clone()]),
    );
    let (_, same_shape) = write_v8_split(
        other_block(nan_column(99)),
        v8_metadata(rows as u64, vec![descriptor.clone()]),
    );
    let (_, other_shape) = fixture_pair();
    let mut reader = FormatReader::new(Cursor::new(primary.clone())).unwrap();
    assert!(reader.attach_attributes(Cursor::new(other_shape)).is_err());
    let mut reader = FormatReader::new(Cursor::new(primary)).unwrap();
    reader
        .attach_attributes(Cursor::new(same_shape))
        .expect("issue #115: a same-shape companion attaches today");
    assert!(matches!(
        reader.read_attribute(BLOCK_ID, 0),
        Err(FormatError::InvalidBlockChecksum(_))
    ));
}

#[test]
fn invalid_dictionary_is_rejected_even_with_valid_checksums() {
    let (block, meta, _) = codec_fixture();
    let (mut primary, mut attributes) = write_v8_split(block, meta);
    let reader = open_pair(primary.clone(), attributes.clone());
    let stats = reader.block_payload_storage_stats().unwrap();
    let schema_index = stats
        .iter()
        .find_map(|payload| match payload.kind {
            BlockPayloadKind::Attribute { schema_index, .. }
                if payload.codec == BlockCodec::Dictionary =>
            {
                Some(schema_index)
            }
            _ => None,
        })
        .expect("a dictionary column");
    let range = reader
        .plan_read(BLOCK_ID, ReadSelection::Attribute { schema_index })
        .unwrap()
        .ranges()[0];
    let (start, length) = (range.offset() as usize, range.stored_bytes() as usize);
    // A zero entry count is invalid; patch it and recompute the payload checksum.
    attributes[start..start + 4].copy_from_slice(&0_u32.to_le_bytes());
    let crc = viewer_format::crc32(&attributes[start..start + length]);
    // A payload directory entry is the codec byte, a u32 checksum and the u64 stored length.
    let footer = primary.len() - 40;
    let root_length = viewer_format::PagedReader::open(Cursor::new(primary.clone()))
        .unwrap()
        .root_page()
        .encoded_len();
    let root_start = footer - root_length;
    let mut entry = vec![BlockCodec::Dictionary as u8];
    entry.extend_from_slice(&range.checksum().to_le_bytes());
    entry.extend_from_slice(&range.stored_bytes().to_le_bytes());
    let hits: Vec<usize> = (root_start..footer - entry.len())
        .filter(|&at| primary[at..at + entry.len()] == entry[..])
        .collect();
    assert_eq!(hits.len(), 1, "directory entry not found uniquely");
    primary[hits[0] + 1..hits[0] + 5].copy_from_slice(&crc.to_le_bytes());
    let page_crc = viewer_format::crc32(&primary[root_start..footer - 4]);
    primary[footer - 4..footer].copy_from_slice(&page_crc.to_le_bytes());
    repair_index_and_footer(&mut primary);
    let mut reader = open_pair(primary, attributes);
    assert!(matches!(
        reader.read_attribute(BLOCK_ID, schema_index),
        Err(FormatError::MalformedBlock(_))
    ));
}

/// Writes `positions` as one single-node `v8` block (no attributes) whose root cell is 2^32 quanta
/// (131072 units) wide, so positions may span the whole block.
fn geometry_only_pair(positions: Vec<[f64; 3]>) -> (Vec<u8>, Vec<u8>) {
    let count = positions.len();
    let block = SourcePointBlock::new(
        V8_BLOCK,
        positions,
        vec![ColorRgb::new(1, 2, 3); count],
        Vec::new(),
    );
    let level = 32;
    let meta = with_v8_extras(global_metadata(
        level,
        [prefix_of(0.5, level); 3],
        vec![V8_BLOCK],
        count as u64,
        Vec::new(),
        v8_crs(),
        None,
    ));
    write_v8_split(block, meta)
}

fn open_geometry_only(positions: Vec<[f64; 3]>) -> FormatReader<Cursor<Vec<u8>>> {
    let (primary, attributes) = geometry_only_pair(positions);
    open_pair(primary, attributes)
}

fn lcg_positions(count: usize, mut state: u32, span: f64) -> Vec<[f64; 3]> {
    (0..count)
        .map(|_| {
            std::array::from_fn(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                f64::from(state) / f64::from(u32::MAX) * span
            })
        })
        .collect()
}

/// The `f32` positions a `v8` reader returns for `positions`: the global 2^-15 grid points.
fn grid_positions(positions: &[[f64; 3]]) -> Vec<[u32; 3]> {
    positions
        .iter()
        .map(|p| p.map(|c| (quanta_to_units(quantize_coordinate(c).unwrap()) as f32).to_bits()))
        .collect()
}

fn read_position_bits(reader: &mut FormatReader<Cursor<Vec<u8>>>) -> Vec<[u32; 3]> {
    reader
        .read_block(BLOCK_ID)
        .unwrap()
        .points()
        .points()
        .iter()
        .map(|p| p.position.map(f32::to_bits))
        .collect()
}

#[test]
fn v8_selects_delta_for_monotonic_data_and_raw_for_deterministic_random_data() {
    let count = 128usize;
    let monotonic: Vec<_> = (0..count)
        .map(|i| [i as f64 / count as f64, 0.5, 0.25])
        .collect();
    // Four points spread over the block: too few for delta or occupancy to beat 4-byte coordinates.
    let random = lcg_positions(4, 0x1234_5678, 4000.0);
    for (positions, expected_encoding) in [
        (monotonic, GeometryEncoding::DeltaVarint),
        (random, GeometryEncoding::RawU32),
    ] {
        let mut reader = open_geometry_only(positions.clone());
        assert_eq!(
            reader.block_storage_stats().unwrap()[0].geometry_encoding,
            expected_encoding
        );
        let mut expected = grid_positions(&positions);
        let mut actual = read_position_bits(&mut reader);
        expected.sort_unstable();
        actual.sort_unstable();
        assert_eq!(actual, expected, "{expected_encoding:?}");
    }
}

#[test]
fn v8_compact_occupancy_roundtrips_and_the_read_plan_counts_the_bitset() {
    for count in [128usize, 4096] {
        let positions = lcg_positions(count, 0x1234_5678, 4000.0);
        let mut reader = open_geometry_only(positions.clone());
        assert_eq!(
            reader.block_storage_stats().unwrap()[0].geometry_encoding,
            GeometryEncoding::CompactOccupied
        );
        let plan = reader
            .plan_read(BLOCK_ID, ReadSelection::Positions)
            .unwrap();
        let payload = reader.block_payload_storage_stats().unwrap()[0].clone();
        assert_eq!(plan.ranges().len(), 1);
        assert_eq!(plan.ranges()[0].decoded_bytes(), payload.raw_bytes);
        assert_eq!(
            plan.memory().output_bytes(),
            count as u64 * std::mem::size_of::<viewer_core::Point>() as u64
        );
        assert!(
            plan.memory().peak_bytes()
                >= payload.raw_bytes
                    + plan.memory().output_bytes()
                    + count as u64 * 12
                    + count.div_ceil(8) as u64
        );
        // A block that stays compact keeps its source row order.
        assert_eq!(read_position_bits(&mut reader), grid_positions(&positions));
    }
}

/// Replaces bytes of the root page of a `v8` primary file and repairs the page, index and footer
/// checksums. `edit` receives the root page without its trailing checksum.
fn patch_root_page(primary: &mut [u8], edit: impl FnOnce(&mut [u8])) {
    let footer = primary.len() - 40;
    let root_length = viewer_format::PagedReader::open(Cursor::new(primary.to_vec()))
        .unwrap()
        .root_page()
        .encoded_len();
    let root_start = footer - root_length;
    edit(&mut primary[root_start..footer - 4]);
    let page_crc = viewer_format::crc32(&primary[root_start..footer - 4]);
    primary[footer - 4..footer].copy_from_slice(&page_crc.to_le_bytes());
    repair_index_and_footer(primary);
}

#[test]
fn v8_refuses_a_plain_occupied_geometry_tag_even_with_repaired_checksums() {
    let count = 128usize;
    let positions = lcg_positions(count, 0x1234_5678, 4000.0);
    let (mut primary, attributes) = geometry_only_pair(positions);
    // A block entry is offset, attribute offset, point count (u64 each), then the geometry tag.
    let mut needle = (count as u64).to_le_bytes().to_vec();
    needle.push(GeometryEncoding::CompactOccupied as u8);
    patch_root_page(&mut primary, |page| {
        let hits: Vec<usize> = (0..page.len() - needle.len())
            .filter(|&at| page[at..at + needle.len()] == needle[..])
            .collect();
        assert_eq!(hits.len(), 1, "block entry not found uniquely");
        page[hits[0] + 8] = GeometryEncoding::Occupied as u8;
    });
    // The index accepts the tag (v8 shares the occupancy layout rules); the occupied decoder refuses
    // a payload written with the compact layout.
    let mut reader = open_pair(primary, attributes);
    assert!(matches!(
        reader.read_block(BLOCK_ID),
        Err(FormatError::MalformedBlock("invalid occupied geometry"))
    ));
}
