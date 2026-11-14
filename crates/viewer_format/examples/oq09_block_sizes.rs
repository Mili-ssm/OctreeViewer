//! OQ-09 experiment: prints one TSV row per block with its hierarchy level, point count,
//! geometry encoding and stored bytes per payload (geometry, colour, attributes).
//! `cargo run --profile perf -p viewer_format --example oq09_block_sizes -- FILE.oct > out.tsv`
use std::{collections::HashMap, fs::File};

use viewer_format::{BlockPayloadKind, FormatReader};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).ok_or("usage: FILE.oct")?;
    let reader = FormatReader::new(File::open(&path)?)?;
    let hierarchy = reader.format_metadata().dataset().hierarchy();
    let mut level_of = HashMap::new();
    let mut stack = vec![hierarchy.root_id()];
    while let Some(id) = stack.pop() {
        let node = hierarchy.node(id).ok_or("missing node")?;
        for b in node.blocks() {
            level_of.insert(b.get(), node.level());
        }
        stack.extend_from_slice(hierarchy.child_ids(id));
    }
    let mut rows: HashMap<u64, [u64; 3]> = HashMap::new();
    for p in reader.block_payload_storage_stats()? {
        let r = rows.entry(p.id.get()).or_default();
        match p.kind {
            BlockPayloadKind::Geometry => r[0] += p.stored_bytes,
            BlockPayloadKind::DisplayColor => r[1] += p.stored_bytes,
            BlockPayloadKind::Attribute { .. } => r[2] += p.stored_bytes,
        }
    }
    println!("level\tid\tpoints\tenc\tgeom\tcolor\tattr");
    for b in reader.block_storage_stats()? {
        let r = rows[&b.id.get()];
        let lvl = level_of.get(&b.id.get()).map_or(-1, |l| i64::from(*l));
        println!(
            "{lvl}\t{}\t{}\t{:?}\t{}\t{}\t{}",
            b.id.get(),
            b.point_count,
            b.geometry_encoding,
            r[0],
            r[1],
            r[2]
        );
    }
    Ok(())
}
