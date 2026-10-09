use std::hint::black_box;
use std::time::{Duration, Instant};

use viewer_format::compress_block;

const POINTS: usize = 262_144;
const ITERATIONS: usize = 5;

fn main() {
    for (name, payload) in [
        ("structured", structured_payload()),
        ("high_entropy", high_entropy_payload()),
    ] {
        measure(name, &payload);
    }
}

fn structured_payload() -> Vec<u8> {
    let mut payload = Vec::with_capacity(POINTS * 17);
    for index in 0..POINTS {
        let x = (index % 1024) as u32;
        let y = (index / 1024) as u32;
        let z = (x * 13 + y * 7) % 4096;
        for coordinate in [x, y, z] {
            payload.extend_from_slice(&coordinate.to_le_bytes());
        }
        payload.extend_from_slice(&[96, 128, 144]);
        payload.extend_from_slice(&((index % 256) as u16).to_le_bytes());
    }
    payload
}

fn high_entropy_payload() -> Vec<u8> {
    let mut payload = Vec::with_capacity(POINTS * 17);
    let mut state = 0xA341_316C_u32;
    for _ in 0..POINTS {
        for _ in 0..3 {
            state = next_random(state);
            payload.extend_from_slice(&state.to_le_bytes());
        }
        for _ in 0..5 {
            state = next_random(state);
            payload.push(state as u8);
        }
    }
    payload
}

fn next_random(mut state: u32) -> u32 {
    state ^= state << 13;
    state ^= state >> 17;
    state ^= state << 5;
    state
}

fn measure(name: &str, payload: &[u8]) {
    let mut encoded_size = 0;
    let mut compression_time = Duration::ZERO;
    let mut decompression_time = Duration::ZERO;
    for _ in 0..ITERATIONS {
        let started = Instant::now();
        let encoded = compress_block(black_box(payload));
        compression_time += started.elapsed();
        encoded_size = encoded.bytes().len();

        let started = Instant::now();
        let decoded = match encoded.codec() {
            viewer_format::BlockCodec::None => encoded.bytes().to_vec(),
            viewer_format::BlockCodec::Lz4 => {
                lz4_flex::block::decompress(encoded.bytes(), payload.len()).unwrap()
            }
        };
        decompression_time += started.elapsed();
        assert_eq!(black_box(decoded), payload);
    }

    let ratio = encoded_size as f64 / payload.len() as f64;
    println!(
        "{name}: points={POINTS} raw_bytes={} stored_bytes={encoded_size} ratio={ratio:.3} encode_ms={:.2} decode_ms={:.2}",
        payload.len(),
        compression_time.as_secs_f64() * 1000.0 / ITERATIONS as f64,
        decompression_time.as_secs_f64() * 1000.0 / ITERATIONS as f64,
    );
}
