//! `v8`: attribute payloads that take the Dictionary, `BytePlanes` and Constant codecs read back
//! exactly, and the retired zstd codec bytes (5 and 6) are refused by the reader (ADR 0060).

mod common;

use std::io::Cursor;

use common::{global_metadata, repair_index_and_footer};
use viewer_core::{
    AttributeColumn, AttributeDataType, AttributeDescriptor, AttributeValues, BlockId, ColorRgb,
};
use viewer_format::{
    BlockCodec, BlockPayloadKind, CodecError, CrsRecord, FormatError, FormatReader, FormatVersion,
    FormatWriter, FormatWriterOptions, PagedReader, SourcePointBlock, crc32,
};

const ROWS: usize = 4096;
const BLOCK: BlockId = BlockId::new(0);

fn descriptor(name: &str, kind: AttributeDataType, components: u32) -> AttributeDescriptor {
    AttributeDescriptor::new(name, kind, components).unwrap()
}

/// `(name, values)` per column: four distinct values (Dictionary), one repeated tuple (Constant)
/// and 64-bit values whose upper half never changes (`BytePlanes`).
fn columns() -> Vec<(AttributeDescriptor, AttributeValues)> {
    let mut state = 0x91ab_032f_u32;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state
    };
    // The format keeps dictionary candidates only for 1-, 2- and 4-byte tuples.
    let dictionary: Vec<u32> = (0..ROWS)
        .map(|_| [3_u32, 700, 4_000_000_000, 65_535][(next() % 4) as usize])
        .collect();
    let planes: Vec<u64> = (0..ROWS)
        .map(|_| 0x0102_0304_0000_0000 | u64::from(next()))
        .collect();
    vec![
        (
            descriptor("dictionary", AttributeDataType::U32, 1),
            AttributeValues::U32(dictionary),
        ),
        (
            descriptor("planes", AttributeDataType::U64, 1),
            AttributeValues::U64(planes),
        ),
        (
            descriptor("constant", AttributeDataType::U8, 1),
            AttributeValues::U8(vec![7; ROWS]),
        ),
    ]
}

/// A single-node `v8` split pair holding `columns()`.
fn write() -> (Vec<u8>, Vec<u8>) {
    let columns = columns();
    let schema: Vec<_> = columns.iter().map(|(d, _)| d.clone()).collect();
    let prefix = [5, 7, 9].map(|k| (1_u64 << 47) + k);
    let crs = CrsRecord::new(
        "EPSG".to_owned(),
        32632,
        viewer_format::CrsRaw::Wkt("PROJCS[\"WGS 84 / UTM zone 32N\"]".to_owned()),
    )
    .unwrap();
    let metadata = global_metadata(48, prefix, vec![BLOCK], ROWS as u64, schema, crs, None)
        .with_point_distance_quantiles(
            viewer_format::PointDistanceQuantiles::new(0.25, 0.5, 1.5).unwrap(),
        )
        .with_lod_attribute_columns(vec![0])
        .with_display_colour_source(viewer_format::DisplayColourSource::None);
    let points = (0..ROWS)
        .map(|i| [i as f64 / ROWS as f64, 0.5, (i % 64) as f64 / 64.0])
        .collect();
    let attributes = columns
        .into_iter()
        .map(|(d, v)| AttributeColumn::new(d, v, ROWS).unwrap())
        .collect();
    let block = SourcePointBlock::new(
        BLOCK,
        points,
        vec![ColorRgb::new(1, 2, 3); ROWS],
        attributes,
    );
    let mut writer = FormatWriter::new_split_for_version(
        FormatVersion::V8,
        Cursor::new(Vec::new()),
        Cursor::new(Vec::new()),
        metadata,
        FormatWriterOptions::new(0.01, 1 << 24).unwrap(),
    )
    .unwrap();
    writer.write_block(block).unwrap();
    let (primary, attributes) = writer.finish_pair().unwrap();
    (primary.into_inner(), attributes.unwrap().into_inner())
}

fn open(
    primary: Vec<u8>,
    attributes: Vec<u8>,
) -> Result<FormatReader<Cursor<Vec<u8>>>, FormatError> {
    let mut reader = FormatReader::new(Cursor::new(primary))?;
    reader.attach_attributes(Cursor::new(attributes))?;
    Ok(reader)
}

