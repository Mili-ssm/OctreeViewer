//! Deterministic local EPT corpus for Additive refinement diagnostics.
use std::{error::Error, fs, path::PathBuf};

use las::{Builder, Point, Writer, point::Format};
use serde_json::json;

const ROOT_SIDE: f64 = 8.0;
const POINTS_PER_NODE: usize = 8 * 8 * 8;

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args_os().skip(1);
    let output = args
        .next()
        .map(PathBuf::from)
        .filter(|_| args.next().is_none())
        .ok_or(
            "usage: cargo run -p viewer_convert --example generate_additive_fixture -- OUTPUT_DIR",
        )?;
    if output.exists() {
        return Err(format!("output path already exists: {}", output.display()).into());
    }

    let data_dir = output.join("ept-data");
    let hierarchy_dir = output.join("ept-hierarchy");
    fs::create_dir_all(&data_dir)?;
    fs::create_dir_all(&hierarchy_dir)?;

    let mut hierarchy = serde_json::Map::new();
    let mut node_count = 0_u64;
    for depth in 0..=2 {
        let divisions = 1_u32 << depth;
        for x in 0..divisions {
            for y in 0..divisions {
                for z in 0..divisions {
                    let key = format!("{depth}-{x}-{y}-{z}");
                    write_node(&data_dir.join(format!("{key}.laz")), depth, x, y, z)?;
                    hierarchy.insert(key, json!(POINTS_PER_NODE));
                    node_count += 1;
                }
            }
        }
    }

    let point_count = node_count * POINTS_PER_NODE as u64;
    let manifest = json!({
        "dataType": "laszip",
        "hierarchyType": "json",
        "points": point_count,
        "bounds": [0, 0, 0, ROOT_SIDE, ROOT_SIDE, ROOT_SIDE],
        "schema": [
            { "name": "X", "type": "signed", "size": 4 },
            { "name": "Y", "type": "signed", "size": 4 },
            { "name": "Z", "type": "signed", "size": 4 }
        ]
    });
    fs::write(
        output.join("ept.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    fs::write(
        hierarchy_dir.join("0-0-0-0.json"),
        serde_json::to_vec_pretty(&hierarchy)?,
    )?;

    println!(
        "Generated additive EPT fixture: {} nodes, {point_count} points at {}",
        node_count,
        output.display()
    );
    Ok(())
}

fn write_node(
    path: &std::path::Path,
    depth: u32,
    x: u32,
    y: u32,
    z: u32,
) -> Result<(), Box<dyn Error>> {
    let divisions = f64::from(1_u32 << depth);
    let cell_side = ROOT_SIDE / divisions;
    let offset = f64::from(depth) / 1000.0;
    let mut header_builder = Builder::from((1, 2));
    header_builder.point_format = Format::new(0)?;
    header_builder.point_format.is_compressed = true;
    let header = header_builder.into_header()?;
    let mut writer = Writer::from_path(path, header)?;

    for ix in 0..8 {
        for iy in 0..8 {
            for iz in 0..8 {
                let coordinate = |cell: u32, sample: u32| {
                    f64::from(cell) * cell_side
                        + (f64::from(sample) + 0.5) * cell_side / 8.0
                        + offset
                };
                writer.write_point(Point {
                    x: coordinate(x, ix),
                    y: coordinate(y, iy),
                    z: coordinate(z, iz),
                    ..Point::default()
                })?;
            }
        }
    }
    writer.close()?;
    Ok(())
}
