mod common;

use std::io::{Cursor, Read, Seek, SeekFrom};

use common::{
    BLOCK_ID, COLORS, POINTS, descriptors, fixture_attributes, fixture_block, metadata, options,
    repair_index_and_footer, write_monolithic, write_split,
};
use viewer_core::{
    AttributeColumn, AttributeDataType, AttributeDescriptor, AttributeValues, BlockId, ColorRgb,
    DatasetSource,
};
#[cfg(feature = "experimental")]
use viewer_core::{OccupiedOctreeRoot, OccupiedPointBlock, VoxelCoord};
#[cfg(feature = "experimental")]
use viewer_format::QuantizedOccupancy;
use viewer_format::{
    FileRole, FormatBlockEncoder, FormatError, FormatIdentity, FormatMetadata, FormatReader,
    FormatVersion, FormatWriter, GeometryEncoding, OctreeReader, ReadSelection, ReadSource,
    SelectedRead, SourcePointBlock, prepare_point_block, quantize_local_positions,
};

#[test]
fn rejects_invalid_delta_even_when_all_file_checksums_are_valid() {
    let mut writer = FormatWriter::new_for_version(
        FormatVersion::V6_PRE1,
        Cursor::new(Vec::new()),
        metadata(1, vec![]),
        options(),
    )
    .unwrap();
    writer
        .write_block(SourcePointBlock::new(
            BLOCK_ID,
            vec![[0.5; 3]],
            vec![ColorRgb::new(10, 20, 30)],
            vec![],
        ))
        .unwrap();
    let mut bytes = writer.finish().unwrap().into_inner();
    let reader = FormatReader::new(Cursor::new(bytes.clone())).unwrap();
    assert_eq!(
        reader.block_storage_stats().unwrap()[0].geometry_encoding,
        GeometryEncoding::DeltaVarint
    );
    assert_eq!(
        reader.block_payload_storage_stats().unwrap()[0].codec,
        viewer_format::BlockCodec::None
    );
    // A single block with no attributes has two 29-byte payload descriptors at the index tail.
    // Mutate its first signed delta to -1, then repair payload, index and footer CRCs.
    let footer = bytes.len() - 40;
    let index = u64::from_le_bytes(bytes[footer + 8..footer + 16].try_into().unwrap()) as usize;
    let index_length =
        u64::from_le_bytes(bytes[footer + 16..footer + 24].try_into().unwrap()) as usize;
    bytes[24] = 1;
    let payload_crc = viewer_format::crc32(&bytes[24..27]);
    let crc_offset = index + index_length - 2 * 29 + 25;
    bytes[crc_offset..crc_offset + 4].copy_from_slice(&payload_crc.to_le_bytes());
    let index_crc = viewer_format::crc32(&bytes[index..index + index_length]);
    bytes[footer + 32..footer + 36].copy_from_slice(&index_crc.to_le_bytes());
    let footer_crc = viewer_format::crc32(&bytes[footer..footer + 36]);
    bytes[footer + 36..].copy_from_slice(&footer_crc.to_le_bytes());
    let mut reader = FormatReader::new(Cursor::new(bytes)).unwrap();
    assert!(matches!(
        reader.read_positions(BLOCK_ID),
        Err(FormatError::MalformedBlock(_))
    ));
}

