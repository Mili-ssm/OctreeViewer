//! OQ-09 experiment: decodes every block (geometry, colours, attributes) of a v7.5 file and
//! prints per-run times plus an order-independent per-block hash (set equality check).
//! `cargo run --profile perf -p viewer_format --example oq09_decode_bench -- FILE.oct [HASHES_OUT]`
use std::{fs::File, io::Write, time::Instant};

use viewer_core::AttributeValues;
use viewer_format::FormatReader;

fn mix(mut h: u64, v: u64) -> u64 {
    h ^= v.wrapping_add(0x9e37_79b9_7f4a_7c15).wrapping_add(h << 6).wrapping_add(h >> 2);
    h.wrapping_mul(0xff51_afd7_ed55_8ccd)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args.next().ok_or("usage: FILE.oct [HASHES_OUT]")?;
    let hashes_out = args.next();
    let mut reader = FormatReader::new(File::open(&path)?)?;
    reader.attach_attributes(File::open(path.replace(".oct", ".octattrs"))?)?;
    let mut ids: Vec<_> = reader.block_storage_stats()?.into_iter().map(|b| b.id).collect();
    ids.sort();
    let schema = reader.format_metadata().dataset().hierarchy().attributes().len() as u32;
    let mut geom_ms: Vec<f64> = Vec::new();
    let mut attr_ms = Vec::new();
    let mut lines = Vec::new();
    // Pass 1 (warm-up): per-block order-independent hash, one block at a time.
    for &id in &ids {
        let batch = reader.read_geometry(id)?;
        let mut hashes: Vec<u64> = batch
            .points()
            .iter()
            .map(|p| {
                let mut h = 1;
                for v in p.position {
                    h = mix(h, u64::from(v.to_bits()));
                }
                mix(mix(mix(h, u64::from(p.color.r)), u64::from(p.color.g)), u64::from(p.color.b))
            })
            .collect();
        for s in 0..schema {
            let column = reader.read_attribute(id, s)?;
            let cc = column.descriptor().component_count() as usize;
            macro_rules! feed {
                ($v:expr, $f:expr) => {
                    for (row, h) in hashes.iter_mut().enumerate() {
                        for c in 0..cc {
                            *h = mix(*h, $f($v[row * cc + c]));
                        }
                    }
                };
            }
            match column.values() {
                AttributeValues::U8(v) => feed!(v, u64::from),
                AttributeValues::I8(v) => feed!(v, |x: i8| x as u64),
                AttributeValues::U16(v) => feed!(v, u64::from),
                AttributeValues::I16(v) => feed!(v, |x: i16| x as u64),
                AttributeValues::U32(v) => feed!(v, u64::from),
                AttributeValues::I32(v) => feed!(v, |x: i32| x as u64),
                AttributeValues::U64(v) => feed!(v, |x: u64| x),
                AttributeValues::I64(v) => feed!(v, |x: i64| x as u64),
                AttributeValues::F32(v) => feed!(v, |x: f32| u64::from(x.to_bits())),
                AttributeValues::F64(v) => feed!(v, |x: f64| x.to_bits()),
            }
        }
        let sum = hashes.iter().fold(0_u64, |s, h| s.wrapping_add(*h));
        lines.push(format!("{} {} {sum:016x}", id.get(), hashes.len()));
    }
    for run in 1..=3 {
        let mut points = 0_usize;
        let started = Instant::now();
        for &id in &ids {
            points += reader.read_geometry(id)?.points().len();
        }
        let g = started.elapsed().as_secs_f64() * 1e3;
        let started = Instant::now();
        for &id in &ids {
            for s in 0..schema {
                std::hint::black_box(reader.read_attribute(id, s)?);
            }
        }
        let a = started.elapsed().as_secs_f64() * 1e3;
        geom_ms.push(g);
        attr_ms.push(a);
        println!("run {run}: geometry+colors {g:.0} ms, attributes {a:.0} ms, points {points}");
    }
    geom_ms.sort_by(f64::total_cmp);
    attr_ms.sort_by(f64::total_cmp);
    println!("median geometry+colors {:.0} ms, attributes {:.0} ms", geom_ms[1], attr_ms[1]);
    if let Some(out) = hashes_out {
        File::create(out)?.write_all(lines.join("\n").as_bytes())?;
    }
    Ok(())
}
