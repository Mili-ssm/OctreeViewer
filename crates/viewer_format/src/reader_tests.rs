//! Reader tests on V8 fixtures (`test_support`): a single-node dataset holding block 0.

use std::{
    cell::RefCell,
    io::{Cursor, Read, Seek, SeekFrom},
    rc::Rc,
};

use viewer_core::{
    AttributeColumn, AttributeDataType, AttributeDescriptor, AttributeValues, BlockId,
    BlockRequestScheduler, ColorRgb, FrustumPlane, LodSelectionConfig, RequestCompletion,
    SpatialFrustum,
};

use super::{FormatError, IndexedReader as FormatReader};
use crate::test_support::{
    COLORS, V8_BLOCK, intensity, v8_block, v8_metadata, v8_metadata_at, v8_pair, write_v8_split,
};
use crate::{FormatReaderLimits, SourcePointBlock, probe_format};

const ID: BlockId = V8_BLOCK;
const GRAY: ColorRgb = ColorRgb::new(160, 160, 160);

/// Opens `reader` the way the public facade does, but keeps the backend so tests see its internals.
fn open<R: Read + Seek>(reader: R) -> Result<FormatReader<R>, FormatError> {
    open_with(reader, FormatReaderLimits::default())
}

fn open_with<R: Read + Seek>(
    mut reader: R,
    limits: FormatReaderLimits,
) -> Result<FormatReader<R>, FormatError> {
    let identity = probe_format(&mut reader)?;
    FormatReader::with_identity(reader, limits, identity)
}

fn attached(pair: &(Vec<u8>, Vec<u8>)) -> FormatReader<Cursor<Vec<u8>>> {
    let mut reader = open(Cursor::new(pair.0.clone())).unwrap();
    reader
        .attach_attributes(Cursor::new(pair.1.clone()))
        .unwrap();
    reader
}

type Reads = Rc<RefCell<Vec<(u64, usize)>>>;

#[derive(Clone)]
struct ReadSpy {
    inner: Cursor<Vec<u8>>,
    reads: Reads,
}

impl Read for ReadSpy {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let start = self.inner.position();
        let read = self.inner.read(buffer)?;
        if read > 0 {
            self.reads.borrow_mut().push((start, read));
        }
        Ok(read)
    }
}

impl Seek for ReadSpy {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(position)
    }
}

fn spy(bytes: Vec<u8>) -> (ReadSpy, Reads) {
    let reads = Rc::new(RefCell::new(Vec::new()));
    let spy = ReadSpy {
        inner: Cursor::new(bytes),
        reads: Rc::clone(&reads),
    };
    (spy, reads)
}

fn footer_index(bytes: &[u8]) -> (usize, usize) {
    let footer = bytes.len() - 40;
    let offset = u64::from_le_bytes(bytes[footer + 8..footer + 16].try_into().unwrap()) as usize;
    let length = u64::from_le_bytes(bytes[footer + 16..footer + 24].try_into().unwrap()) as usize;
    (offset, length)
}

fn repair_index_and_footer_checksums(bytes: &mut [u8]) {
    let footer = bytes.len() - 40;
    let (index, length) = footer_index(bytes);
    let checksum = crate::crc32(&bytes[index..index + length]);
    bytes[footer + 32..footer + 36].copy_from_slice(&checksum.to_le_bytes());
    let checksum = crate::crc32(&bytes[footer..footer + 36]);
    bytes[footer + 36..].copy_from_slice(&checksum.to_le_bytes());
}

fn sum_lengths(reads: &Reads) -> u64 {
    reads.borrow().iter().map(|(_, n)| *n as u64).sum()
}

fn single_block(
    points: usize,
    schema: Vec<AttributeDescriptor>,
    columns: Vec<AttributeColumn>,
    position: [f64; 3],
    color: ColorRgb,
) -> (Vec<u8>, Vec<u8>) {
    let block = SourcePointBlock::new(ID, vec![position; points], vec![color; points], columns);
    write_v8_split(block, v8_metadata(points as u64, schema))
}

#[test]
fn shared_dataset_metadata_reuses_the_same_hierarchy_allocation() {
    let metadata = v8_metadata(4, vec![intensity()]);
    let first = metadata.shared_dataset();
    let second = metadata.shared_dataset();
    assert!(std::sync::Arc::ptr_eq(&first, &second));
}

