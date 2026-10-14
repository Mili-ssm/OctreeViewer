//! Isolated exact byte-plane locality experiment; no production codec selection changes.
use std::{hint::black_box, time::Instant};
use viewer_format::crc32;

#[derive(Clone, Copy)]
enum Strategy {
    Column,
    Row,
    Tile,
}

impl Strategy {
    fn name(self) -> &'static str {
        match self {
            Self::Column => "column",
            Self::Row => "row",
            Self::Tile => "tile256",
        }
    }
}

#[inline]
fn copy_byte<const DECODE: bool>(
    input: &[u8],
    output: &mut [u8],
    width: usize,
    rows: usize,
    row: usize,
    byte: usize,
) {
    if DECODE {
        output[row * width + byte] = input[byte * rows + row];
    } else {
        output[byte * rows + row] = input[row * width + byte];
    }
}

fn transform<const DECODE: bool>(input: &[u8], width: usize, strategy: Strategy) -> Vec<u8> {
    let rows = input.len() / width;
    let mut output = vec![0; input.len()];
    match strategy {
        Strategy::Column => {
            for byte in 0..width {
                for row in 0..rows {
                    copy_byte::<DECODE>(input, &mut output, width, rows, row, byte);
                }
            }
        }
        Strategy::Row => {
            for row in 0..rows {
                for byte in 0..width {
                    copy_byte::<DECODE>(input, &mut output, width, rows, row, byte);
                }
            }
        }
        Strategy::Tile => {
            for start in (0..rows).step_by(256) {
                let end = (start + 256).min(rows);
                for byte in 0..width {
                    for row in start..end {
                        copy_byte::<DECODE>(input, &mut output, width, rows, row, byte);
                    }
                }
            }
        }
    }
    output
}

fn measure<const DECODE: bool>(input: &[u8], width: usize, repetitions: usize) {
    let expected = transform::<DECODE>(input, width, Strategy::Column);
    let strategies = [Strategy::Column, Strategy::Row, Strategy::Tile];
    for repetition in 0..repetitions {
        for offset in 0..3 {
            let strategy = strategies[(repetition + offset) % 3];
            let started = Instant::now();
            let output = transform::<DECODE>(black_box(input), width, strategy);
            black_box(&output);
            let elapsed = started.elapsed().as_secs_f64();
            assert_eq!(output, expected);
            println!(
                "direction={} strategy={} width={width} rows={} rep={} elapsed_ms={:.6} checksum={:08x}",
                if DECODE { "decode" } else { "encode" },
                strategy.name(),
                input.len() / width,
                repetition + 1,
                elapsed * 1000.0,
                crc32(&output)
            );
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() > 1 {
        return Err("usage: byte_plane_bench [REPETITIONS]".into());
    }
    let repetitions = args
        .first()
        .map(|v| v.parse::<usize>())
        .transpose()?
        .unwrap_or(9);
    if repetitions == 0 {
        return Err("positive repetitions required".into());
    }
    for rows in [262_144, 1_048_576] {
        for width in [3, 6, 8, 16] {
            let mut state = 123_u32;
            let raw: Vec<_> = (0..rows * width)
                .map(|_| {
                    state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                    (state >> 24) as u8
                })
                .collect();
            let planes = transform::<false>(&raw, width, Strategy::Column);
            assert_eq!(transform::<true>(&planes, width, Strategy::Column), raw);
            measure::<false>(&raw, width, repetitions);
            measure::<true>(&planes, width, repetitions);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strategies_preserve_layout_and_bits_at_tile_boundaries() {
        assert_eq!(
            transform::<false>(&[0, 1, 2, 3, 4, 5], 3, Strategy::Column),
            [0, 3, 1, 4, 2, 5]
        );
        for rows in [0, 1, 31, 255, 256, 257] {
            for width in [1, 3, 6, 8, 16, 257] {
                let raw: Vec<_> = (0..rows * width).map(|i| (i ^ (i >> 8)) as u8).collect();
                let expected = transform::<false>(&raw, width, Strategy::Column);
                for strategy in [Strategy::Column, Strategy::Row, Strategy::Tile] {
                    let planes = transform::<false>(&raw, width, strategy);
                    assert_eq!(planes, expected);
                    assert_eq!(transform::<true>(&planes, width, strategy), raw);
                }
            }
        }
    }
}
