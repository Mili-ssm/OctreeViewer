use std::io::{Cursor, Read, Seek, SeekFrom};

use viewer_core::{
    AttributeColumn, AttributeDataType, AttributeDescriptor, AttributeValues, BlockId, ColorRgb,
    CoordinateFrame, DatasetMetadata, NodeId, SpatialBounds, SpatialHierarchy, SpatialNode,
};
use viewer_format::{
    FileRole, FormatBlockEncoder, FormatError, FormatIdentity, FormatMetadata, FormatReader,
    FormatVersion, FormatWriter, FormatWriterOptions, GeometryEncoding, OctreeReader,
    SourcePointBlock, prepare_point_block,
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

    for version in [FormatVersion::V5, FormatVersion::V6_PRE1] {
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
    for version in [FormatVersion::V5, FormatVersion::V6_PRE1] {
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
fn unsupported_v6_final_and_pre2_are_rejected_by_writer_encoder_and_reader() {
    let meta = metadata(POINTS.len() as u64, descriptors());
    let options = options();
    let versions = [
        FormatVersion::V6,
        FormatVersion::V6Pre(std::num::NonZeroU32::new(2).unwrap()),
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
