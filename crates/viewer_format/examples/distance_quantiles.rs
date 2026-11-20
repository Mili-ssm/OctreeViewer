//! Prints the reference spacing and the stored nearest-neighbour distance quantiles of paged files:
//! `cargo run --release -p viewer_format --example distance_quantiles -- FILE.oct...`

use viewer_format::{FormatReader, FormatReaderLimits};

fn main() {
    for path in std::env::args().skip(1) {
        let file = std::fs::File::open(&path).expect("open file");
        let reader =
            FormatReader::open_lazy(file, FormatReaderLimits::default()).expect("lazy open");
        let spacing = reader
            .reference_spacing()
            .expect("paged files store the spacing");
        match reader.point_distance_quantiles() {
            Some(q) => println!(
                "{path}: reference_spacing={spacing} p1={} p5={} p50={}",
                q.p1, q.p5, q.p50
            ),
            None => println!("{path}: reference_spacing={spacing} quantiles=none"),
        }
    }
}
