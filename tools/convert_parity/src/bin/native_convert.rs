//! Native reference for the conversion parity check: `native_convert INPUT OUTPUT.oct
//! POINTS_PER_BLOCK DIVISIONS [FORMAT [ROW_ORDER]] [--attr-codec SCHEME] [--workers N]` runs the `viewer_convert` CLI path, writes the `.oct` /
//! `.octattrs` pair and prints the block count.
use std::path::Path;

use viewer_convert::{ConvertOptions, convert_file};

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let [input, output, points_per_block, divisions, tail @ ..] = arguments.as_slice() else {
        eprintln!(
            "usage: native_convert INPUT OUTPUT.oct POINTS_PER_BLOCK DIVISIONS [FORMAT [source|hybrid]] [--attr-codec lz4|cold|cold-rgb|all] [--workers N]"
        );
        std::process::exit(2);
    };
    let mut rest = Vec::new();
    let mut attr_codec = None;
    let mut workers = None;
    let mut tail = tail.iter();
    while let Some(argument) = tail.next() {
        match argument.as_str() {
            "--attr-codec" => attr_codec = tail.next(),
            "--workers" => workers = tail.next(),
            _ => rest.push(argument),
        }
    }
    let mut options =
        ConvertOptions::new(points_per_block.parse().expect("points per block"), 0.01)
            .and_then(|options| {
                options.with_spatial_divisions(divisions.parse().expect("divisions"))
            })
            .expect("valid options");
    if let Some(format) = rest.first() {
        options = options
            .with_format_version(format.parse().expect("format version"))
            .expect("format version options");
    }
    if let Some(order) = rest.get(1) {
        options = options
            .with_row_order(order.parse().expect("row order"))
            .expect("row order options");
    }
    if let Some(scheme) = attr_codec {
        options = options.with_attribute_codec(scheme.parse().expect("attribute codec"));
    }
    if let Some(workers) = workers {
        options = options
            .with_spatial_workers(workers.parse().expect("workers"))
            .expect("worker options");
    }
    let report =
        convert_file(Path::new(input), Path::new(output), options).expect("native conversion");
    println!("{}", report.block_count);
}