#[test]
fn external_payload_buffers_materialize_selected_results_and_reject_bad_inputs() {
    use crate::{ReadSelection, ReadSource, SelectedRead};
    let (primary, attributes) = v8_pair();
    let slice = |source: ReadSource, offset: u64, length: u64| {
        let source = match source {
            ReadSource::Primary => &primary,
            ReadSource::Attributes => &attributes,
        };
        source[offset as usize..(offset + length) as usize].to_vec()
    };
    let reader = attached(&(primary.clone(), attributes.clone()));
    for selection in [
        ReadSelection::Positions,
        ReadSelection::Geometry,
        ReadSelection::DisplayColors,
        ReadSelection::Attribute { schema_index: 0 },
    ] {
        let plan = reader.plan_read(ID, selection).unwrap();
        let buffers = plan
            .ranges()
            .iter()
            .map(|r| slice(r.source(), r.offset(), r.stored_bytes()))
            .collect();
        let actual = reader
            .materialize_selected_from_buffers(plan, buffers)
            .unwrap();
        let mut sync = attached(&(primary.clone(), attributes.clone()));
        match (selection, actual) {
            (ReadSelection::Positions, SelectedRead::Positions(points)) => {
                assert_eq!(points.points(), sync.read_positions(ID).unwrap().points());
            }
            (ReadSelection::Geometry, SelectedRead::Geometry(points)) => {
                assert_eq!(points.points(), sync.read_geometry(ID).unwrap().points());
            }
            (ReadSelection::DisplayColors, SelectedRead::DisplayColors(colors)) => {
                assert_eq!(colors, sync.read_display_colors(ID).unwrap());
            }
            (ReadSelection::Attribute { schema_index }, SelectedRead::Attribute(column)) => {
                assert_eq!(
                    column.values(),
                    sync.read_attribute(ID, schema_index).unwrap().values()
                );
            }
            _ => panic!("materialized variant does not match selection"),
        }
    }
    let plan = reader.plan_read(ID, ReadSelection::Positions).unwrap();
    assert!(
        reader
            .materialize_selected_from_buffers(plan, vec![vec![0]])
            .is_err()
    );
    let plan = reader.plan_read(ID, ReadSelection::Positions).unwrap();
    let mut bad = primary.clone();
    let range = plan.ranges()[0];
    bad[range.offset() as usize] ^= 1;
    let buffers = vec![
        bad[range.offset() as usize..(range.offset() + range.stored_bytes()) as usize].to_vec(),
    ];
    assert!(
        reader
            .materialize_selected_from_buffers(plan, buffers)
            .is_err()
    );
    let plan = reader.plan_read(ID, ReadSelection::Positions).unwrap();
    let other = attached(&(primary, attributes));
    let range = plan.ranges()[0];
    let buffer = vec![vec![0; range.stored_bytes() as usize]];
    assert!(
        other
            .materialize_selected_from_buffers(plan, buffer)
            .is_err(),
        "a plan belongs to the reader that made it"
    );
}

#[test]
fn external_payload_buffers_materialize_geometry_without_attached_attributes() {
    use crate::{ReadSelection, SelectedRead};
    let (primary, _) = v8_pair();
    let reader = open(Cursor::new(primary.clone())).unwrap();
    assert!(!reader.has_attributes());
    let plan = reader.plan_read(ID, ReadSelection::Geometry).unwrap();
    assert_eq!(plan.ranges().len(), 1);
    let range = plan.ranges()[0];
    let payload =
        primary[range.offset() as usize..(range.offset() + range.stored_bytes()) as usize].to_vec();
    let SelectedRead::Geometry(actual) = reader
        .materialize_selected_from_buffers(plan, vec![payload])
        .unwrap()
    else {
        panic!("materialized variant does not match selection");
    };
    let mut sync = open(Cursor::new(primary)).unwrap();
    assert_eq!(actual.points(), sync.read_geometry(ID).unwrap().points());
}

