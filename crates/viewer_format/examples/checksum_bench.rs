//! Portable IEEE CRC throughput; old bytewise recurrence is the independent control.
use std::{hint::black_box, time::Instant};
use viewer_format::crc32;

const TABLE: [u32; 256] = {
    let mut table = [0; 256];
    let mut index = 0;
    while index < 256 {
        let mut value = index as u32;
        let mut bit = 0;
        while bit < 8 {
            value = (value >> 1) ^ (0xEDB8_8320 & 0_u32.wrapping_sub(value & 1));
            bit += 1;
        }
        table[index] = value;
        index += 1;
    }
    table
};

fn bytewise(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for &byte in bytes {
        crc = TABLE[(crc as u8 ^ byte) as usize] ^ (crc >> 8);
    }
    crc ^ u32::MAX
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() > 2 {
        return Err("usage: checksum_bench [MIB [REPETITIONS]]".into());
    }
    let mib = args
        .first()
        .map(|v| v.parse::<usize>())
        .transpose()?
        .unwrap_or(64);
    let repetitions = args
        .get(1)
        .map(|v| v.parse::<usize>())
        .transpose()?
        .unwrap_or(9);
    if mib == 0 || repetitions == 0 {
        return Err("positive payload size and repetitions required".into());
    }
    let length = mib
        .checked_mul(1024 * 1024)
        .ok_or("payload size overflow")?;
    let bytes: Vec<_> = (0..length)
        .map(|i| (i ^ (i >> 8) ^ (i >> 16)) as u8)
        .collect();
    let expected = bytewise(&bytes);
    for repetition in 1..=repetitions {
        for optimized in if repetition % 2 == 1 {
            [false, true]
        } else {
            [true, false]
        } {
            let started = Instant::now();
            let checksum = if optimized {
                crc32(black_box(&bytes))
            } else {
                bytewise(black_box(&bytes))
            };
            black_box(checksum);
            let elapsed = started.elapsed().as_secs_f64();
            if checksum != expected {
                return Err("checksum mismatch".into());
            }
            println!(
                "rep={repetition} variant={} bytes={length} elapsed_ms={:.3} mib_per_second={:.3} checksum={checksum:08x}",
                if optimized { "slicing8" } else { "bytewise" },
                elapsed * 1000.0,
                mib as f64 / elapsed
            );
        }
    }
    Ok(())
}