#[test]
fn v8_dictionary_byte_planes_and_constant_payloads_read_back_the_input() {
    let (primary, attributes) = write();
    let mut reader = open(primary, attributes).unwrap();
    let stats = reader.block_payload_storage_stats().unwrap();
    let codec_of = |name: &str| {
        stats
            .iter()
            .find(|s| matches!(&s.kind, BlockPayloadKind::Attribute { name: n, .. } if n == name))
            .unwrap()
            .codec
    };
    assert_eq!(codec_of("dictionary"), BlockCodec::Dictionary);
    assert_eq!(codec_of("planes"), BlockCodec::BytePlanes);
    assert_eq!(codec_of("constant"), BlockCodec::Constant);
    for (index, (_, values)) in columns().iter().enumerate() {
        assert_eq!(
            reader.read_attribute(BLOCK, index as u32).unwrap().values(),
            values,
            "column {index}"
        );
    }
}

/// Patches the directory codec byte of the `name` payload to `codec` and repairs the page and
/// footer checksums, so the reader gets past integrity checks and meets the unknown codec itself.
fn with_codec_byte(name: &str, codec: u8) -> (Vec<u8>, Vec<u8>) {
    let (mut primary, attributes) = write();
    let reader = open(primary.clone(), attributes.clone()).unwrap();
    let stored = reader
        .block_payload_storage_stats()
        .unwrap()
        .into_iter()
        .find(|s| matches!(&s.kind, BlockPayloadKind::Attribute { name: n, .. } if n == name))
        .unwrap();
    let footer = primary.len() - 40;
    let root_length = PagedReader::open(Cursor::new(primary.clone()))
        .unwrap()
        .root_page()
        .encoded_len();
    let root_start = footer - root_length;
    // A payload directory entry is the codec byte, a u32 checksum and the u64 stored length.
    let hits: Vec<usize> = (root_start..footer - 13)
        .filter(|&at| {
            primary[at] == stored.codec as u8
                && primary[at + 5..at + 13] == stored.stored_bytes.to_le_bytes()
        })
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "directory entry of {name} not found uniquely"
    );
    primary[hits[0]] = codec;
    let crc = crc32(&primary[root_start..footer - 4]);
    primary[footer - 4..footer].copy_from_slice(&crc.to_le_bytes());
    repair_index_and_footer(&mut primary);
    (primary, attributes)
}

#[test]
fn v8_reader_refuses_the_retired_zstd_codec_bytes() {
    for name in ["dictionary", "planes", "constant"] {
        for byte in [5_u8, 6] {
            let (primary, attributes) = with_codec_byte(name, byte);
            // The directory is decoded at open: the error is the codec, not a checksum or a panic.
            assert!(
                matches!(
                    open(primary, attributes),
                    Err(FormatError::Codec(CodecError::UnsupportedCodec(b))) if b == byte
                ),
                "{name} codec byte {byte}"
            );
        }
    }
}

/// The async reader (the path the native host and the wasm bindings share) opens the pair through
/// the same directory decoder.
#[cfg(feature = "async-reader")]
#[test]
fn v8_async_reader_refuses_the_retired_zstd_codec_bytes() {
    use viewer_format::{AsyncFormatReader, AsyncReadError};
    let dir = std::env::temp_dir().join(format!("viewer-format-v8-codecs-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let mut outcomes = Vec::new();
    for byte in [5_u8, 6] {
        let (primary, attributes) = with_codec_byte("dictionary", byte);
        let (primary_path, attributes_path) = (dir.join("a.oct"), dir.join("a.octattrs"));
        std::fs::write(&primary_path, primary).unwrap();
        std::fs::write(&attributes_path, attributes).unwrap();
        outcomes.push((
            byte,
            runtime.block_on(AsyncFormatReader::open(
                &primary_path,
                Some(attributes_path),
                1,
                u64::MAX,
            )),
        ));
    }
    std::fs::remove_dir_all(&dir).unwrap();
    for (byte, outcome) in outcomes {
        assert!(
            matches!(
                outcome,
                Err(AsyncReadError::Format(FormatError::Codec(CodecError::UnsupportedCodec(b)))) if b == byte
            ),
            "codec byte {byte}"
        );
    }
}