#[test]
fn v8_split_reader_opens_from_async_index_and_companion_ranges() {
    use crate::{PagedCore, ReadSelection, ReadSource, SelectedRead};
    let (primary, attributes) = v8_pair();
    let limits = FormatReaderLimits::default();
    let file_length = primary.len() as u64;
    let footer = &primary[primary.len() - 40..];
    let location = PagedCore::locate_index(file_length, footer, &limits).unwrap();
    assert_eq!(location.offset + location.length, file_length - 40);
    let index = &primary[location.offset as usize..(location.offset + location.length) as usize];
    let mut external =
        crate::FormatReader::<Cursor<Vec<u8>>>::paged_external(file_length, footer, index, limits)
            .unwrap();
    assert!(!external.has_attributes());
    assert!(matches!(
        external.read_geometry(ID),
        Err(FormatError::ExternalPayloadBuffersRequired)
    ));

    let plan = external.plan_read(ID, ReadSelection::Geometry).unwrap();
    assert_eq!(plan.ranges().len(), 1);
    assert_eq!(plan.ranges()[0].source(), ReadSource::Primary);

    let attributes_footer = &attributes[attributes.len() - 40..];
    external
        .attach_attribute_identity(
            attributes.len() as u64,
            &attributes[..24],
            attributes_footer,
        )
        .unwrap();
    assert!(external.has_attributes());
    let plan = external.plan_read(ID, ReadSelection::Geometry).unwrap();
    assert_eq!(plan.ranges().len(), 2);
    let buffers = plan
        .ranges()
        .iter()
        .map(|range| {
            let source = match range.source() {
                ReadSource::Primary => primary.as_slice(),
                ReadSource::Attributes => attributes.as_slice(),
            };
            source[range.offset() as usize..(range.offset() + range.stored_bytes()) as usize]
                .to_vec()
        })
        .collect();
    let SelectedRead::Geometry(actual) = external
        .materialize_selected_from_buffers(plan, buffers)
        .unwrap()
    else {
        panic!("materialized variant does not match selection");
    };
    let mut sync = attached(&(primary.clone(), attributes.clone()));
    assert_eq!(actual.points(), sync.read_geometry(ID).unwrap().points());

    let mut bad_footer = footer.to_vec();
    bad_footer[0] ^= 1;
    assert!(PagedCore::locate_index(file_length, &bad_footer, &limits).is_err());
    let mut bad_index = index.to_vec();
    bad_index[0] ^= 1;
    assert!(
        crate::FormatReader::<Cursor<Vec<u8>>>::paged_external(
            file_length,
            footer,
            &bad_index,
            limits
        )
        .is_err()
    );
    let mut bad_attributes_footer = attributes_footer.to_vec();
    bad_attributes_footer[8] ^= 1;
    assert!(
        external
            .attach_attribute_identity(
                attributes.len() as u64,
                &attributes[..24],
                &bad_attributes_footer
            )
            .is_err()
    );
    let mut mismatched = attributes_footer.to_vec();
    mismatched[8] ^= 1;
    let crc = crate::crc32(&mismatched[..36]);
    mismatched[36..40].copy_from_slice(&crc.to_le_bytes());
    assert!(matches!(
        external.attach_attribute_identity(attributes.len() as u64, &attributes[..24], &mismatched),
        Err(FormatError::InvalidMetadata(_))
    ));
}

#[test]
fn reads_metadata_and_only_the_requested_point_block() {
    // Larger than the one-read tail window, so opening cannot have read the payload.
    let count = 400_000;
    let mut state = 12345_u32;
    let positions = (0..count)
        .map(|_| {
            std::array::from_fn(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                f64::from(state) / f64::from(u32::MAX)
            })
        })
        .collect();
    let values = (0..count).map(|i| (i * 7919 % 65_536) as u16).collect();
    let column = AttributeColumn::new(intensity(), AttributeValues::U16(values), count).unwrap();
    let block = SourcePointBlock::new(ID, positions, vec![COLORS[1]; count], vec![column]);
    let (primary, attributes) = write_v8_split(block, v8_metadata(count as u64, vec![intensity()]));
    assert!(primary.len() as u64 > crate::TAIL_READ_BYTES);
    let (reader, reads) = spy(primary.clone());
    let mut format = open(reader).unwrap();
    format.attach_attributes(spy(attributes).0).unwrap();
    let storage = format.block_storage_stats().unwrap();
    assert_eq!(storage.len(), 1);
    assert_eq!(storage[0].id, ID);
    assert_eq!(storage[0].point_count, count as u64);
    assert!(
        sum_lengths(&reads) <= crate::TAIL_READ_BYTES + 24 + 40,
        "open and stats read the header, the footer and the tail window, not the payloads"
    );
    assert_eq!(format.format_metadata().visual_scale(), 1.0);

    let entry = format.blocks[&ID].clone();
    reads.borrow_mut().clear();
    assert_eq!(format.attribute_range(0, 0).unwrap(), Some([0.0, 65535.0]));
    assert_eq!(format.attribute_range(0, 0).unwrap(), Some([0.0, 65535.0]));
    assert!(reads.borrow().is_empty(), "ranges come from the index");
    let block = format.read_point_block(ID).unwrap();
    assert_eq!(block.points().len(), count);
    assert_eq!(block.points().points()[1].color, COLORS[1]);
    let AttributeValues::U16(read_values) = block.attributes()[0].values() else {
        panic!("expected u16 values");
    };
    assert_eq!(read_values[1], 7919);
    assert!(sum_lengths(&reads) > 0);
    assert!(
        reads.borrow().iter().all(|(start, length)| {
            *start >= entry.offset && *start + *length as u64 <= entry.offset + entry.payload_length
        }),
        "reading a block touches only that block's primary payload bytes"
    );
}

