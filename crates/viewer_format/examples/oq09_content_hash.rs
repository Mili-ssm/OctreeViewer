//! OQ-09 experiment: prints one row per block with an order-independent digest of every point's
//! position, colour and attribute values. Two files hold the same content when the outputs match.
//! `cargo run --profile perf -p viewer_format --example oq09_content_hash -- FILE.oct > out.tsv`
//! (the `.octattrs` sidecar must sit next to the real file).
use std::fs::File;

use viewer_core::{AttributeValues, DatasetSource};
use viewer_format::FormatReader;

fn mix(mut x: u64) -> u64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

fn value_bits(values: &AttributeValues, i: usize) -> u64 {
    match values {
        AttributeValues::U8(v) => v[i].into(),
        AttributeValues::I8(v) => v[i] as u8 as u64,
        AttributeValues::U16(v) => v[i].into(),
        AttributeValues::I16(v) => v[i] as u16 as u64,
        AttributeValues::U32(v) => v[i].into(),
        AttributeValues::I32(v) => v[i] as u32 as u64,
        AttributeValues::U64(v) => v[i],
        AttributeValues::I64(v) => v[i] as u64,
        AttributeValues::F32(v) => v[i].to_bits().into(),
        AttributeValues::F64(v) => v[i].to_bits(),
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).ok_or("usage: FILE.oct")?;
    let mut reader = FormatReader::new(File::open(&path)?)?;
    reader.attach_attributes(File::open(path.replace(".oct", ".octattrs"))?)?;
    let mut ids: Vec<_> = reader.block_storage_stats()?.iter().map(|b| b.id).collect();
    ids.sort_by_key(|id| id.get());
    for id in ids {
        let block = reader.read_block(id)?;
        let points = block.points().points();
        let (mut sum, mut xor) = (0u64, 0u64);
        let mut parts = [0u64; 2]; // positions, colours alone
        let mut per_column = vec![0u64; block.attributes().len()];
        for (i, p) in points.iter().enumerate() {
            let mut h = 0x9e37_79b9_7f4a_7c15u64;
            for c in p.position {
                h = mix(h ^ u64::from(c.to_bits()));
            }
            parts[0] = parts[0].wrapping_add(h);
            h = mix(h ^ u64::from_le_bytes([p.color.r, p.color.g, p.color.b, 0, 0, 0, 0, 0]));
            parts[1] = parts[1].wrapping_add(h);
            for (k, column) in block.attributes().iter().enumerate() {
                // Tuple columns are interleaved: hash every component of row `i`.
                let width = column.values().len() / points.len();
                let bits = (0..width).fold(0u64, |acc, j| {
                    mix(acc ^ value_bits(column.values(), i * width + j))
                });
                per_column[k] = per_column[k].wrapping_add(mix(bits));
                h = mix(h ^ bits);
            }
            sum = sum.wrapping_add(h);
            xor ^= mix(h);
        }
        println!(
            "{}\t{}\t{sum:016x}\t{xor:016x}\t{:016x}\t{:016x}\t{}",
            id.get(),
            points.len(),
            parts[0],
            parts[1],
            per_column
                .iter()
                .map(|c| format!("{c:016x}"))
                .collect::<Vec<_>>()
                .join(",")
        );
    }
    Ok(())
}
