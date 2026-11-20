//! `v8.1-pre`: blocks of nodes with children store only the columns of the file's LOD set.

mod common;

use std::io::Cursor;

use common::{repair_index_and_footer, root_origin};
use viewer_core::{
    AttributeColumn, AttributeDataType, AttributeDescriptor, AttributeValues, BlockId, ColorRgb,
    CoordinateFrame, DatasetMetadata, NodeId, SpatialHierarchy, SpatialNode,
};
use viewer_format::{
    BlockPayloadKind, CrsRecord, FormatError, FormatMetadata, FormatReader, FormatReaderLimits,
    FormatVersion, FormatWriter, FormatWriterOptions, GlobalFrame, NodeAttributeRange, NodeCell,
    PagedReader, SourcePointBlock,
};

const V8_PRE1: FormatVersion = FormatVersion::V8_PRE1;
const ROOT_LEVEL: u32 = 40;
const ROOT_PREFIX: [u64; 3] = [(1 << 39) + 3, (1 << 39) + 4, (1 << 39) + 5];
/// Node 0 is the only LOD node; nodes 1 and 2 are its leaves. Source block id = node id + 1; the file
/// numbers blocks from 0 in node order, so its LOD block is 0 and its leaves are 1 and 2.
const NODES: u64 = 3;

fn schema() -> Vec<AttributeDescriptor> {
    vec![
        AttributeDescriptor::new("intensity", AttributeDataType::U16, 1).unwrap(),
        AttributeDescriptor::new("scan_angle", AttributeDataType::U8, 1).unwrap(),
        AttributeDescriptor::new("classification", AttributeDataType::U8, 1).unwrap(),
    ]
}

fn crs() -> CrsRecord {
    CrsRecord::new(
        "EPSG".to_owned(),
        32632,
        viewer_format::CrsRaw::Wkt(
            "PROJCS[\"WGS 84 / UTM zone 32N\",AUTHORITY[\"EPSG\",\"32632\"]]".into(),
        ),
    )
    .unwrap()
}

fn quantiles() -> viewer_format::PointDistanceQuantiles {
    viewer_format::PointDistanceQuantiles::new(0.25, 0.5, 1.5).unwrap()
}

/// Metadata without a LOD set, plus one two-point block per node.
fn dataset() -> (FormatMetadata, Vec<SourcePointBlock>) {
    let depths = [0_u32, 1, 1];
    let prefixes: [[u64; 3]; 3] = [
        ROOT_PREFIX,
        std::array::from_fn(|axis| ROOT_PREFIX[axis] << 1),
        std::array::from_fn(|axis| (ROOT_PREFIX[axis] << 1) | 1),
    ];
    let cells: Vec<_> = (0..NODES as usize)
        .map(|i| {
            let spacing = if i == 0 {
                2.0_f64.powi(41 - ROOT_LEVEL as i32)
            } else {
                2.0_f64.powi(-5)
            };
            (NodeId::new(i as u64), NodeCell::new(prefixes[i], spacing))
        })
        .collect();
    let origin = root_origin(ROOT_LEVEL, ROOT_PREFIX);
    let frame = GlobalFrame::new(
        ROOT_LEVEL,
        ROOT_PREFIX,
        origin,
        origin.map(|q| q + 100 * 32_768),
        crs(),
        None,
        cells,
    )
    .unwrap();
    let mut nodes = Vec::new();
    let mut blocks = Vec::new();
    for i in 0..NODES as usize {
        let bounds = frame.node_bounds(depths[i], prefixes[i]).unwrap();
        nodes.push(
            SpatialNode::new(
                NodeId::new(i as u64),
                (i > 0).then(|| NodeId::new(0)),
                depths[i],
                bounds,
                2,
                0.0,
                vec![BlockId::new(i as u64 + 1)],
            )
            .unwrap(),
        );
        let at = |fraction: f64| {
            std::array::from_fn(|axis| {
                bounds.min()[axis] + (bounds.max()[axis] - bounds.min()[axis]) * fraction
            })
        };
        let columns = vec![
            AttributeColumn::new(
                schema().remove(0),
                AttributeValues::U16(vec![1000 + i as u16, 2000]),
                2,
            )
            .unwrap(),
            AttributeColumn::new(
                schema().remove(1),
                AttributeValues::U8(vec![5, 6 + i as u8]),
                2,
            )
            .unwrap(),
            AttributeColumn::new(
                schema().remove(2),
                AttributeValues::U8(vec![2, 3 + i as u8]),
                2,
            )
            .unwrap(),
        ];
        blocks.push(SourcePointBlock::new(
            BlockId::new(i as u64 + 1),
            vec![at(0.25), at(0.75)],
            vec![ColorRgb::new(1, 2, 3); 2],
            columns,
        ));
    }
    let hierarchy = SpatialHierarchy::new(nodes, schema(), Some(6)).unwrap();
    let metadata = FormatMetadata::new(
        DatasetMetadata::new(
            CoordinateFrame::new(frame.frame_origin()).unwrap(),
            hierarchy,
        ),
        None,
        Some("sample-grid:256;leaf:64000".to_owned()),
        Vec::new(),
    )
    .unwrap()
    .with_global_frame(frame)
    .unwrap()
    .with_point_distance_quantiles(quantiles());
    (metadata, blocks)
}