#[test]
fn forked_readers_share_the_index_and_read_with_independent_cursors() {
    let (primary, _) = v8_pair();
    let reader = open(Cursor::new(primary.clone())).unwrap();
    assert_eq!(
        reader.attribute_range(0, 0).unwrap(),
        None.or(reader.attribute_range(0, 0).unwrap())
    );
    let first = reader.fork_reader(Cursor::new(primary.clone()));
    let second = reader.fork_reader(Cursor::new(primary));
    assert!(std::sync::Arc::ptr_eq(&first.blocks, &second.blocks));
    assert!(std::sync::Arc::ptr_eq(&first.metadata, &second.metadata));
    assert!(std::sync::Arc::ptr_eq(
        &first.attribute_range_cache,
        &second.attribute_range_cache
    ));

    let (a, b) = std::thread::scope(|scope| {
        let first = scope.spawn(move || {
            let mut reader = first;
            reader.read_positions(ID).unwrap().len()
        });
        let second = scope.spawn(move || {
            let mut reader = second;
            reader.read_positions(ID).unwrap().len()
        });
        (first.join().unwrap(), second.join().unwrap())
    });
    assert_eq!((a, b), (4, 4));
}

#[test]
fn scheduler_tickets_read_selected_format_blocks_through_dataset_source() {
    let mut reader = attached(&v8_pair());
    let hierarchy = reader.format_metadata().dataset().hierarchy().clone();
    let bounds = hierarchy.stats().root_bounds;
    let (min, max) = (bounds.min(), bounds.max());
    let frustum = SpatialFrustum::new(vec![
        FrustumPlane::new([1.0, 0.0, 0.0], -min[0]).unwrap(),
        FrustumPlane::new([-1.0, 0.0, 0.0], max[0]).unwrap(),
        FrustumPlane::new([0.0, 1.0, 0.0], -min[1]).unwrap(),
        FrustumPlane::new([0.0, -1.0, 0.0], max[1]).unwrap(),
        FrustumPlane::new([0.0, 0.0, 1.0], -min[2]).unwrap(),
        FrustumPlane::new([0.0, 0.0, -1.0], max[2]).unwrap(),
    ])
    .unwrap();
    let mut scheduler = BlockRequestScheduler::new(1).unwrap();
    scheduler
        .update_view(
            &hierarchy,
            &frustum,
            LodSelectionConfig::new(1.0, 4).unwrap(),
            min,
            |_| 0.0,
        )
        .unwrap();
    let request = scheduler.take_next().unwrap().unwrap();
    let block =
        viewer_core::DatasetSource::read_block(&mut reader, request.ticket().block_id()).unwrap();
    assert_eq!(block.points().len(), 4);
    assert_eq!(
        scheduler
            .complete(request.ticket(), Ok(block.id()))
            .unwrap(),
        RequestCompletion::Delivered
    );
    assert_eq!(scheduler.pending_count(), 0);
    assert_eq!(scheduler.in_flight_count(), 0);
}

#[test]
fn verifies_block_crc_when_read_and_keeps_open_lazy() {
    let (mut primary, _) = v8_pair();
    let entry = open(Cursor::new(primary.clone())).unwrap().blocks[&ID].clone();
    primary[entry.offset as usize] ^= 1;
    let mut corrupted = open(Cursor::new(primary)).unwrap();
    assert!(matches!(
        corrupted.read_positions(ID),
        Err(FormatError::InvalidBlockChecksum(id)) if id == ID
    ));
}

#[test]
fn applies_index_limit_before_reading_the_index() {
    let (primary, _) = v8_pair();
    let limits = FormatReaderLimits::new(1, 1024, 10, 10, 10, 10, 100).unwrap();
    assert!(matches!(
        open_with(Cursor::new(primary), limits),
        Err(FormatError::IndexExceedsLimit { limit: 1, .. })
    ));
}

#[test]
fn rejects_corrupt_index_and_blocks_over_the_configured_limit() {
    let (mut primary, _) = v8_pair();
    let (index_offset, _) = footer_index(&primary);
    primary[index_offset] ^= 1;
    assert!(matches!(
        open(Cursor::new(primary)),
        Err(FormatError::InvalidChecksum("index"))
    ));
    let limits = FormatReaderLimits::new(1 << 20, 1, 10, 10, 10, 10, 100).unwrap();
    assert!(matches!(
        open_with(Cursor::new(v8_pair().0), limits),
        Err(FormatError::BlockExceedsLimit { limit: 1, .. })
    ));
}

