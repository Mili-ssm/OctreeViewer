//! V8 fixtures for unit tests: a single-node `v8` dataset whose root holds every point, so each
//! column is stored in full. Mirrors `tests/common/v8_fixture.rs`, which integration tests use.

use std::io::Cursor;

use viewer_core::{
    AttributeColumn, AttributeDataType, AttributeDescriptor, AttributeValues, BlockId, ColorRgb,
    CoordinateFrame, DatasetMetadata, NodeId, SpatialHierarchy, SpatialNode,
};

use crate::{
    CrsRecord, DisplayColourSource, FormatMetadata, FormatVersion, FormatWriter,
    FormatWriterOptions, GlobalFrame, NodeCell, PointDistanceQuantiles, SourcePointBlock,
};

/// The id every `v8` file gives its first block: the file numbers blocks from 0 in node order.
pub(crate) const V8_BLOCK: BlockId = BlockId::new(0);
pub(crate) const V8_LEVEL: u32 = 48;

pub(crate) const POINTS: [[f64; 3]; 4] = [
    [0.125, 0.25, 0.5],
    [0.25, 0.375, 0.625],
    [0.5, 0.5, 0.75],
    [0.875, 0.75, 0.125],
];
pub(crate) const COLORS: [ColorRgb; 4] = [
    ColorRgb::new(1, 2, 3),
    ColorRgb::new(40, 50, 60),
    ColorRgb::new(127, 128, 129),
    ColorRgb::new(253, 254, 255),
];

pub(crate) fn v8_options() -> FormatWriterOptions {
    FormatWriterOptions::new(0.001, 1 << 24).unwrap()
}

pub(crate) fn v8_crs() -> CrsRecord {
    CrsRecord::new(
        "EPSG".to_owned(),
        32632,
        crate::CrsRaw::Wkt("PROJCS[\"WGS 84 / UTM zone 32N\"]".to_owned()),
    )
    .unwrap()
}

/// Prefix of the node at absolute `level` that contains `coordinate` (CRS units), per axis.
pub(crate) fn prefix_of(coordinate: f64, level: u32) -> u64 {
    let quanta = crate::quantize_coordinate(coordinate).unwrap();
    let u = (quanta as u64) ^ (1 << 63);
    u.checked_shr(64 - level).unwrap_or(0)
}

/// Single-node `v8` metadata; the root cell contains the origin-side unit cube corner `anchor`.
pub(crate) fn v8_metadata_at(
    anchor: f64,
    blocks: Vec<BlockId>,
    point_count: u64,
    schema: Vec<AttributeDescriptor>,
) -> FormatMetadata {
    let prefix = [prefix_of(anchor, V8_LEVEL); 3];
    let origin = prefix.map(|p| crate::cell_origin_quanta(V8_LEVEL, p));
    let frame = GlobalFrame::new(
        V8_LEVEL,
        prefix,
        origin,
        origin,
        v8_crs(),
        None,
        vec![(NodeId::new(0), NodeCell::new(prefix, 2.0_f64.powi(-5)))],
    )
    .unwrap();
    let bounds = frame.node_bounds(0, prefix).unwrap();
    let node = SpatialNode::new(NodeId::new(0), None, 0, bounds, point_count, 0.0, blocks).unwrap();
    let hierarchy = SpatialHierarchy::new(vec![node], schema, Some(point_count)).unwrap();
    FormatMetadata::new(
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
    .with_point_distance_quantiles(PointDistanceQuantiles::new(0.25, 0.5, 1.5).unwrap())
    .with_lod_attribute_columns(Vec::new())
    .with_display_colour_source(DisplayColourSource::None)
}

pub(crate) fn v8_metadata(point_count: u64, schema: Vec<AttributeDescriptor>) -> FormatMetadata {
    v8_metadata_at(0.5, vec![V8_BLOCK], point_count, schema)
}

pub(crate) fn write_v8_split(block: SourcePointBlock, meta: FormatMetadata) -> (Vec<u8>, Vec<u8>) {
    let mut writer = FormatWriter::new_split_for_version(
        FormatVersion::V8,
        Cursor::new(Vec::new()),
        Cursor::new(Vec::new()),
        meta,
        v8_options(),
    )
    .unwrap();
    writer.write_block(block).unwrap();
    let (primary, attributes) = writer.finish_pair().unwrap();
    (primary.into_inner(), attributes.unwrap().into_inner())
}

pub(crate) fn intensity() -> AttributeDescriptor {
    AttributeDescriptor::new("intensity", AttributeDataType::U16, 1).unwrap()
}

/// Four points with one `u16` intensity column, as block `V8_BLOCK`.
pub(crate) fn v8_block() -> SourcePointBlock {
    let column = AttributeColumn::new(
        intensity(),
        AttributeValues::U16(vec![0, 100, 200, 65535]),
        4,
    )
    .unwrap();
    SourcePointBlock::new(V8_BLOCK, POINTS.to_vec(), COLORS.to_vec(), vec![column])
}

/// The matching `(primary, attributes)` pair.
pub(crate) fn v8_pair() -> (Vec<u8>, Vec<u8>) {
    write_v8_split(v8_block(), v8_metadata(4, vec![intensity()]))
}
