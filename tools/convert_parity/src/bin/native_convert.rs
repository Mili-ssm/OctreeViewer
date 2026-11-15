//! Native reference for the conversion parity check: `native_convert INPUT OUTPUT.oct
//! POINTS_PER_BLOCK DIVISIONS [FORMAT [ROW_ORDER]]` runs the `viewer_convert` CLI path, writes the `.oct` /
//! `.octattrs` pair and prints the block count.
use std::path::Path;

use viewer_convert::{ConvertOptions, convert_file};

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let [input, output, points_per_block, divisions, rest @ ..] = arguments.as_slice() else {
        eprintln!(
            "usage: native_convert INPUT OUTPUT.oct POINTS_PER_BLOCK DIVISIONS [v5|v7.4-pre|v7.5-pre|v7 [source|hybrid]]"
        );
        std::process::exit(2);
    };
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
    let report =
        convert_file(Path::new(input), Path::new(output), options).expect("native conversion");
    println!("{}", report.block_count);
}