#[test]
fn rejects_truncated_and_unsupported_revision_files() {
    let (primary, _) = v8_pair();
    let mut truncated = primary.clone();
    truncated.pop();
    assert!(matches!(
        open(Cursor::new(truncated)),
        Err(FormatError::InvalidFooter)
    ));
    // Major 8 is recognized, but only the revision the writer emits has a layout.
    let revision = u32::from_le_bytes(primary[16..20].try_into().unwrap()) + 2;
    let mut unsupported = primary.clone();
    unsupported[16..20].copy_from_slice(&revision.to_le_bytes());
    let crc = crate::crc32(&unsupported[..20]);
    unsupported[20..24].copy_from_slice(&crc.to_le_bytes());
    assert!(matches!(
        open(Cursor::new(unsupported)),
        Err(FormatError::UnsupportedFormatVersion(version))
            if version.major() == 8 && version.revision() == revision
    ));
    let mut bad_footer = primary;
    let footer = bad_footer.len() - 40;
    bad_footer[footer..footer + 8].copy_from_slice(b"OCTVEND3");
    let checksum = crate::crc32(&bad_footer[footer..footer + 36]);
    bad_footer[footer + 36..].copy_from_slice(&checksum.to_le_bytes());
    assert!(matches!(
        open(Cursor::new(bad_footer)),
        Err(FormatError::InvalidFooter)
    ));
}

#[test]
fn constant_geometry_is_delta_encoded_and_reports_encoded_raw_bytes() {
    let column =
        AttributeColumn::new(intensity(), AttributeValues::U16(vec![128; 4096]), 4096).unwrap();
    let pair = single_block(
        4096,
        vec![intensity()],
        vec![column],
        [0.5; 3],
        ColorRgb::new(20, 40, 60),
    );
    let mut reader = attached(&pair);
    let storage = reader.block_storage_stats().unwrap();
    assert_ne!(
        storage[0].geometry_encoding,
        crate::GeometryEncoding::RawU32,
        "4096 identical points must not stay raw"
    );
    assert!(storage[0].raw_bytes < storage[0].logical_bytes);
    let points = reader.read_positions(ID).unwrap();
    assert_eq!(points.len(), 4096);
    assert!(points.points().iter().all(|p| p.position == [0.5; 3]));
}

#[test]
fn reads_visual_scale_and_hierarchy_semantics() {
    let metadata = v8_metadata(4, vec![intensity()])
        .with_visual_scale(50.0)
        .unwrap();
    let (primary, attributes) = write_v8_split(v8_block(), metadata);
    let reader = attached(&(primary, attributes));
    assert_eq!(reader.format_metadata().visual_scale(), 50.0);
    assert_eq!(
        reader.format_metadata().dataset().hierarchy().semantics(),
        viewer_core::HierarchySemantics::Replacement
    );
    assert!(reader.format_metadata().crs().is_some());
}

#[test]
fn reads_an_independently_compressed_block() {
    let column =
        AttributeColumn::new(intensity(), AttributeValues::U16(vec![128; 4096]), 4096).unwrap();
    let pair = single_block(
        4096,
        vec![intensity()],
        vec![column],
        [0.25, 0.5, 0.75],
        ColorRgb::new(20, 40, 60),
    );
    let mut reader = attached(&pair);
    assert!(
        reader.blocks[&ID]
            .payloads
            .iter()
            .any(|payload| payload.codec == crate::BlockCodec::Lz4)
    );
    let storage = reader.block_storage_stats().unwrap();
    assert_eq!(storage.len(), 1);
    assert_eq!(storage[0].point_count, 4096);
    assert!(storage[0].stored_bytes < storage[0].raw_bytes);
    let block = reader.read_point_block(ID).unwrap();
    assert_eq!(block.points().len(), 4096);
    assert!(block.estimated_memory_bytes() < 4096 * 128);
    assert_eq!(block.points().points()[2048].position, [0.25, 0.5, 0.75]);
    assert_eq!(
        block.attributes()[0].values(),
        &AttributeValues::U16(vec![128; 4096])
    );
}

#[test]
fn round_trips_a_larger_synthetic_block() {
    let count = 65_536;
    let pair = single_block(
        count,
        Vec::new(),
        Vec::new(),
        [0.25, 0.5, 0.75],
        ColorRgb::new(20, 40, 60),
    );
    let mut reader = attached(&pair);
    let block = reader.read_point_block(ID).unwrap();
    assert_eq!(block.points().len(), count);
    assert_eq!(block.points().points()[0].position, [0.25, 0.5, 0.75]);
    assert_eq!(
        block.points().points()[count - 1].color,
        ColorRgb::new(20, 40, 60)
    );
}

