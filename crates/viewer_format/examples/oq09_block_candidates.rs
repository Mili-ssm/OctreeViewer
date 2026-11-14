//! OQ-09 experiment: re-encodes chosen blocks of a v7.5 file and prints every geometry candidate
//! (raw and compressed bytes). Run with and without `OQ09_MORTON=1` to compare.
//! `cargo run --profile perf -p viewer_format --example oq09_block_candidates -- FILE.oct QERR ID...`
use std::fs::File;

use viewer_core::BlockId;
use viewer_format::{
    BlockEncoder, FormatBlockEncoder, FormatReader, FormatVersion, FormatWriterOptions,
    SourcePointBlock,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args.next().ok_or("usage: FILE.oct QERR ID...")?;
    let qerr: f64 = args.next().ok_or("QERR")?.parse()?;
    let ids: Vec<u64> = args.map(|a| a.parse().unwrap()).collect();
    let mut reader = FormatReader::new(File::open(&path)?)?;
    reader.attach_attributes(File::open(path.replace(".oct", ".octattrs"))?)?;
    let metadata = reader.format_metadata().clone();
    let schema = metadata.dataset().hierarchy().attributes().len() as u32;
    let options = FormatWriterOptions::new(qerr, 64 * 1024 * 1024)?;
    let encoder = FormatBlockEncoder::new(FormatVersion::V7_PRE5, &metadata, options)?;
    for id in ids {
        let id = BlockId::new(id);
        let batch = reader.read_positions(id)?;
        let colors = reader.read_display_colors(id)?;
        let positions = batch
            .points()
            .iter()
            .map(|p| p.position.map(f64::from))
            .collect();
        let mut attrs = Vec::new();
        for s in 0..schema {
            attrs.push(reader.read_attribute(id, s)?);
        }
        let n = batch.points().len();
        let block = SourcePointBlock::new(id, positions, colors, attrs);
        let (prepared, diag) = encoder.prepare_with_diagnostics(block)?;
        println!("block {} points {n}", id.get());
        let _ = &prepared;
        for c in diag.expect("diagnostics").candidates {
            println!(
                "  {:?} raw {} compressed {} winner {}",
                c.kind, c.raw_bytes, c.compressed_bytes, c.selected_winners
            );
        }
    }
    Ok(())
}
