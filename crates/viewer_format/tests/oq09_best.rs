//! OQ-09 experiment: a block written with `OQ09_MORTON=best` keeps the same (position, colour)
//! multiset as the source. The env var is read once per process, so this lives in its own binary.
mod common;

use std::io::Cursor;

use common::{BLOCK_ID, metadata, options};
use viewer_core::{ColorRgb, DatasetSource};
use viewer_format::{
    FormatReader, FormatVersion, FormatWriter, SourcePointBlock, quantize_local_positions,
};

fn lcg(state: &mut u32) -> f64 {
    *state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    f64::from(*state) / f64::from(u32::MAX)
}

#[test]
fn best_order_keeps_the_position_colour_multiset() {
    // SAFETY: single test in this binary, set before any worker reads it.
    unsafe { std::env::set_var("OQ09_MORTON", "best") };
    let mut state = 0x1234_5678_u32;
    for clustered in [false, true] {
        let count = 4096;
        let positions: Vec<[f64; 3]> = (0..count)
            .map(|_| {
                std::array::from_fn(|_| {
                    let r = lcg(&mut state);
                    if clustered {
                        (f64::from((r * 4.0) as u8) + lcg(&mut state) * 0.01) / 4.0
                    } else {
                        r
                    }
                })
            })
            .collect();
        let colors: Vec<_> = (0..count)
            .map(|i| ColorRgb::new(i as u8, (i >> 8) as u8, 23))
            .collect();
        let expected = quantize_local_positions(&positions, 0.001)
            .unwrap()
            .decode_local()
            .unwrap();
        let mut writer = FormatWriter::new_for_version(
            FormatVersion::V7_PRE1,
            Cursor::new(Vec::new()),
            metadata(count as u64, vec![]),
            options(),
        )
        .unwrap();
        writer
            .write_block(SourcePointBlock::new(
                BLOCK_ID,
                positions,
                colors.clone(),
                vec![],
            ))
            .unwrap();
        let mut reader = FormatReader::new(writer.finish().unwrap()).unwrap();
        let block = reader.read_block(BLOCK_ID).unwrap();
        let mut got: Vec<_> = block
            .points()
            .points()
            .iter()
            .map(|p| {
                (
                    p.position.map(f32::to_bits),
                    [p.color.r, p.color.g, p.color.b],
                )
            })
            .collect();
        let mut want: Vec<_> = expected
            .iter()
            .zip(&colors)
            .map(|(p, c)| (p.map(f32::to_bits), [c.r, c.g, c.b]))
            .collect();
        got.sort();
        want.sort();
        assert_eq!(got, want, "clustered={clustered}");
    }
}
