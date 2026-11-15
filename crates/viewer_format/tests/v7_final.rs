//! Final `v7`: the `v7.5-pre` byte layout under its own identity (ADR 0044).

mod common;

use std::io::Cursor;

use common::{
    BLOCK_ID, COLORS, POINTS, descriptors, fixture_attributes, global_metadata, prefix_of,
    repair_index_and_footer,
};
use viewer_core::{AttributeDataType, AttributeDescriptor, AttributeValues, BlockId, NodeId};
use viewer_format::{
    CrsRaw, CrsRecord, FormatError, FormatMetadata, FormatReader, FormatReaderLimits,
    FormatVersion, FormatWriter, FormatWriterOptions, OctreeReader, PagedCore, RowOrder,
    SourcePointBlock, crc32,
};

const V7: FormatVersion = FormatVersion::V7;
const V75: FormatVersion = FormatVersion::V7_PRE5;

fn crs() -> CrsRecord {
    CrsRecord::new(
        "EPSG".to_owned(),
        32632,
        CrsRaw::Wkt("PROJCS[\"WGS 84 / UTM zone 32N\",AUTHORITY[\"EPSG\",\"32632\"]]".to_owned()),
    )
    .unwrap()
}

fn metadata_with(schema: Vec<AttributeDescriptor>) -> FormatMetadata {
    let level = 32;
    let prefix = [
        prefix_of(262_050.0, level),
        prefix_of(4_424_050.0, level),
        prefix_of(50.0, level),
    ];
    global_metadata(level, prefix, vec![BLOCK_ID], 4, schema, crs(), None)
}

fn options() -> FormatWriterOptions {
    FormatWriterOptions::new(0.001, 1 << 20)
        .unwrap()
        .with_row_order(RowOrder::Hybrid)
}

fn write_pair(version: FormatVersion) -> (Vec<u8>, Vec<u8>) {
    let mut writer = FormatWriter::new_split_for_version(
        version,
        Cursor::new(Vec::new()),
        Cursor::new(Vec::new()),
        metadata_with(descriptors()),
        options(),
    )
    .unwrap();
    writer
        .write_block(SourcePointBlock::new(
            BLOCK_ID,
            POINTS.to_vec(),
            COLORS.to_vec(),
            fixture_attributes(),
        ))
        .unwrap();
    let (primary, attributes) = writer.finish_pair().unwrap();
    (primary.into_inner(), attributes.unwrap().into_inner())
}

fn set_header_revision(bytes: &mut [u8], revision: u32) {
    bytes[16..20].copy_from_slice(&revision.to_le_bytes());
    let checksum = crc32(&bytes[..20]);
    bytes[20..24].copy_from_slice(&checksum.to_le_bytes());
}

#[test]
fn truncated_paged_companion_is_rejected_without_underflow() {
    let (primary, attributes) = write_pair(V7);
    let truncated = attributes[..30].to_vec();
    let mut eager = FormatReader::new(Cursor::new(primary.clone())).unwrap();
    assert!(
        eager
            .attach_attributes(Cursor::new(truncated.clone()))
            .is_err()
    );
    let mut lazy =
        FormatReader::open_lazy(Cursor::new(primary), FormatReaderLimits::default()).unwrap();
    let error = lazy.attach_attributes(Cursor::new(truncated)).unwrap_err();
    assert!(matches!(error, FormatError::FileTooShort), "{error:?}");
}

#[test]
fn v7_round_trips_paged_pair() {
    let (primary, attributes) = write_pair(V7);
    assert_eq!(&primary[primary.len() - 40..][..8], b"OCTVEND7");
    assert_eq!(&attributes[attributes.len() - 40..][..8], b"OCTVEND7");
    assert!(PagedCore::is_paged_footer(&primary[primary.len() - 40..]));

    let mut eager = FormatReader::new(Cursor::new(primary.clone())).unwrap();
    eager
        .attach_attributes(Cursor::new(attributes.clone()))
        .unwrap();
    let mut lazy =
        FormatReader::open_lazy(Cursor::new(primary), FormatReaderLimits::default()).unwrap();
    lazy.attach_attributes(Cursor::new(attributes)).unwrap();
    for reader in [&eager, &lazy] {
        assert_eq!(reader.format_version(), V7);
    }
    let eager_nodes: Vec<_> = eager
        .format_metadata()
        .dataset()
        .hierarchy()
        .query(
            eager
                .format_metadata()
                .dataset()
                .hierarchy()
                .stats()
                .root_bounds,
        )
        .map(|node| (node.id(), node.point_count()))
        .collect();
    assert_eq!(eager_nodes, [(NodeId::new(0), 4)]);
    assert_eq!(
        lazy.format_metadata()
            .dataset()
            .hierarchy()
            .node(NodeId::new(0))
            .unwrap()
            .point_count(),
        4
    );
    // The paged format numbers blocks itself, so the id comes from the node.
    let block = eager
        .format_metadata()
        .dataset()
        .hierarchy()
        .node(NodeId::new(0))
        .unwrap()
        .blocks()[0];
    let a = eager.read_positions(block).unwrap();
    let b = lazy.read_positions(block).unwrap();
    assert_eq!(a.points().len(), 4);
    assert_eq!(a.points(), b.points());
    assert_eq!(
        eager.read_attribute(block, 0).unwrap(),
        lazy.read_attribute(block, 0).unwrap()
    );
}

