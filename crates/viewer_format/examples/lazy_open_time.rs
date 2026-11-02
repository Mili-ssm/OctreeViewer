//! Times `FormatReader::open_lazy` (tail read plus root page) on `v7.5-pre` files:
//! `cargo run --release -p viewer_format --example lazy_open_time -- FILE.oct...`

use std::time::Instant;

use viewer_format::{FormatReader, FormatReaderLimits};

fn main() {
    for path in std::env::args().skip(1) {
        let mut times = Vec::new();
        for _ in 0..5 {
            let file = std::fs::File::open(&path).expect("open file");
            let started = Instant::now();
            let reader =
                FormatReader::open_lazy(file, FormatReaderLimits::default()).expect("lazy open");
            times.push(started.elapsed().as_secs_f64() * 1e3);
            drop(reader);
        }
        times.sort_by(f64::total_cmp);
        println!("{path}: median {:.3} ms, min {:.3} ms", times[2], times[0]);
    }
}
