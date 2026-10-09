use std::{env, error::Error, fs::File, path::PathBuf};

use viewer_core::{
    BlockId, ColorRgb, CoordinateFrame, DatasetMetadata, HierarchySemantics, NodeId, SpatialBounds,
    SpatialHierarchy, SpatialNode,
};
use viewer_format::{FormatMetadata, FormatWriter, FormatWriterOptions, SourcePointBlock};

fn main() -> Result<(), Box<dyn Error>> {
    let output = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("examples/web-host/sample.oct"));
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let block_id = BlockId::new(1);
    let node_id = NodeId::new(1);
    let point_count = 64 * 64;
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
        Vec::new(),
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
    for y in 0..64 {
        for x in 0..64 {
            let x_phase = f64::from(x) * 0.19;
            let y_phase = f64::from(y) * 0.16;
            let z = (x_phase.sin() * y_phase.cos() * 2.0) + 2.0;
            positions.push([f64::from(x), f64::from(y), z]);
            colors.push(ColorRgb::new(
                40 + x as u8 * 3,
                45 + y as u8 * 3,
                (120.0 + z * 25.0) as u8,
            ));
        }
    }

    let mut writer = FormatWriter::new(
        File::create(output)?,
        metadata,
        FormatWriterOptions::new(0.01, 1024 * 1024)?,
    )?;
    writer.write_block(SourcePointBlock::new(
        block_id,
        positions,
        colors,
        Vec::new(),
    ))?;
    writer.finish()?;
    Ok(())
}