/// Byte ranges that carry the identity of a paged file: header revision and CRC, index magic,
/// and the footer (magic, index CRC, footer CRC).
fn identity_ranges(bytes: &[u8], has_index: bool) -> Vec<std::ops::Range<usize>> {
    let end = bytes.len();
    let mut ranges = vec![16..24, end - 40..end - 32, end - 8..end];
    if has_index {
        let offset = u64::from_le_bytes(bytes[end - 32..end - 24].try_into().unwrap()) as usize;
        ranges.push(offset..offset + 8);
        ranges.push(end - 8..end - 4);
    }
    ranges
}

#[test]
fn v7_and_v7_5_pre_differ_only_in_identity() {
    let (v7_primary, v7_attributes) = write_pair(V7);
    let (pre_primary, pre_attributes) = write_pair(V75);
    for (v7, pre, has_index) in [
        (&v7_primary, &pre_primary, true),
        (&v7_attributes, &pre_attributes, false),
    ] {
        assert_eq!(v7.len(), pre.len());
        let ranges = identity_ranges(v7, has_index);
        let mut touched = 0;
        for (position, (a, b)) in v7.iter().zip(pre.iter()).enumerate() {
            if a != b {
                touched += 1;
                assert!(
                    ranges.iter().any(|range| range.contains(&position)),
                    "byte {position} differs outside the identity"
                );
            }
        }
        assert!(touched > 0);
    }
    assert_eq!(&v7_primary[..8], &pre_primary[..8]);
    assert_eq!(v7_primary[16..20], 0_u32.to_le_bytes());
    assert_eq!(pre_primary[16..20], 5_u32.to_le_bytes());
}

#[test]
fn eager_reader_rejects_mismatched_header_and_footer() {
    let (v7_primary, _) = write_pair(V7);
    let (pre_primary, _) = write_pair(V75);
    let mut v7_header_pre_footer = pre_primary.clone();
    set_header_revision(&mut v7_header_pre_footer, 0);
    let mut pre_header_v7_footer = v7_primary.clone();
    set_header_revision(&mut pre_header_v7_footer, 5);
    for bytes in [v7_header_pre_footer, pre_header_v7_footer] {
        assert!(matches!(
            FormatReader::new(Cursor::new(bytes)),
            Err(FormatError::InvalidFooter)
        ));
    }
    // A footer whose magic is swapped with the index left alone fails as well.
    let mut swapped = v7_primary;
    let end = swapped.len();
    swapped[end - 40..end - 32].copy_from_slice(b"OCTVE75P");
    repair_index_and_footer(&mut swapped);
    assert!(FormatReader::new(Cursor::new(swapped)).is_err());
}

#[test]
fn writer_rejects_more_than_256_attributes() {
    let wide = |count: usize| -> Vec<AttributeDescriptor> {
        (0..count)
            .map(|index| {
                AttributeDescriptor::new(format!("column_{index}"), AttributeDataType::U8, 1)
                    .unwrap()
            })
            .collect()
    };
    for version in FormatVersion::WRITABLE {
        let metadata = if version.uses_global_frame() {
            metadata_with(wide(257))
        } else {
            common::metadata(4, wide(257))
        };
        let result =
            FormatWriter::new_for_version(version, Cursor::new(Vec::new()), metadata, options());
        let result = match result {
            Err(FormatError::InvalidMetadata(_)) if version.uses_pages() => {
                // Paged layouts are only written as a pair.
                FormatWriter::new_split_for_version(
                    version,
                    Cursor::new(Vec::new()),
                    Cursor::new(Vec::new()),
                    metadata_with(wide(257)),
                    options(),
                )
            }
            other => other,
        };
        assert!(
            matches!(
                result,
                Err(FormatError::TooManyAttributes {
                    count: 257,
                    limit: 256
                })
            ),
            "{version}"
        );
    }
    // 256 columns still work, so the limit is the default reader limit and nothing lower.
    let mut writer = FormatWriter::new_split_for_version(
        V7,
        Cursor::new(Vec::new()),
        Cursor::new(Vec::new()),
        metadata_with(wide(256)),
        options(),
    )
    .unwrap();
    let columns = wide(256)
        .into_iter()
        .map(|descriptor| {
            viewer_core::AttributeColumn::new(
                descriptor,
                AttributeValues::U8(vec![1; POINTS.len()]),
                POINTS.len(),
            )
            .unwrap()
        })
        .collect();
    writer
        .write_block(SourcePointBlock::new(
            BlockId::new(BLOCK_ID.get()),
            POINTS.to_vec(),
            COLORS.to_vec(),
            columns,
        ))
        .unwrap();
    let (primary, _) = writer.finish_pair().unwrap();
    FormatReader::new(Cursor::new(primary.into_inner())).unwrap();
}
