//! V8 fixtures for tests that need a minimal valid `v8` dataset: one root node (so every column is
//! stored in full) holding every point of the block. Include with
//! `#[path = "common/v8_fixture.rs"] mod v8_fixture;` next to `mod common;`.
#![allow(
    dead_code,
    reason = "each test binary uses a different subset of these helpers"
)]

use std::io::Cursor;

use viewer_core::{AttributeDescriptor, BlockId};
use viewer_format::{
    CrsRecord, DisplayColourSource, FormatMetadata, FormatVersion, FormatWriter,
    FormatWriterOptions, PointDistanceQuantiles, SourcePointBlock,
};

use crate::common::{COLORS, POINTS, fixture_attributes, global_metadata, prefix_of};

/// The id every `v8` file gives its first block: the file numbers blocks from 0 in node order.
pub const V8_BLOCK: BlockId = BlockId::new(0);

pub const V8_LEVEL: u32 = 48;

pub fn v8_crs() -> CrsRecord {
    CrsRecord::new(
        "EPSG".to_owned(),
        32632,
        viewer_format::CrsRaw::Wkt("PROJCS[\"WGS 84 / UTM zone 32N\"]".to_owned()),
    )
    .unwrap()
}

pub fn v8_options() -> FormatWriterOptions {
    FormatWriterOptions::new(0.001, 1 << 24).unwrap()
}

/// Single-node `v8` metadata; the root cell contains `anchor` on every axis.
pub fn v8_metadata_at(
    anchor: f64,
    blocks: Vec<BlockId>,
    point_count: u64,
    schema: Vec<AttributeDescriptor>,
) -> FormatMetadata {
    let prefix = [prefix_of(anchor, V8_LEVEL); 3];
    global_metadata(
        V8_LEVEL,
        prefix,
        blocks,
        point_count,
        schema,
        v8_crs(),
        None,
    )
    .with_point_distance_quantiles(PointDistanceQuantiles::new(0.25, 0.5, 1.5).unwrap())
    .with_lod_attribute_columns(Vec::new())
    .with_display_colour_source(DisplayColourSource::None)
}

pub fn v8_metadata(point_count: u64, schema: Vec<AttributeDescriptor>) -> FormatMetadata {
    v8_metadata_at(0.5, vec![V8_BLOCK], point_count, schema)
}

pub fn write_v8_split(block: SourcePointBlock, meta: FormatMetadata) -> (Vec<u8>, Vec<u8>) {
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

/// The shared four-point fixture with ten typed attribute columns, as block `V8_BLOCK`.
pub fn v8_fixture_block() -> SourcePointBlock {
    SourcePointBlock::new(
        V8_BLOCK,
        POINTS.to_vec(),
        COLORS.to_vec(),
        fixture_attributes(),
    )
}
