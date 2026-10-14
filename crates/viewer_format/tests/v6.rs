use std::io::{Cursor, Read, Seek, SeekFrom};

use viewer_core::{
    AttributeColumn, AttributeDataType, AttributeDescriptor, AttributeValues, BlockId, ColorRgb,
    CoordinateFrame, DatasetMetadata, DatasetSource, NodeId, OccupiedOctreeRoot,
    OccupiedPointBlock, SpatialBounds, SpatialHierarchy, SpatialNode, VoxelCoord,
};
use viewer_format::{
    FileRole, FormatBlockEncoder, FormatError, FormatIdentity, FormatMetadata, FormatReader,
    FormatVersion, FormatWriter, FormatWriterOptions, GeometryEncoding, OctreeReader,
    QuantizedOccupancy, SourcePointBlock, prepare_point_block, quantize_local_positions,
};

const BLOCK_ID: BlockId = BlockId::new(17);
const POINTS: [[f64; 3]; 4] = [
    [0.125, 0.25, 0.5],
    [0.25, 0.375, 0.625],
    [0.5, 0.5, 0.75],
    [0.875, 0.75, 0.125],
];
const COLORS: [ColorRgb; 4] = [
    ColorRgb::new(1, 2, 3),
    ColorRgb::new(40, 50, 60),
    ColorRgb::new(127, 128, 129),
    ColorRgb::new(253, 254, 255),
];

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

fn options() -> FormatWriterOptions {
    FormatWriterOptions::new(0.001, 1024 * 1024).unwrap()
}

fn descriptors() -> Vec<AttributeDescriptor> {
    [
        ("u8", AttributeDataType::U8),
        ("i8", AttributeDataType::I8),
        ("u16", AttributeDataType::U16),
        ("i16", AttributeDataType::I16),
        ("u32", AttributeDataType::U32),
        ("i32", AttributeDataType::I32),
        ("u64", AttributeDataType::U64),
        ("i64", AttributeDataType::I64),
        ("f32", AttributeDataType::F32),
        ("f64", AttributeDataType::F64),
    ]
    .into_iter()
    .map(|(name, kind)| AttributeDescriptor::new(name, kind, 1).unwrap())
    .collect()
}

fn fixture_attributes() -> Vec<AttributeColumn> {
    let schema = descriptors();
    let values = [
        AttributeValues::U8(vec![0, 1, 255, 42]),
        AttributeValues::I8(vec![i8::MIN, -1, i8::MAX, 0]),
        AttributeValues::U16(vec![0, 1, u16::MAX, 42]),
        AttributeValues::I16(vec![i16::MIN, -1, i16::MAX, 0]),
        AttributeValues::U32(vec![0, 1, u32::MAX, 42]),
        AttributeValues::I32(vec![i32::MIN, -1, i32::MAX, 0]),
        AttributeValues::U64(vec![0, 1, u64::MAX, 9_007_199_254_740_993]),
        AttributeValues::I64(vec![i64::MIN, -1, i64::MAX, 0]),
        AttributeValues::F32(vec![
            f32::from_bits(0x7fc0_0042),
            -0.0,
            f32::INFINITY,
            f32::MIN,
        ]),
        AttributeValues::F64(vec![
            f64::from_bits(0x7ff8_0000_0000_0042),
            -0.0,
            f64::INFINITY,
            f64::MIN,
        ]),
    ];
    schema
        .into_iter()
        .zip(values)
        .map(|(descriptor, values)| AttributeColumn::new(descriptor, values, POINTS.len()).unwrap())
        .collect()
}

fn metadata(point_count: u64, schema: Vec<AttributeDescriptor>) -> FormatMetadata {
    let bounds = SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap();
    let node = SpatialNode::new(
        NodeId::new(0),
        None,
        0,
        bounds,
        point_count,
        0.0,
        vec![BLOCK_ID],
    )
    .unwrap();
    let hierarchy = SpatialHierarchy::new(vec![node], schema, Some(point_count)).unwrap();
    FormatMetadata::new(
        DatasetMetadata::new(CoordinateFrame::new([0.0; 3]).unwrap(), hierarchy),
        None,
        None,
        Vec::new(),
    )
    .unwrap()
}

fn fixture_block() -> SourcePointBlock {
    SourcePointBlock::new(
        BLOCK_ID,
        POINTS.to_vec(),
        COLORS.to_vec(),
        fixture_attributes(),
    )
}

fn write_split(
    version: FormatVersion,
    block: SourcePointBlock,
    meta: FormatMetadata,
) -> (Vec<u8>, Vec<u8>) {
    let mut writer = FormatWriter::new_split_for_version(
        version,
        Cursor::new(Vec::new()),
        Cursor::new(Vec::new()),
        meta,
        options(),
    )
    .unwrap();
    writer.write_block(block).unwrap();
    let (primary, attributes) = writer.finish_pair().unwrap();
    (primary.into_inner(), attributes.unwrap().into_inner())
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
fn unsupported_v6_final_and_pre5_are_rejected_by_writer_encoder_and_reader() {
    let meta = metadata(POINTS.len() as u64, descriptors());
    let options = options();
    let versions = [
        FormatVersion::V6,
        FormatVersion::V6Pre(std::num::NonZeroU32::new(5).unwrap()),
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
    for (version, split) in [FormatVersion::V6_PRE2, FormatVersion::V6_PRE4]
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
            if version == FormatVersion::V6_PRE4 {
                GeometryEncoding::CompactOccupied
            } else {
                GeometryEncoding::Occupied
            }
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

fn repair_index_and_footer(bytes: &mut [u8]) {
    let footer = bytes.len() - 40;
    let index = u64::from_le_bytes(bytes[footer + 8..footer + 16].try_into().unwrap()) as usize;
    let length = u64::from_le_bytes(bytes[footer + 16..footer + 24].try_into().unwrap()) as usize;
    let crc = viewer_format::crc32(&bytes[index..index + length]);
    bytes[footer + 32..footer + 36].copy_from_slice(&crc.to_le_bytes());
    let crc = viewer_format::crc32(&bytes[footer..footer + 36]);
    bytes[footer + 36..].copy_from_slice(&crc.to_le_bytes());
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
    let descriptor = AttributeDescriptor::new("nan_payload", AttributeDataType::F64, 1).unwrap();
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
    let (primary, attrs) = write_split(FormatVersion::V6_PRE4, make(), meta.clone());
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
    let (_, attrs4) = write_split(FormatVersion::V6_PRE4, make(), meta.clone());
    assert!(wrong.attach_attributes(Cursor::new(attrs4)).is_err());
    let mut writer = FormatWriter::new_for_version(
        FormatVersion::V6_PRE4,
        Cursor::new(Vec::new()),
        meta,
        options(),
    )
    .unwrap();
    writer.write_block(make()).unwrap();
    let original = writer.finish().unwrap().into_inner();
    let footer = original.len() - 40;
    let index = u64::from_le_bytes(original[footer + 8..footer + 16].try_into().unwrap()) as usize;
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
