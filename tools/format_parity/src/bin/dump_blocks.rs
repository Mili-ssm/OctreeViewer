//! Writes 4 planned source blocks of a real LAS/LAZ conversion (default CLI options) to a file in
//! the format documented on `format_parity::serialize_blocks`: 1 LOD block, then the 3 largest leaf
//! (source-point) blocks. The first planned blocks are small LOD samples (< 64 KiB of
//! geometry) where `lz4_flex`'s 32/64-bit hash tables agree, so they could not reveal a divergence;
//! hence the leaves must exceed `MIN_LEAF_POINTS` (the synthetic fixture's size, above 64 KiB).
//! Usage: `dump_blocks` INPUT.laz OUT.blocks
use std::{env, fs, path::PathBuf};

use viewer_convert::ConvertOptions;

const LEAVES: usize = 3;
const LODS: usize = 1;
const MIN_LEAF_POINTS: usize = 16_384;

fn main() {
    let mut args = env::args_os().skip(1);
    let input = PathBuf::from(args.next().expect("INPUT"));
    let output = PathBuf::from(args.next().expect("OUT"));
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).expect("create output directory");
    }
    // The planning workspace is created next to the output and removed on return.
    let mut blocks = viewer_convert::plan_sample_blocks(
        &input,
        &output,
        ConvertOptions::default(),
        LEAVES,
        LODS,
    )
    .expect("plan source blocks");
    assert_eq!(blocks.len(), LEAVES + LODS, "input plans too few blocks");
    for leaf in &blocks[..LEAVES] {
        let points = leaf.positions_local().len();
        assert!(
            points >= MIN_LEAF_POINTS,
            "leaf block {:?} has {points} points (< {MIN_LEAF_POINTS}); geometry stays below 64 KiB",
            leaf.id()
        );
    }
    // The planner returns leaves then LOD; the digest fixture wants the LOD (root) block first.
    blocks.rotate_right(LODS);
    let bytes = format_parity::serialize_blocks(&blocks);
    fs::write(&output, &bytes).expect("write dump");
    let counts = blocks
        .iter()
        .map(|block| block.positions_local().len())
        .collect::<Vec<_>>();
    eprintln!("wrote blocks with {counts:?} points, {} bytes", bytes.len());
}