#[test]
fn payload_stats_report_columns_without_io() {
    let (primary, _) = v8_pair();
    let (backend, reads) = spy(primary);
    let reader = open(backend).unwrap();
    reads.borrow_mut().clear();
    let aggregates = reader.block_storage_stats().unwrap();
    let payloads = reader.block_payload_storage_stats().unwrap();
    assert!(reads.borrow().is_empty());
    assert_eq!(aggregates.len(), 1);
    assert_eq!(payloads[0].kind, super::BlockPayloadKind::Geometry);
    assert!(payloads.iter().any(|payload| payload.kind
        == super::BlockPayloadKind::Attribute {
            schema_index: 0,
            name: "intensity".to_owned()
        }));
    let aggregate = &aggregates[0];
    let matching = payloads.iter().filter(|payload| payload.id == aggregate.id);
    assert_eq!(
        matching.clone().map(|p| p.stored_bytes).sum::<u64>(),
        aggregate.stored_bytes
    );
    assert_eq!(
        matching.map(|p| p.raw_bytes).sum::<u64>(),
        aggregate.raw_bytes
    );
}

#[test]
fn checksums_are_lazy_and_independent_for_geometry_and_each_attribute() {
    let pair = v8_pair();
    let reader = attached(&pair);
    let entry = reader.blocks[&ID].clone();
    let (first, second) = (&entry.payloads, entry.payloads.len());
    for column_index in 0..second {
        if first[column_index].stored_length == 0 {
            continue;
        }
        // Only geometry lives in the primary file; colour and attributes are companion columns.
        let in_companion = column_index >= 1;
        let mut primary = pair.0.clone();
        let mut companion = pair.1.clone();
        if in_companion {
            companion[first[column_index].offset as usize] ^= 1;
        } else {
            primary[(entry.offset + first[column_index].offset) as usize] ^= 1;
        }
        let mut reader = open(Cursor::new(primary)).unwrap();
        reader.attach_attributes(Cursor::new(companion)).unwrap();
        for index in 0..second as u32 {
            let result = match index {
                0 => reader.read_positions(ID).map(|_| ()),
                1 => reader.read_display_colors(ID).map(|_| ()),
                _ => reader.read_attribute(ID, index - 2).map(|_| ()),
            };
            if index as usize == column_index {
                assert!(
                    matches!(result, Err(FormatError::InvalidBlockChecksum(_))),
                    "column {column_index}"
                );
            } else {
                result.unwrap();
            }
        }
        assert!(matches!(
            reader.read_point_block(ID),
            Err(FormatError::InvalidBlockChecksum(_))
        ));
    }
}

#[test]
fn rejects_invalid_payload_directory_before_payload_reads() {
    let (primary, _) = v8_pair();
    let reader = open(Cursor::new(primary.clone())).unwrap();
    let entry = reader.blocks[&ID].clone();
    assert!(entry.payloads.len() >= 3);
    let (index, _) = footer_index(&primary);
    // The payload directory is the tail of the single block entry in the index; find it by scanning
    // for each mutation that must be refused after repairing the checksums.
    let mut rejected = 0;
    for position in index..primary.len() - 40 {
        let mut corrupted = primary.clone();
        corrupted[position] ^= 0xFF;
        repair_index_and_footer_checksums(&mut corrupted);
        let (backend, reads) = spy(corrupted);
        let opened = open(backend);
        assert!(
            reads
                .borrow()
                .iter()
                .all(|(start, length)| *start + *length as u64 <= 24
                    || *start >= index as u64
                    || *start + *length as u64 == primary.len() as u64),
            "no payload byte is read while opening"
        );
        if opened.is_err() {
            rejected += 1;
        }
    }
    assert!(rejected > 50, "most index bytes are validated: {rejected}");
    let limits = FormatReaderLimits::new(1 << 20, 49, 10, 10, 10, 10, 100).unwrap();
    assert!(matches!(
        open_with(Cursor::new(primary), limits),
        Err(FormatError::BlockExceedsLimit { .. })
    ));
}