#[test]
fn small_encoded_geometry_cannot_bypass_the_logical_reader_limit() {
    let count = 1000;
    let mut writer = FormatWriter::new_for_version(
        FormatVersion::V6_PRE1,
        Cursor::new(Vec::new()),
        metadata(count, vec![]),
        options(),
    )
    .unwrap();
    writer
        .write_block(SourcePointBlock::new(
            BLOCK_ID,
            vec![[0.5; 3]; count as usize],
            vec![ColorRgb::new(10, 20, 30); count as usize],
            vec![],
        ))
        .unwrap();
    let bytes = writer.finish().unwrap().into_inner();
    let limits =
        viewer_format::FormatReaderLimits::new(1 << 20, 8000, 10, 10, 10, 10, 100).unwrap();
    assert!(matches!(
        FormatReader::with_limits(Cursor::new(bytes), limits),
        Err(FormatError::BlockExceedsLimit {
            requested: 15000,
            limit: 8000
        })
    ));
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

#[test]
fn v5_and_v6_pre1_preserve_geometry_colors_order_and_typed_attribute_bits() {
    let schema = descriptors();
    let meta = metadata(POINTS.len() as u64, schema.clone());
    let expected_attributes = fixture_attributes();

    for version in [
        FormatVersion::V5,
        FormatVersion::V6_PRE1,
        FormatVersion::V6_PRE2,
        FormatVersion::V6_PRE3,
        FormatVersion::V6_PRE4,
        FormatVersion::V7_PRE1,
    ] {
        let (primary, attributes) = write_split(version, fixture_block(), meta.clone());
        let mut reader = FormatReader::new(Cursor::new(primary)).unwrap();
        assert_eq!(reader.format_version(), version);
        reader.attach_attributes(Cursor::new(attributes)).unwrap();

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
}

#[test]
fn read_positions_without_companion_returns_neutral_gray() {
    let (primary, _) = write_split(
        FormatVersion::V6_PRE1,
        fixture_block(),
        metadata(POINTS.len() as u64, descriptors()),
    );
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

fn parallel_readers(bytes: Vec<u8>, count: usize) -> Vec<FormatReader<Cursor<Vec<u8>>>> {
    let base = FormatReader::new(Cursor::new(bytes.clone())).unwrap();
    let mut readers = Vec::with_capacity(count);
    readers.push(base);
    for _ in 1..count {
        let fork = readers[0].fork_reader(Cursor::new(bytes.clone()));
        readers.push(fork);
    }
    readers
}

#[test]
fn retained_budgeted_output_blocks_a_fork_until_drop_then_resumes() {
    let meta = metadata(POINTS.len() as u64, descriptors());
    let (primary, attributes) = write_split(FormatVersion::V7_PRE1, fixture_block(), meta);
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
    let meta = metadata(POINTS.len() as u64, descriptors());
    let (primary, attributes) = write_split(FormatVersion::V7_PRE1, fixture_block(), meta);
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
    let meta = metadata(POINTS.len() as u64, descriptors());
    let original = write_monolithic(FormatVersion::V7_PRE1, fixture_block(), meta);
    let selection = ReadSelection::Positions;
    let probe = FormatReader::new(Cursor::new(original.clone())).unwrap();
    let plan = probe.plan_read(BLOCK_ID, selection).unwrap();
    let range = plan.ranges()[0];
    let mut corrupt = original;
    corrupt[range.offset() as usize] ^= 1;
    let source = CountingReader::new(corrupt);
    let mut reader = FormatReader::new(source).unwrap();
    let budget = viewer_format::ReadBudget::new(plan.memory().peak_bytes());
    assert!(matches!(
        reader.try_read_selected(BLOCK_ID, selection, &budget),
        Err(FormatError::InvalidBlockChecksum(_))
    ));
    assert_eq!(budget.reserved_bytes(), 0);

    let bytes = write_monolithic(
        FormatVersion::V7_PRE1,
        fixture_block(),
        metadata(POINTS.len() as u64, descriptors()),
    );
    let fail = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut reader = FormatReader::new(SwitchableReader {
        cursor: Cursor::new(bytes),
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

    let bytes = write_monolithic(
        FormatVersion::V7_PRE1,
        fixture_block(),
        metadata(POINTS.len() as u64, descriptors()),
    );
    let requests = [
        (BLOCK_ID, ReadSelection::DisplayColors),
        (BLOCK_ID, ReadSelection::Positions),
        (BLOCK_ID, ReadSelection::Attribute { schema_index: 6 }),
        (BLOCK_ID, ReadSelection::Geometry),
    ];
    for reader_count in [1, 2] {
        let mut readers = parallel_readers(bytes.clone(), reader_count);
        let budget = viewer_format::ReadBudget::new(u64::MAX);
        let mut delivered = Vec::new();
        let progress = read_selected_parallel(&mut readers, &requests, &budget, |index, result| {
            delivered.push(index);
            match (index, result) {
                (0, SelectedRead::DisplayColors(colors)) => {
                    assert_eq!(colors.as_slice(), COLORS.as_slice())
                }
                (1, SelectedRead::Positions(points)) => {
                    assert_eq!(points.len(), POINTS.len())
                }
                (2, SelectedRead::Attribute(column)) => {
                    assert_values_bits_eq(column.values(), fixture_attributes()[6].values())
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

    let bytes = write_monolithic(
        FormatVersion::V7_PRE1,
        fixture_block(),
        metadata(POINTS.len() as u64, descriptors()),
    );
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

    let bytes = write_monolithic(
        FormatVersion::V7_PRE1,
        fixture_block(),
        metadata(POINTS.len() as u64, descriptors()),
    );
    let base = FormatReader::new(Cursor::new(bytes.clone())).unwrap();
    let peak = base
        .plan_read(BLOCK_ID, ReadSelection::Positions)
        .unwrap()
        .memory()
        .peak_bytes();
    let requests = [(BLOCK_ID, ReadSelection::Positions); 2];

    let mut readers = parallel_readers(bytes.clone(), 2);
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

    let mut readers = parallel_readers(bytes.clone(), 2);
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

    let mut readers = parallel_readers(bytes, 2);
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

    let bytes = write_monolithic(
        FormatVersion::V7_PRE1,
        fixture_block(),
        metadata(POINTS.len() as u64, descriptors()),
    );
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

    let meta = metadata(POINTS.len() as u64, descriptors());
    let original = write_monolithic(FormatVersion::V7_PRE1, fixture_block(), meta);
    let probe = FormatReader::new(Cursor::new(original.clone())).unwrap();
    let range = probe
        .plan_read(BLOCK_ID, ReadSelection::Positions)
        .unwrap()
        .ranges()[0];
    let mut corrupt = original.clone();
    corrupt[range.offset() as usize] ^= 1;
    let mut readers = parallel_readers(corrupt, 2);
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
    let mut bad_colors = original;
    bad_colors[color_range.offset() as usize] ^= 1;
    let mut readers = parallel_readers(bad_colors, 2);
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

    let bytes = write_monolithic(
        FormatVersion::V7_PRE1,
        fixture_block(),
        metadata(POINTS.len() as u64, descriptors()),
    );
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

    let mut readers = parallel_readers(bytes, 2);
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
    let meta = metadata(POINTS.len() as u64, descriptors());
    let selections = [
        (ReadSelection::Positions, vec![0]),
        (ReadSelection::Geometry, vec![0, 1]),
        (ReadSelection::DisplayColors, vec![1]),
        (ReadSelection::Attribute { schema_index: 6 }, vec![8]),
    ];
    for version in [
        FormatVersion::V5,
        FormatVersion::V6_PRE4,
        FormatVersion::V7_PRE1,
    ] {
        for split in [false, true] {
            let (primary_bytes, attribute_bytes) = if split {
                let (primary, attributes) = write_split(version, fixture_block(), meta.clone());
                (primary, Some(attributes))
            } else {
                (
                    write_monolithic(version, fixture_block(), meta.clone()),
                    None,
                )
            };
            let primary = CountingReader::new(primary_bytes.clone());
            let attributes = attribute_bytes.clone().map(CountingReader::new);
            let primary_counter = primary.bytes_read.clone();
            let attribute_counter = attributes.as_ref().map(|reader| reader.bytes_read.clone());
            let mut reader = FormatReader::new(primary).unwrap();
            if let Some(attributes) = attributes {
                reader.attach_attributes(attributes).unwrap();
            }
            let payload_stats = reader.block_payload_storage_stats().unwrap();
            for (selection, payload_indices) in &selections {
                primary_counter.set(0);
                if let Some(counter) = &attribute_counter {
                    counter.set(0);
                }
                let plan = reader.plan_read(BLOCK_ID, *selection).unwrap();
                assert_eq!(plan.block_id(), BLOCK_ID);
                assert_eq!(plan.selection(), *selection);
                assert_eq!(plan.ranges().len(), payload_indices.len());
                assert_eq!(primary_counter.get(), 0, "plan performed primary I/O");
                assert_eq!(
                    attribute_counter
                        .as_ref()
                        .map_or(0, |counter| counter.get()),
                    0,
                    "plan performed companion I/O"
                );

                let selected_stats = payload_indices
                    .iter()
                    .map(|index| &payload_stats[*index])
                    .collect::<Vec<_>>();
                let mut expected_primary_bytes = 0_u64;
                let mut expected_attribute_bytes = 0_u64;
                for (range, (&payload_index, stats)) in plan
                    .ranges()
                    .iter()
                    .zip(payload_indices.iter().zip(selected_stats))
                {
                    let is_attribute = split && payload_index > 0;
                    let source = if is_attribute {
                        ReadSource::Attributes
                    } else {
                        ReadSource::Primary
                    };
                    let preceding_bytes = if split {
                        let start = if payload_index == 0 { 0 } else { 1 };
                        payload_stats[start..payload_index]
                            .iter()
                            .map(|payload| payload.stored_bytes)
                            .sum::<u64>()
                    } else {
                        payload_stats[..payload_index]
                            .iter()
                            .map(|payload| payload.stored_bytes)
                            .sum::<u64>()
                    };
                    let expected_offset = 24 + preceding_bytes;
                    assert_eq!(range.source(), source);
                    assert_eq!(range.offset(), expected_offset);
                    assert_eq!(range.stored_bytes(), stats.stored_bytes);
                    assert_eq!(range.decoded_bytes(), stats.raw_bytes);
                    assert_eq!(range.codec(), stats.codec);
                    let source_bytes = if is_attribute {
                        attribute_bytes.as_ref().unwrap()
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
                    ReadSelection::DisplayColors => {
                        (POINTS.len() * std::mem::size_of::<ColorRgb>()) as u64
                    }
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
                    SelectedRead::DisplayColors(colors)
                        if *selection == ReadSelection::DisplayColors =>
                    {
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
                assert_eq!(
                    attribute_counter
                        .as_ref()
                        .map_or(0, |counter| counter.get()) as u64,
                    expected_attribute_bytes
                );
                assert!(plan.ranges().len() <= 2);
            }
        }
    }
}

#[test]
fn selected_read_budget_is_checked_before_any_payload_io() {
    let meta = metadata(POINTS.len() as u64, descriptors());
    let (primary, attributes) = write_split(FormatVersion::V7_PRE1, fixture_block(), meta);
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
    let meta = metadata(POINTS.len() as u64, descriptors());
    let (primary, _) = write_split(FormatVersion::V7_PRE1, fixture_block(), meta);
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
    let schema = descriptors();
    let meta = metadata(POINTS.len() as u64, schema);
    for version in [
        FormatVersion::V5,
        FormatVersion::V6_PRE1,
        FormatVersion::V6_PRE2,
        FormatVersion::V6_PRE3,
        FormatVersion::V6_PRE4,
        FormatVersion::V7_PRE1,
    ] {
        let (primary, attributes) = write_split(version, fixture_block(), meta.clone());
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
}

#[test]
fn split_companions_are_version_bound_and_forks_keep_the_detected_version() {
    let meta = metadata(POINTS.len() as u64, descriptors());
    let (v5_primary, v5_attributes) = write_split(FormatVersion::V5, fixture_block(), meta.clone());
    let (v6_primary, v6_attributes) = write_split(FormatVersion::V6_PRE1, fixture_block(), meta);

    let mut v5_reader = FormatReader::new(Cursor::new(v5_primary.clone())).unwrap();
    assert!(
        v5_reader
            .attach_attributes(Cursor::new(v6_attributes))
            .is_err()
    );
    let mut v6_reader = FormatReader::new(Cursor::new(v6_primary.clone())).unwrap();
    assert!(
        v6_reader
            .attach_attributes(Cursor::new(v5_attributes))
            .is_err()
    );

    let v6_reader = FormatReader::new(Cursor::new(v6_primary.clone())).unwrap();
    let mut fork = v6_reader.fork_reader(Cursor::new(v6_primary));
    assert_eq!(fork.format_version(), FormatVersion::V6_PRE1);
    fork.attach_attributes(Cursor::new(
        write_split(
            FormatVersion::V6_PRE1,
            fixture_block(),
            metadata(POINTS.len() as u64, descriptors()),
        )
        .1,
    ))
    .unwrap();
    assert_eq!(fork.read_geometry(BLOCK_ID).unwrap().len(), POINTS.len());
}

#[test]
fn v7_pre1_is_a_distinct_identity_with_pre4_payload_behavior() {
    use viewer_format::BlockPayloadKind;

    let meta = metadata(POINTS.len() as u64, descriptors());
    let (v6_primary, v6_attributes) =
        write_split(FormatVersion::V6_PRE4, fixture_block(), meta.clone());
    let (v7_primary, v7_attributes) =
        write_split(FormatVersion::V7_PRE1, fixture_block(), meta.clone());
    assert_eq!(&v7_primary[..8], b"OCTVFRM1");
    let v7_footer = v7_primary.len() - 40;
    let v7_index = u64::from_le_bytes(
        v7_primary[v7_footer + 8..v7_footer + 16]
            .try_into()
            .unwrap(),
    ) as usize;
    assert_eq!(&v7_primary[v7_index..v7_index + 8], b"OCTVI71P");
    assert_eq!(
        &v7_primary[v7_primary.len() - 40..v7_primary.len() - 32],
        b"OCTVE71P"
    );
    assert_eq!(&v7_attributes[..8], b"OCTVFRM1");
    assert_eq!(
        &v7_attributes[v7_attributes.len() - 40..v7_attributes.len() - 32],
        b"OCTVE71P"
    );

    let v6_footer = v6_primary.len() - 40;
    let v6_index = u64::from_le_bytes(
        v6_primary[v6_footer + 8..v6_footer + 16]
            .try_into()
            .unwrap(),
    ) as usize;
    assert_eq!(&v6_primary[24..v6_index], &v7_primary[24..v7_index]);
    assert_eq!(
        &v6_attributes[24..v6_attributes.len() - 40],
        &v7_attributes[24..v7_attributes.len() - 40]
    );

    let v6 = FormatReader::new(Cursor::new(v6_primary)).unwrap();
    let mut v7 = FormatReader::new(Cursor::new(v7_primary.clone())).unwrap();
    assert_eq!(v7.format_version(), FormatVersion::V7_PRE1);
    assert_eq!(
        v7.block_storage_stats().unwrap(),
        v6.block_storage_stats().unwrap()
    );
    assert_eq!(
        v7.block_payload_storage_stats().unwrap(),
        v6.block_payload_storage_stats().unwrap()
    );
    assert!(v7.attach_attributes(Cursor::new(v6_attributes)).is_err());
    v7.attach_attributes(Cursor::new(v7_attributes.clone()))
        .unwrap();
    assert_eq!(v7.read_positions(BLOCK_ID).unwrap().len(), POINTS.len());
    assert_eq!(v7.read_display_colors(BLOCK_ID).unwrap(), COLORS);
    for index in 0..descriptors().len() {
        assert_values_bits_eq(
            v7.read_attribute(BLOCK_ID, index as u32).unwrap().values(),
            fixture_attributes()[index].values(),
        );
    }

    let base = FormatReader::new(Cursor::new(v7_primary.clone())).unwrap();
    let fork = base.fork_reader(Cursor::new(v7_primary));
    assert_eq!(fork.format_version(), FormatVersion::V7_PRE1);

    let encoder = FormatBlockEncoder::new(FormatVersion::V6_PRE4, &meta, options()).unwrap();
    let prepared = viewer_format::BlockEncoder::prepare(&encoder, fixture_block()).unwrap();
    let mut writer = FormatWriter::new_for_version(
        FormatVersion::V7_PRE1,
        Cursor::new(Vec::new()),
        meta.clone(),
        options(),
    )
    .unwrap();
    assert!(matches!(
        writer.write_prepared_block(prepared),
        Err(FormatError::PreparedVersionMismatch {
            writer: FormatVersion::V7Pre(_),
            block: FormatVersion::V6Pre(_),
        })
    ));
    writer.write_block(fixture_block()).unwrap();

    // V7 retains the stored-payload CRC check from pre4.
    let mut corrupted = write_split(FormatVersion::V7_PRE1, fixture_block(), meta).0;
    corrupted[24] ^= 1;
    let mut reader = FormatReader::new(Cursor::new(corrupted)).unwrap();
    assert!(matches!(
        reader.read_positions(BLOCK_ID),
        Err(FormatError::InvalidBlockChecksum(_))
    ));
    assert!(
        v7.block_payload_storage_stats()
            .unwrap()
            .iter()
            .any(|payload| matches!(payload.kind, BlockPayloadKind::Geometry))
    );
}

#[test]
fn v6_selects_delta_for_monotonic_data_and_raw_for_deterministic_random_data() {
    let count = 128usize;
    let schema = Vec::new();
    let mut writer = FormatWriter::new_for_version(
        FormatVersion::V6_PRE1,
        Cursor::new(Vec::new()),
        metadata(count as u64, schema.clone()),
        options(),
    )
    .unwrap();
    let monotonic: Vec<_> = (0..count)
        .map(|i| [i as f64 / count as f64, 0.5, 0.25])
        .collect();
    writer
        .write_block(SourcePointBlock::new(
            BLOCK_ID,
            monotonic,
            vec![ColorRgb::new(1, 2, 3); count],
            Vec::new(),
        ))
        .unwrap();
    let bytes = writer.finish().unwrap().into_inner();
    let reader = FormatReader::new(Cursor::new(bytes)).unwrap();
    assert_eq!(
        reader.block_storage_stats().unwrap()[0].geometry_encoding,
        GeometryEncoding::DeltaVarint
    );

    let count = 128usize;
    let mut state = 0x1234_5678u32;
    let random: Vec<_> = (0..count)
        .map(|_| {
            std::array::from_fn(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                f64::from(state) / f64::from(u32::MAX)
            })
        })
        .collect();
    let mut writer = FormatWriter::new_for_version(
        FormatVersion::V6_PRE1,
        Cursor::new(Vec::new()),
        metadata(count as u64, schema),
        options(),
    )
    .unwrap();
    writer
        .write_block(SourcePointBlock::new(
            BLOCK_ID,
            random,
            vec![ColorRgb::new(1, 2, 3); count],
            Vec::new(),
        ))
        .unwrap();
    let bytes = writer.finish().unwrap().into_inner();
    let reader = FormatReader::new(Cursor::new(bytes)).unwrap();
    assert_eq!(
        reader.block_storage_stats().unwrap()[0].geometry_encoding,
        GeometryEncoding::RawU32
    );
}

#[test]
fn prepared_v5_block_is_rejected_by_v6_writer_and_writer_remains_usable() {
    let schema = descriptors();
    let meta = metadata(POINTS.len() as u64, schema);
    let prepared = prepare_point_block(fixture_block(), &meta, options()).unwrap();
    let mut writer = FormatWriter::new_for_version(
        FormatVersion::V6_PRE1,
        Cursor::new(Vec::new()),
        meta.clone(),
        options(),
    )
    .unwrap();
    assert!(matches!(
        writer.write_prepared_block(prepared),
        Err(FormatError::PreparedVersionMismatch {
            writer: FormatVersion::V6Pre(_),
            block: FormatVersion::V5,
        })
    ));
    writer.write_block(fixture_block()).unwrap();
    let bytes = writer.finish().unwrap().into_inner();
    assert_eq!(
        FormatReader::new(Cursor::new(bytes))
            .unwrap()
            .block_storage_stats()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn unsupported_v6_final_pre5_and_v7_pre6_are_rejected_by_writer_encoder_and_reader() {
    let meta = metadata(POINTS.len() as u64, descriptors());
    let options = options();
    let versions = [
        FormatVersion::V6,
        FormatVersion::V6Pre(std::num::NonZeroU32::new(5).unwrap()),
        FormatVersion::V7Pre(std::num::NonZeroU32::new(6).unwrap()),
    ];
    for version in versions {
        assert!(matches!(
            FormatWriter::new_for_version(version, Cursor::new(Vec::new()), meta.clone(), options),
            Err(FormatError::UnsupportedFormatVersion(actual)) if actual == version
        ));
        assert!(matches!(
            FormatBlockEncoder::new(version, &meta, options),
            Err(FormatError::UnsupportedFormatVersion(actual)) if actual == version
        ));
        let header = FormatIdentity {
            version,
            role: FileRole::Monolithic,
        }
        .to_header_bytes()
        .unwrap();
        assert!(matches!(
            FormatReader::new(Cursor::new(header)),
            Err(FormatError::UnsupportedFormatVersion(actual)) if actual == version
        ));
    }
}

#[cfg(feature = "experimental")]
#[test]
fn occupied_cells_keep_rows_and_attribute_bits_from_v5_and_v6_readers() {
    let a = VoxelCoord::new(-1, 0, 0);
    let b = VoxelCoord::new(0, -1, 0);
    let cells = [a, b, a, b];
    for version in [
        FormatVersion::V5,
        FormatVersion::V6_PRE1,
        FormatVersion::V6_PRE2,
        FormatVersion::V6_PRE3,
        FormatVersion::V6_PRE4,
        FormatVersion::V7_PRE1,
    ] {
        for attach in [false, true] {
            let (primary, attributes) = write_split(
                version,
                fixture_block(),
                metadata(POINTS.len() as u64, descriptors()),
            );
            let mut reader = FormatReader::new(Cursor::new(primary)).unwrap();
            if attach {
                reader.attach_attributes(Cursor::new(attributes)).unwrap();
            }
            let source = reader.read_block(BLOCK_ID).unwrap();
            let occupied = OccupiedPointBlock::new(
                OccupiedOctreeRoot::new([-2; 3], 2).unwrap(),
                source,
                &cells,
            )
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
            assert_eq!(reader.format_version(), version);
        }
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
fn pre2_physically_selects_occupied_arrays_and_preserves_every_payload_row() {
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
    let expected = quantize_local_positions(&positions, 0.001)
        .unwrap()
        .decode_local()
        .unwrap();
    for (version, split) in [
        FormatVersion::V6_PRE2,
        FormatVersion::V6_PRE4,
        FormatVersion::V7_PRE1,
    ]
    .into_iter()
    .flat_map(|v| [false, true].map(|s| (v, s)))
    {
        let meta = metadata(count as u64, vec![descriptor.clone()]);
        let block = SourcePointBlock::new(
            BLOCK_ID,
            positions.clone(),
            colors.clone(),
            vec![column.clone()],
        );
        let mut reader = if split {
            let (primary, attrs) = write_split(version, block, meta);
            let mut reader = FormatReader::new(Cursor::new(primary)).unwrap();
            reader.attach_attributes(Cursor::new(attrs)).unwrap();
            reader
        } else {
            let mut writer =
                FormatWriter::new_for_version(version, Cursor::new(Vec::new()), meta, options())
                    .unwrap();
            writer.write_block(block).unwrap();
            FormatReader::new(writer.finish().unwrap()).unwrap()
        };
        assert_eq!(
            reader.block_storage_stats().unwrap()[0].geometry_encoding,
            if version != FormatVersion::V6_PRE2 {
                GeometryEncoding::CompactOccupied
            } else {
                GeometryEncoding::Occupied
            }
        );
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
                >= geometry_payload.raw_bytes
                    + geometry_plan.memory().output_bytes()
                    + count as u64 * 12
                    + count.div_ceil(8) as u64
        );
        let block = reader.read_block(BLOCK_ID).unwrap();
        assert_eq!(
            block
                .points()
                .points()
                .iter()
                .map(|p| p.position.map(f32::to_bits))
                .collect::<Vec<_>>(),
            expected
                .iter()
                .map(|p| p.map(f32::to_bits))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            block
                .points()
                .points()
                .iter()
                .map(|p| p.color)
                .collect::<Vec<_>>(),
            colors
        );
        assert_eq!(block.attributes()[0].values(), &values);
    }
}

#[test]
fn pre1_rejects_occupied_tag_even_with_repaired_checksums() {
    let block = SourcePointBlock::new(BLOCK_ID, POINTS.to_vec(), COLORS.to_vec(), vec![]);
    let (mut primary, _) = write_split(FormatVersion::V6_PRE1, block, metadata(4, vec![]));
    let footer = primary.len() - 40;
    let offset = u64::from_le_bytes(primary[footer + 8..footer + 16].try_into().unwrap()) as usize;
    let length = u64::from_le_bytes(primary[footer + 16..footer + 24].try_into().unwrap()) as usize;
    // One 94-byte directory prefix followed by two 29-byte payload entries.
    primary[offset + length - 94 - 58 + 32] = 2;
    let crc = viewer_format::crc32(&primary[offset..offset + length]);
    primary[footer + 32..footer + 36].copy_from_slice(&crc.to_le_bytes());
    let crc = viewer_format::crc32(&primary[footer..footer + 36]);
    primary[footer + 36..].copy_from_slice(&crc.to_le_bytes());
    assert!(matches!(
        FormatReader::new(Cursor::new(primary)),
        Err(FormatError::MalformedIndex(_))
    ));
}

#[test]
fn pre2_sidecars_prepared_blocks_and_forks_keep_exact_revision_identity() {
    let meta = metadata(POINTS.len() as u64, descriptors());
    let (pre1, attrs1) = write_split(FormatVersion::V6_PRE1, fixture_block(), meta.clone());
    let (pre2, attrs2) = write_split(FormatVersion::V6_PRE2, fixture_block(), meta.clone());
    let mut reader1 = FormatReader::new(Cursor::new(pre1)).unwrap();
    assert!(
        reader1
            .attach_attributes(Cursor::new(attrs2.clone()))
            .is_err()
    );
    let mut reader2 = FormatReader::new(Cursor::new(pre2.clone())).unwrap();
    assert!(reader2.attach_attributes(Cursor::new(attrs1)).is_err());
    let mut fork = reader2.fork_reader(Cursor::new(pre2));
    assert_eq!(fork.format_version(), FormatVersion::V6_PRE2);
    fork.attach_attributes(Cursor::new(attrs2)).unwrap();
    assert_eq!(
        fork.read_block(BLOCK_ID).unwrap().points().len(),
        POINTS.len()
    );
    let encoder = FormatBlockEncoder::new(FormatVersion::V6_PRE1, &meta, options()).unwrap();
    let prepared = viewer_format::BlockEncoder::prepare(&encoder, fixture_block()).unwrap();
    let mut writer = FormatWriter::new_for_version(
        FormatVersion::V6_PRE2,
        Cursor::new(Vec::new()),
        meta,
        options(),
    )
    .unwrap();
    assert!(matches!(
        writer.write_prepared_block(prepared),
        Err(FormatError::PreparedVersionMismatch { .. })
    ));
    writer.write_block(fixture_block()).unwrap();
    let mut reader = FormatReader::new(writer.finish().unwrap()).unwrap();
    assert_eq!(
        reader.read_block(BLOCK_ID).unwrap().points().len(),
        POINTS.len()
    );
}

fn pre3_fixture() -> (SourcePointBlock, FormatMetadata) {
    let rows = 4096;
    let mut state = 0x91ab032f_u32;
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
                    .flat_map(|_| [f64::from_bits(0x7ff8000000000042), -0.0])
                    .collect(),
            ),
            rows,
        )
        .unwrap(),
    );
    let schema = columns.iter().map(|c| c.descriptor().clone()).collect();
    (
        SourcePointBlock::new(
            BLOCK_ID,
            indices.iter().map(|i| POINTS[*i]).collect(),
            indices.iter().map(|i| COLORS[*i]).collect(),
            columns,
        ),
        metadata(rows as u64, schema),
    )
}

#[test]
fn pre3_selects_attribute_codecs_preserves_typed_bits_and_reads_only_requested_column() {
    use viewer_format::{BlockCodec, BlockPayloadKind};
    let (block, meta) = pre3_fixture();
    let expected = block.attributes().to_vec();
    let (pre2, attrs2) = write_split(FormatVersion::V6_PRE2, block, meta.clone());
    let (block, _) = pre3_fixture();
    let (pre3, attrs3) = write_split(FormatVersion::V6_PRE3, block, meta);
    assert!(pre3.len() + attrs3.len() < pre2.len() + attrs2.len());
    let primary = CountingReader::new(pre3);
    let attributes = CountingReader::new(attrs3);
    let p_count = primary.bytes_read.clone();
    let a_count = attributes.bytes_read.clone();
    let mut reader = FormatReader::new(primary).unwrap();
    reader.attach_attributes(attributes).unwrap();
    let stats = reader.block_payload_storage_stats().unwrap();
    assert!(stats.iter().any(|p| p.codec == BlockCodec::Dictionary));
    assert_eq!(stats.last().unwrap().codec, BlockCodec::Constant);
    assert_eq!(stats[1].codec, BlockCodec::Dictionary); // RGB is a three-component tuple.
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
        expected[dictionary_index as usize].values(),
    );
    for (index, column) in expected.iter().enumerate() {
        p_count.set(0);
        a_count.set(0);
        let actual = reader.read_attribute(BLOCK_ID, index as u32).unwrap();
        assert_values_bits_eq(actual.values(), column.values());
        let payload = stats.iter().find(|p| matches!(p.kind, BlockPayloadKind::Attribute { schema_index, .. } if schema_index == index as u32)).unwrap();
        assert_eq!(p_count.get(), 0);
        assert_eq!(a_count.get() as u64, payload.stored_bytes);
    }
    assert_eq!(reader.read_display_colors(BLOCK_ID).unwrap().len(), 4096);
    let mut wrong = FormatReader::new(Cursor::new(pre2)).unwrap();
    // A pre2 primary cannot accept a pre3 sidecar even when its schema matches.
    let (block, meta) = pre3_fixture();
    let (_, attrs3) = write_split(FormatVersion::V6_PRE3, block, meta);
    assert!(wrong.attach_attributes(Cursor::new(attrs3)).is_err());
}

#[test]
fn pre3_rejects_invalid_dictionary_even_with_valid_crcs_and_old_revision_relabel() {
    let (block, meta) = pre3_fixture();
    let mut writer = FormatWriter::new_for_version(
        FormatVersion::V6_PRE3,
        Cursor::new(Vec::new()),
        meta,
        options(),
    )
    .unwrap();
    writer.write_block(block).unwrap();
    let original = writer.finish().unwrap().into_inner();
    let reader = FormatReader::new(Cursor::new(original.clone())).unwrap();
    let stats = reader.block_payload_storage_stats().unwrap();
    let footer = original.len() - 40;
    let index = u64::from_le_bytes(original[footer + 8..footer + 16].try_into().unwrap()) as usize;
    let length =
        u64::from_le_bytes(original[footer + 16..footer + 24].try_into().unwrap()) as usize;
    let mut corrupt = original.clone();
    let start = 24 + stats[0].stored_bytes as usize; // RGB dictionary is the second payload.
    corrupt[start..start + 4].copy_from_slice(&0_u32.to_le_bytes());
    let crc = viewer_format::crc32(&corrupt[start..start + stats[1].stored_bytes as usize]);
    let descriptor = index + length - stats.len() * 29 + 29;
    corrupt[descriptor + 25..descriptor + 29].copy_from_slice(&crc.to_le_bytes());
    repair_index_and_footer(&mut corrupt);
    let mut reader = FormatReader::new(Cursor::new(corrupt)).unwrap();
    assert!(matches!(
        reader.read_display_colors(BLOCK_ID),
        Err(FormatError::MalformedBlock(_))
    ));

    let mut old = original;
    old[..24].copy_from_slice(
        &FormatIdentity {
            version: FormatVersion::V6_PRE2,
            role: FileRole::Monolithic,
        }
        .to_header_bytes()
        .unwrap(),
    );
    old[index..index + 8].copy_from_slice(b"OCTVI62P");
    old[footer..footer + 8].copy_from_slice(b"OCTVE62P");
    repair_index_and_footer(&mut old);
    assert!(matches!(
        FormatReader::new(Cursor::new(old)),
        Err(FormatError::MalformedIndex(_))
    ));
}

#[test]
fn pre3_checks_attribute_checksum_before_expansion() {
    let (block, meta) = pre3_fixture();
    let (primary, mut attributes) = write_split(FormatVersion::V6_PRE3, block, meta);
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
fn pre4_byte_planes_are_selective_exact_and_rejected_by_older_revisions() {
    for new_version in [FormatVersion::V6_PRE4, FormatVersion::V7_PRE1] {
        use viewer_format::{BlockCodec, BlockPayloadKind};
        let rows = 4096;
        let mut state = 1234_u32;
        let values = AttributeValues::F64(
            (0..rows)
                .map(|_| {
                    state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                    f64::from_bits(0x7ff8000000000000 | u64::from(state))
                })
                .collect(),
        );
        let descriptor =
            AttributeDescriptor::new("nan_payload", AttributeDataType::F64, 1).unwrap();
        let column = AttributeColumn::new(descriptor.clone(), values.clone(), rows).unwrap();
        let make = || {
            SourcePointBlock::new(
                BLOCK_ID,
                (0..rows).map(|i| [i as f64, 0., 0.]).collect(),
                vec![ColorRgb::new(1, 2, 3); rows],
                vec![column.clone()],
            )
        };
        let meta = metadata(rows as u64, vec![descriptor]);
        let (old, old_attrs) = write_split(FormatVersion::V6_PRE3, make(), meta.clone());
        let (primary, attrs) = write_split(new_version, make(), meta.clone());
        assert!(primary.len() + attrs.len() < old.len() + old_attrs.len());
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
        pc.set(0);
        ac.set(0);
        assert_values_bits_eq(
            reader.read_attribute(BLOCK_ID, 0).unwrap().values(),
            &values,
        );
        assert_eq!(pc.get(), 0);
        assert_eq!(ac.get() as u64, payload.stored_bytes);
        let mut wrong = FormatReader::new(Cursor::new(old)).unwrap();
        let (_, attrs4) = write_split(new_version, make(), meta.clone());
        assert!(wrong.attach_attributes(Cursor::new(attrs4)).is_err());
        let mut writer =
            FormatWriter::new_for_version(new_version, Cursor::new(Vec::new()), meta, options())
                .unwrap();
        writer.write_block(make()).unwrap();
        let original = writer.finish().unwrap().into_inner();
        let footer = original.len() - 40;
        let index =
            u64::from_le_bytes(original[footer + 8..footer + 16].try_into().unwrap()) as usize;
        for version in [
            FormatVersion::V6_PRE1,
            FormatVersion::V6_PRE2,
            FormatVersion::V6_PRE3,
        ] {
            let mut bad = original.clone();
            bad[..24].copy_from_slice(
                &FormatIdentity {
                    version,
                    role: FileRole::Monolithic,
                }
                .to_header_bytes()
                .unwrap(),
            );
            let revision = version.revision();
            bad[index..index + 8].copy_from_slice(format!("OCTVI6{revision}P").as_bytes());
            bad[footer..footer + 8].copy_from_slice(format!("OCTVE6{revision}P").as_bytes());
            repair_index_and_footer(&mut bad);
            assert!(matches!(
                FormatReader::new(Cursor::new(bad)),
                Err(FormatError::MalformedIndex(_))
            ));
        }
    }
}
#[test]
fn pre3_rejects_compact_tag_even_with_repaired_index_crc() {
    let block = SourcePointBlock::new(BLOCK_ID, POINTS.to_vec(), COLORS.to_vec(), vec![]);
    let (mut primary, _) = write_split(FormatVersion::V6_PRE3, block, metadata(4, vec![]));
    let footer = primary.len() - 40;
    let index = u64::from_le_bytes(primary[footer + 8..footer + 16].try_into().unwrap()) as usize;
    let length = u64::from_le_bytes(primary[footer + 16..footer + 24].try_into().unwrap()) as usize;
    primary[index + length - 94 - 58 + 32] = GeometryEncoding::CompactOccupied as u8;
    repair_index_and_footer(&mut primary);
    assert!(matches!(
        FormatReader::new(Cursor::new(primary)),
        Err(FormatError::MalformedIndex(_))
    ));
}

#[test]
fn v7_dictionary_and_constant_columns_preserve_typed_bits() {
    use viewer_format::BlockCodec;
    let (block, meta) = pre3_fixture();
    let expected = block.attributes().to_vec();
    let (primary, attributes) = write_split(FormatVersion::V7_PRE1, block, meta);
    let mut reader = FormatReader::new(Cursor::new(primary)).unwrap();
    reader.attach_attributes(Cursor::new(attributes)).unwrap();
    let stats = reader.block_payload_storage_stats().unwrap();
    assert!(
        stats
            .iter()
            .any(|payload| payload.codec == BlockCodec::Dictionary)
    );
    assert_eq!(stats.last().unwrap().codec, BlockCodec::Constant);
    for (index, column) in expected.iter().enumerate() {
        assert_values_bits_eq(
            reader
                .read_attribute(BLOCK_ID, index as u32)
                .unwrap()
                .values(),
            column.values(),
        );
    }
}
