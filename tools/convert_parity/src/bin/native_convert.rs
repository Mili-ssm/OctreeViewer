//! Native reference for the conversion parity check: `native_convert INPUT OUTPUT.oct [--workers N]`
//! runs the `viewer_convert` CLI path (V8 with its fixed parameters), writes the `.oct` /
//! `.octattrs` pair and prints the block count.
use std::path::Path;

use viewer_convert::{ConvertOptions, convert_file};

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let [input, output, tail @ ..] = arguments.as_slice() else {
        eprintln!("usage: native_convert INPUT OUTPUT.oct [--workers N]");
        std::process::exit(2);
    };
    let mut options = ConvertOptions::default().validate().expect("valid options");
    match tail {
        [] => {}
        [flag, workers] if flag == "--workers" => {
            options = options
                .with_spatial_workers(workers.parse().expect("workers"))
                .expect("worker options");
        }
        _ => {
            eprintln!("usage: native_convert INPUT OUTPUT.oct [--workers N]");
            std::process::exit(2);
        }
    }
    let report =
        convert_file(Path::new(input), Path::new(output), options).expect("native conversion");
    println!("{}", report.block_count);
}