#[test]
fn round_trips_empty_geometry_and_attribute_columns() {
    let descriptor = AttributeDescriptor::new("empty", AttributeDataType::U64, 2).unwrap();
    let column =
        AttributeColumn::new(descriptor.clone(), AttributeValues::U64(Vec::new()), 0).unwrap();
    let pair = single_block(
        0,
        vec![descriptor],
        vec![column],
        [0.5; 3],
        ColorRgb::new(0, 0, 0),
    );
    let mut reader = attached(&pair);
    assert_eq!(reader.read_geometry(ID).unwrap().len(), 0);
    assert_eq!(
        reader.read_attribute(ID, 0).unwrap().values(),
        &AttributeValues::U64(Vec::new())
    );
    assert_eq!(reader.read_point_block(ID).unwrap().points().len(), 0);
    assert!(matches!(
        reader.read_attribute_row(ID, 0, 0),
        Err(FormatError::UnknownRow { .. })
    ));
}

#[test]
fn split_geometry_is_gray_and_selective_reads_do_not_touch_other_file() {
    let (primary, attributes) = v8_pair();
    let (primary_spy, primary_reads) = spy(primary);
    let (attribute_spy, attribute_reads) = spy(attributes);
    let mut reader = open(primary_spy).unwrap();
    assert!(!reader.has_attributes());
    let gray = reader.read_point_block(ID).unwrap();
    assert!(gray.attributes().is_empty());
    assert!(gray.points().points().iter().all(|p| p.color == GRAY));
    assert!(matches!(
        reader.read_attribute(ID, 0),
        Err(FormatError::AttributesUnavailable)
    ));
    reader.attach_attributes(attribute_spy).unwrap();
    primary_reads.borrow_mut().clear();
    attribute_reads.borrow_mut().clear();
    reader.read_positions(ID).unwrap();
    assert!(attribute_reads.borrow().is_empty());
    assert_eq!(
        sum_lengths(&primary_reads),
        reader.blocks[&ID].payloads[0].stored_length
    );
    primary_reads.borrow_mut().clear();
    assert_eq!(
        reader.read_attribute(ID, 0).unwrap().values(),
        &AttributeValues::U16(vec![0, 100, 200, 65535])
    );
    assert!(primary_reads.borrow().is_empty());
    assert_eq!(
        sum_lengths(&attribute_reads),
        reader.blocks[&ID].payloads[2].stored_length
    );
    assert_eq!(
        reader.read_geometry(ID).unwrap().points()[1].color,
        COLORS[1]
    );
    assert_eq!(reader.attribute_range(0, 0).unwrap(), Some([0.0, 65535.0]));
    assert!(reader.attribute_range(0, 1).is_err());
}

#[test]
fn split_rejects_wrong_truncated_or_corrupt_companion_without_losing_geometry() {
    let (primary, attributes) = v8_pair();
    // A companion written for a different index must be refused.
    let other_meta = v8_metadata_at(0.25, vec![ID], 4, vec![intensity()])
        .with_visual_scale(2.0)
        .unwrap();
    let (_, wrong_identity) = write_v8_split(v8_block(), other_meta);
    let mut reader = open(Cursor::new(primary)).unwrap();
    assert!(
        reader
            .attach_attributes(Cursor::new(wrong_identity))
            .is_err()
    );
    assert!(
        reader
            .attach_attributes(Cursor::new(attributes[..attributes.len() - 1].to_vec()))
            .is_err()
    );
    reader.read_positions(ID).unwrap();
    let mut corrupt = attributes;
    corrupt[24] ^= 1;
    reader.attach_attributes(Cursor::new(corrupt)).unwrap();
    reader.read_positions(ID).unwrap();
    reader.read_attribute(ID, 0).unwrap();
    assert!(matches!(
        reader.read_display_colors(ID),
        Err(FormatError::InvalidBlockChecksum(_))
    ));
}

#[test]
fn attribute_range_cache_validates_before_lookup_and_memoizes_none() {
    let (primary, _) = v8_pair();
    let reader = open(Cursor::new(primary)).unwrap();
    assert!(matches!(
        reader.attribute_range(1, 0),
        Err(FormatError::UnknownAttribute(1))
    ));
    assert!(matches!(
        reader.attribute_range(0, 1),
        Err(FormatError::InvalidSchema)
    ));
    assert!(reader.attribute_range_cache.lock().unwrap().is_empty());

    let empty_pair = single_block(
        0,
        vec![intensity()],
        vec![AttributeColumn::new(intensity(), AttributeValues::U16(Vec::new()), 0).unwrap()],
        [0.5; 3],
        ColorRgb::new(0, 0, 0),
    );
    let empty = open(Cursor::new(empty_pair.0)).unwrap();
    assert_eq!(empty.attribute_range(0, 0).unwrap(), None);
    assert_eq!(empty.attribute_range(0, 0).unwrap(), None);
    let cache = empty.attribute_range_cache.lock().unwrap();
    assert_eq!(cache.len(), 1);
    assert_eq!(cache[0].range, None);
}