fn write_with(lod_columns: Option<Vec<u32>>) -> Result<(Vec<u8>, Vec<u8>), FormatError> {
    let (metadata, blocks) = dataset();
    let metadata = match lod_columns {
        Some(columns) => metadata.with_lod_attribute_columns(columns),
        None => metadata,
    };
    let mut writer = FormatWriter::new_split_for_version(
        V8_PRE1,
        Cursor::new(Vec::new()),
        Cursor::new(Vec::new()),
        metadata,
        FormatWriterOptions::new(0.01, 1 << 24).unwrap(),
    )?;
    for block in blocks {
        writer.write_block(block)?;
    }
    let (primary, attributes) = writer.finish_pair()?;
    Ok((primary.into_inner(), attributes.unwrap().into_inner()))
}

fn open(primary: Vec<u8>, attributes: Vec<u8>) -> FormatReader<Cursor<Vec<u8>>> {
    let mut reader = FormatReader::new(Cursor::new(primary)).unwrap();
    reader.attach_attributes(Cursor::new(attributes)).unwrap();
    reader
}

#[test]
fn lod_blocks_omit_columns_outside_the_lod_set() {
    // The LOD set keeps intensity (0) and classification (2); scan_angle (1) is dropped from the LOD block.
    let (primary, attributes) = write_with(Some(vec![0, 2])).unwrap();
    let mut reader = open(primary.clone(), attributes);
    let lod = BlockId::new(0);
    assert!(reader.attribute_present(lod, 0).unwrap());
    assert!(!reader.attribute_present(lod, 1).unwrap());
    assert!(reader.attribute_present(lod, 2).unwrap());
    let kept = reader.read_attribute(lod, 2).unwrap();
    assert_eq!(kept.values(), &AttributeValues::U8(vec![2, 3]));
    // Leaves keep every column.
    for leaf in [BlockId::new(1), BlockId::new(2)] {
        for column in 0..3 {
            assert!(reader.attribute_present(leaf, column).unwrap());
            reader.read_attribute(leaf, column).unwrap();
        }
    }
    // The set is in the header, whatever the reader.
    let paged = PagedReader::open(Cursor::new(primary.clone())).unwrap();
    assert_eq!(paged.header().lod_attribute_columns(), Some(&[0, 2][..]));
    assert!(
        viewer_format::verify_paged(&mut Cursor::new(primary), &FormatReaderLimits::default())
            .is_ok()
    );
}

#[test]
fn an_empty_lod_set_keeps_only_geometry_and_display_colour() {
    let (primary, attributes) = write_with(Some(Vec::new())).unwrap();
    let mut reader = open(primary, attributes);
    for column in 0..3 {
        assert!(!reader.attribute_present(BlockId::new(0), column).unwrap());
        assert!(reader.attribute_present(BlockId::new(1), column).unwrap());
    }
    assert_eq!(
        reader.read_display_colors(BlockId::new(0)).unwrap().len(),
        2
    );
}

#[test]
fn lod_blocks_shrink_the_file() {
    let all = write_with(Some(vec![0, 1, 2])).unwrap();
    let some = write_with(Some(vec![0])).unwrap();
    let stats_all = payload_stats(&all);
    let stats_some = payload_stats(&some);
    // The only difference is the omitted columns (1 and 2) of the LOD block (0).
    let omitted: u64 = stats_all
        .iter()
        .filter(|(id, kind, _)| {
            *id == BlockId::new(0)
                && matches!(kind, BlockPayloadKind::Attribute { schema_index, .. } if *schema_index != 0)
        })
        .map(|(_, _, stored)| stored)
        .sum();
    assert!(omitted > 0);
    assert_eq!(all.1.len() - some.1.len(), omitted as usize);
    // Leaf blocks keep byte-identical payload entries and attribute values.
    for leaf in [BlockId::new(1), BlockId::new(2)] {
        let of = |stats: &[(BlockId, BlockPayloadKind, u64)]| {
            stats
                .iter()
                .filter(|(id, _, _)| *id == leaf)
                .cloned()
                .collect::<Vec<_>>()
        };
        assert_eq!(of(&stats_all), of(&stats_some));
    }
}

fn payload_stats(file: &(Vec<u8>, Vec<u8>)) -> Vec<(BlockId, BlockPayloadKind, u64)> {
    open(file.0.clone(), file.1.clone())
        .block_payload_storage_stats()
        .unwrap()
        .into_iter()
        .map(|stat| (stat.id, stat.kind, stat.stored_bytes))
        .collect()
}

