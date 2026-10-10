//! Isolates integer cell addressing; excludes floating-point point classification and I/O.
use std::{hint::black_box, time::Instant};

#[inline(never)]
fn arithmetic(index: usize, divisions: usize) -> [usize; 3] {
    [
        index / (divisions * divisions),
        (index / divisions) % divisions,
        index % divisions,
    ]
}
#[inline(never)]
fn bit_fields(index: usize, bits: u32) -> [usize; 3] {
    let mask = (1 << bits) - 1;
    [index >> (2 * bits), (index >> bits) & mask, index & mask]
}
fn measure(f: impl Fn(usize) -> [usize; 3]) -> f64 {
    let started = Instant::now();
    for i in 0..2_000_000 {
        black_box(f(black_box(i & 511)));
    }
    started.elapsed().as_secs_f64() * 1000.0
}
fn main() {
    for d in [2_usize, 4, 8] {
        let bits = d.trailing_zeros();
        for i in 0..d * d * d {
            assert_eq!(arithmetic(i, d), bit_fields(i, bits));
        }
        let mut old = Vec::new();
        let mut new = Vec::new();
        for pass in 0..9 {
            let mut run = |which| {
                if which {
                    old.push(measure(|i| arithmetic(i, black_box(d))));
                } else {
                    new.push(measure(|i| bit_fields(i, black_box(bits))));
                }
            };
            run(pass % 2 == 0);
            run(pass % 2 != 0);
        }
        old.sort_by(f64::total_cmp);
        new.sort_by(f64::total_cmp);
        println!(
            "divisions={d} arithmetic_median_ms={:.3} bits_median_ms={:.3} ratio={:.3}",
            old[4],
            new[4],
            old[4] / new[4]
        );
    }
}