#[test]
fn attribute_range_cache_preserves_multicomponent_integer_ranges_and_evicts_lru() {
    const COMPONENTS: usize = 65;
    let descriptor =
        AttributeDescriptor::new("vector", AttributeDataType::U64, COMPONENTS as u32).unwrap();
    let mut values = (0..COMPONENTS)
        .map(|c| c as u64)
        .chain((0..COMPONENTS).map(|c| c as u64 + 10))
        .collect::<Vec<_>>();
    values[0] = u64::MAX - 1;
    values[COMPONENTS] = u64::MAX;
    values[1] = 7;
    values[COMPONENTS + 1] = 11;
    let column = AttributeColumn::new(descriptor.clone(), AttributeValues::U64(values), 2).unwrap();
    let (primary, _) = single_block(
        2,
        vec![descriptor],
        vec![column],
        [0.5; 3],
        ColorRgb::new(1, 2, 3),
    );
    let reader = open(Cursor::new(primary)).unwrap();
    assert_eq!(
        reader.attribute_range(0, 0).unwrap(),
        Some([(u64::MAX - 1) as f64, u64::MAX as f64])
    );
    assert_eq!(reader.attribute_range(0, 1).unwrap(), Some([7.0, 11.0]));
    for component in 0..64 {
        reader.attribute_range(0, component).unwrap();
    }
    // Refresh component zero so component one becomes the least recently used entry.
    reader.attribute_range(0, 0).unwrap();
    reader.attribute_range(0, COMPONENTS as u32 - 1).unwrap();
    let cache = reader.attribute_range_cache.lock().unwrap();
    assert_eq!(cache.len(), 64);
    assert_eq!(cache.front().unwrap().component, 2);
    assert!(!cache.iter().any(|entry| entry.component == 1));
    assert!(cache.iter().any(|entry| entry.component == 0));
    drop(cache);
    assert_eq!(reader.attribute_range(0, 1).unwrap(), Some([7.0, 11.0]));
    assert_eq!(reader.attribute_range_cache.lock().unwrap().len(), 64);
}

#[test]
fn reading_an_absent_column_is_an_error_not_a_panic() {
    let mut reader = attached(&v8_pair());
    let payload_count = reader.blocks[&ID].payloads.len();
    assert_eq!(payload_count, 3);
    let blocks = std::sync::Arc::get_mut(&mut reader.blocks).unwrap();
    let entry = blocks.get_mut(&ID).unwrap();
    let offset = entry.payloads[2].offset;
    entry.payloads[2] = crate::layout::PayloadDirectoryEntry::not_stored(
        crate::layout::StoredColumn::Absent,
        offset,
    );
    assert!(!reader.attribute_present(ID, 0).unwrap());
    assert!(!reader.attribute_present(ID, 1).unwrap(), "unknown index");
    assert!(
        !reader.attribute_present(ID, u32::MAX).unwrap(),
        "huge index"
    );
    assert!(
        reader.attribute_present(BlockId::new(999), 0).is_err(),
        "unknown block is an error, not an absent column"
    );
    assert!(matches!(
        reader.read_attribute(ID, 0),
        Err(FormatError::AttributeAbsent { block, attribute: 0 }) if block == ID
    ));
    assert!(matches!(
        reader.plan_read(ID, crate::ReadSelection::Attribute { schema_index: 0 }),
        Err(FormatError::AttributeAbsent { attribute: 0, .. })
    ));
    assert!(matches!(
        reader.read_attribute_row(ID, 0, 0),
        Err(FormatError::AttributeAbsent { .. })
    ));
    // Geometry, the whole block and the stats still work and skip the absent column.
    let block = reader.read_point_block(ID).unwrap();
    assert_eq!((block.points().len(), block.attributes().len()), (4, 0));
    let stats = reader.block_payload_storage_stats().unwrap();
    assert_eq!(stats.iter().filter(|stat| stat.id == ID).count(), 2);
    // A derived colour whose source column is not stored is an error, as is one outside the schema.
    for from in [2, 99] {
        let entry = std::sync::Arc::get_mut(&mut reader.blocks)
            .unwrap()
            .get_mut(&ID)
            .unwrap();
        let offset = entry.payloads[1].offset;
        entry.payloads[1] = crate::layout::PayloadDirectoryEntry::not_stored(
            crate::layout::StoredColumn::Derived { from },
            offset,
        );
        assert!(reader.read_display_colors(ID).is_err());
    }
}
