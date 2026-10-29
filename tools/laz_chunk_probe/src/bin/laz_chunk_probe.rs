//! Native side of the S2 measurement.
//!
//! `make-fixture <laz> <chunks> <out>` writes the fixture shared with `run-wasm.mjs`;
//! `bench <fixture> [iterations]` prints one JSON line with the same fields as the wasm runner.

use std::fs::File;
use std::io::Cursor;
use std::path::Path;
use std::time::Instant;

use laz_chunk_probe::{Fixture, OpenedLaz, RangeDecoder, decode_chunk_range, fnv1a};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["make-fixture", laz, chunks, out] => make_fixture(laz, chunks.parse().unwrap(), out),
        ["info", laz] => info(laz),
        ["make-synthetic", out] => {
            std::fs::write(
                out,
                laz_chunk_probe::synthetic_variable_chunk_laz().unwrap(),
            )
            .unwrap();
        }
        ["bench", fixture] => bench(fixture, 3),
        ["bench", fixture, iterations] => bench(fixture, iterations.parse().unwrap()),
        _ => {
            eprintln!(
                "usage: laz_chunk_probe info <laz> | make-synthetic <out.laz> | make-fixture <laz> <chunks> <out> | bench <fixture> [iterations]"
            );
            std::process::exit(2);
        }
    }
}

fn info(laz: &str) {
    let opened = OpenedLaz::open(Path::new(laz)).unwrap();
    let counts: Vec<u64> = opened
        .index
        .table
        .as_ref()
        .iter()
        .map(|entry| entry.point_count)
        .collect();
    println!(
        "chunks={} header_points={} variable={} record_size={} min={} max={} first8={:?} items={:?}",
        counts.len(),
        opened.header_points,
        opened.index.vlr.uses_variable_size_chunks(),
        opened.index.record_size(),
        counts.iter().min().unwrap(),
        counts.iter().max().unwrap(),
        &counts[..8.min(counts.len())],
        opened
            .index
            .vlr
            .items()
            .iter()
            .map(|item| item.item_type())
            .collect::<Vec<_>>(),
    );
}

fn make_fixture(laz: &str, chunks: usize, out: &str) {
    let opened = OpenedLaz::open(Path::new(laz)).unwrap();
    let mut file = File::open(laz).unwrap();
    let bytes = Fixture::encode(&mut file, &opened.index, chunks).unwrap();
    std::fs::write(out, bytes).unwrap();
}

fn millis(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1e3
}

fn bench(path: &str, iterations: usize) {
    let fixture = Fixture::parse(std::fs::read(path).unwrap()).unwrap();
    let chunks = fixture.index.table.len();
    let points: u64 = fixture
        .index
        .table
        .as_ref()
        .iter()
        .map(|entry| entry.point_count)
        .sum();
    let (mut batch, mut each, mut setup) = (Vec::new(), Vec::new(), Vec::new());
    let (mut batch_hash, mut each_hash) = (0, 0);
    // Iteration 0 is a warm-up and is dropped.
    for iteration in 0..=iterations {
        let start = Instant::now();
        let mut out = Vec::new();
        decode_chunk_range(
            &mut Cursor::new(&fixture.data),
            &fixture.index,
            0..chunks,
            &mut out,
        )
        .unwrap();
        let time = millis(start);
        batch_hash = fnv1a(&out);

        let start = Instant::now();
        let mut out = Vec::new();
        for chunk in 0..chunks {
            decode_chunk_range(
                &mut Cursor::new(&fixture.data),
                &fixture.index,
                chunk..chunk + 1,
                &mut out,
            )
            .unwrap();
        }
        let each_time = millis(start);
        each_hash = fnv1a(&out);

        let start = Instant::now();
        for chunk in 0..chunks {
            RangeDecoder::new(
                &mut Cursor::new(&fixture.data),
                &fixture.index,
                chunk..chunk + 1,
            )
            .unwrap();
        }
        let setup_time = millis(start);
        if iteration > 0 {
            batch.push(time);
            each.push(each_time);
            setup.push(setup_time);
        }
    }
    let list = |values: &[f64]| format!("{values:?}");
    println!(
        "{{\"target\":\"native\",\"chunks\":{chunks},\"points\":{points},\"batch_ms\":{},\"each_ms\":{},\"setup_ms\":{},\"batch_hash\":\"{batch_hash:016x}\",\"each_hash\":\"{each_hash:016x}\"}}",
        list(&batch),
        list(&each),
        list(&setup)
    );
}
