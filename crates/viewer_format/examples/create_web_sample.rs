use std::{env, error::Error, fs::File, path::PathBuf};

use viewer_core::{
    AttributeColumn, AttributeDataType, AttributeDescriptor, AttributeValues, BlockId, ColorRgb,
    CoordinateFrame, DatasetMetadata, HierarchySemantics, NodeId, SpatialBounds, SpatialHierarchy,
    SpatialNode,
};
use viewer_format::{
    FormatMetadata, FormatVersion, FormatWriter, FormatWriterOptions, SourcePointBlock,
};

fn main() -> Result<(), Box<dyn Error>> {
    let output = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("examples/web-host/sample.oct"));
    let version = env::args()
        .nth(2)
        .map(|value| value.parse::<FormatVersion>())
        .transpose()?
        .unwrap_or(FormatVersion::V5);
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let block_id = BlockId::new(1);
    let node_id = NodeId::new(1);
    let point_count = 64 * 64;
    let intensity = AttributeDescriptor::new("intensity", AttributeDataType::U16, 1)?;
    let classification = AttributeDescriptor::new("classification", AttributeDataType::U8, 1)?;
    let hierarchy = SpatialHierarchy::new_with_semantics(
        vec![SpatialNode::new(
            node_id,
            None,
            0,
            SpatialBounds::new([-1.0, -1.0, -1.0], [64.0, 64.0, 5.0])?,
            point_count,
            1.0,
            vec![block_id],
        )?],
        vec![intensity.clone(), classification.clone()],
        Some(point_count),
        HierarchySemantics::Replacement,
    )?;
    let metadata = FormatMetadata::new(
        DatasetMetadata::new(CoordinateFrame::new([0.0; 3])?, hierarchy),
        Some("LOCAL_SYNTHETIC".to_owned()),
        Some("deterministic 64x64 wave grid".to_owned()),
        Vec::new(),
    )?;
    let mut positions = Vec::with_capacity(point_count as usize);
    let mut colors = Vec::with_capacity(point_count as usize);
    let mut intensities = Vec::with_capacity(point_count as usize);
    let mut classifications = Vec::with_capacity(point_count as usize);
    for y in 0..64 {
        for x in 0..64 {
            let x_phase = f64::from(x) * 0.19;
            let y_phase = f64::from(y) * 0.16;
            #[expect(
                clippy::disallowed_methods,
                reason = "demo-surface generator, not the conversion output path"
            )]
            let z = (x_phase.sin() * y_phase.cos() * 2.0) + 2.0;
            positions.push([f64::from(x), f64::from(y), z]);
            intensities.push((x * 1024 + y * 8) as u16);
            classifications.push(((x / 16 + y / 16) % 4) as u8);
            colors.push(ColorRgb::new(
                40 + x as u8 * 3,
                45 + y as u8 * 3,
                (120.0 + z * 25.0) as u8,
            ));
        }
    }

    let mut writer = FormatWriter::new_split_for_version(
        version,
        File::create(&output)?,
        File::create(output.with_extension("octattrs"))?,
        metadata,
        FormatWriterOptions::new(0.01, 1024 * 1024)?,
    )?;
    writer.write_block(SourcePointBlock::new(
        block_id,
        positions,
        colors,
        vec![
            AttributeColumn::new(
                intensity,
                AttributeValues::U16(intensities),
                point_count as usize,
            )?,
            AttributeColumn::new(
                classification,
                AttributeValues::U8(classifications),
                point_count as usize,
            )?,
        ],
    ))?;
    writer.finish()?;
    Ok(())
}