#[test]
fn lod_blocks_carry_no_range_for_omitted_columns() {
    let all = open_pair(write_with(Some(vec![0, 1, 2])).unwrap());
    let some = open_pair(write_with(Some(vec![0, 2])).unwrap());
    let columns_of = |reader: &FormatReader<Cursor<Vec<u8>>>, node: u64| -> Vec<u32> {
        reader
            .format_metadata()
            .node_attribute_ranges(NodeId::new(node))
            .iter()
            .map(NodeAttributeRange::schema_index)
            .collect()
    };
    // Column 1 (scan_angle) is omitted on the LOD node 0 and present on its leaves.
    assert_eq!(columns_of(&all, 0), [0, 1, 2]);
    assert_eq!(columns_of(&some, 0), [0, 2]);
    for leaf in [1, 2] {
        assert_eq!(columns_of(&some, leaf), [0, 1, 2]);
    }
    // Leaves cover the LOD values, so the file-wide ranges do not change.
    for column in 0..3 {
        assert_eq!(
            some.attribute_range(column, 0).unwrap(),
            all.attribute_range(column, 0).unwrap(),
            "column {column}"
        );
    }
}

fn open_pair(file: (Vec<u8>, Vec<u8>)) -> FormatReader<Cursor<Vec<u8>>> {
    open(file.0, file.1)
}

#[test]
fn reading_an_absent_column_is_an_error_not_a_panic() {
    let (primary, attributes) = write_with(Some(vec![0])).unwrap();
    let mut reader = open(primary, attributes);
    for column in [1, 2] {
        assert!(matches!(
            reader.read_attribute(BlockId::new(0), column),
            Err(FormatError::AttributeAbsent { attribute, .. }) if attribute == column
        ));
        assert!(matches!(
            reader.read_attribute_row(BlockId::new(0), 0, column),
            Err(FormatError::AttributeAbsent { .. })
        ));
    }
    // A column outside the schema is still an unknown attribute.
    assert!(matches!(
        reader.read_attribute(BlockId::new(0), 3),
        Err(FormatError::UnknownAttribute(3))
    ));
}

#[test]
fn the_writer_rejects_a_corrupt_or_missing_lod_set() {
    for bad in [vec![3], vec![0, 0], vec![2, 0], vec![0, 1, 2, 3]] {
        let result = write_with(Some(bad.clone()));
        assert!(
            matches!(result, Err(FormatError::InvalidMetadata(_))),
            "{bad:?}: {result:?}"
        );
    }
    assert!(matches!(
        write_with(None),
        Err(FormatError::InvalidMetadata(_))
    ));
}

#[test]
fn revisions_without_a_lod_set_reject_one() {
    let (metadata, _) = dataset();
    let metadata = metadata.with_lod_attribute_columns(vec![0]);
    let result = FormatWriter::new_split_for_version(
        FormatVersion::V7,
        Cursor::new(Vec::new()),
        Cursor::new(Vec::new()),
        metadata,
        FormatWriterOptions::new(0.01, 1 << 24).unwrap(),
    );
    assert!(matches!(result, Err(FormatError::InvalidMetadata(_))));
}

/// Offset of the LOD list in the index: it follows the three f64 quantiles.
fn lod_list_offset(bytes: &[u8]) -> usize {
    let footer = bytes.len() - 40;
    let index = u64::from_le_bytes(bytes[footer + 8..footer + 16].try_into().unwrap()) as usize;
    let length = u64::from_le_bytes(bytes[footer + 16..footer + 24].try_into().unwrap()) as usize;
    let reader = PagedReader::open(Cursor::new(bytes.to_vec())).unwrap();
    let spacing = reader.header().reference_spacing().to_le_bytes();
    bytes[index..index + length]
        .windows(8)
        .position(|window| window == spacing)
        .unwrap()
        + index
        + 8
        + 24
}

#[test]
fn corrupt_lod_set_is_rejected() {
    let primary = write_with(Some(vec![0, 2])).unwrap().0;
    let at = lod_list_offset(&primary);
    assert_eq!(&primary[at..at + 4], &2_u32.to_le_bytes());
    let patches: [(&str, Vec<u8>); 5] = [
        (
            "index outside the schema",
            [2_u32, 0, 3].iter().flat_map(|v| v.to_le_bytes()).collect(),
        ),
        (
            "repeated index",
            [2_u32, 2, 2].iter().flat_map(|v| v.to_le_bytes()).collect(),
        ),
        (
            "decreasing indices",
            [2_u32, 2, 0].iter().flat_map(|v| v.to_le_bytes()).collect(),
        ),
        (
            "more entries than the schema",
            [4_u32, 0, 1].iter().flat_map(|v| v.to_le_bytes()).collect(),
        ),
        (
            "count beyond the index",
            [u32::MAX, 0, 2]
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect(),
        ),
    ];
    for (name, patch) in patches {
        let mut bytes = primary.clone();
        bytes[at..at + patch.len()].copy_from_slice(&patch);
        repair_index_and_footer(&mut bytes);
        assert!(
            matches!(
                PagedReader::open(Cursor::new(bytes.clone())),
                Err(FormatError::MalformedIndex(_))
            ),
            "{name}"
        );
        assert!(FormatReader::new(Cursor::new(bytes)).is_err(), "{name}");
    }
    // A file cut short anywhere is an error and never a panic.
    for cut in (0..primary.len()).step_by(53) {
        assert!(
            PagedReader::open(Cursor::new(primary[..cut].to_vec())).is_err(),
            "{cut}"
        );
    }
}
