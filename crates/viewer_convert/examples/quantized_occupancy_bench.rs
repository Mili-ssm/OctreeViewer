//! Bounded experiment on a source prefix, before f32 projection; not a .oct storage benchmark.
use std::{error::Error, time::Instant};

use serde_json::json;
use viewer_format::{Crc32, QuantizedOccupancy, quantize_local_positions};

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let input = args.next().ok_or(
        "usage: quantized_occupancy_bench INPUT.las [ROWS [REPETITIONS [RESIDUAL_BITS_CSV]]]",
    )?;
    let rows: u64 = args.next().map(|v| v.parse()).transpose()?.unwrap_or(32768);
    let repetitions: usize = args.next().map(|v| v.parse()).transpose()?.unwrap_or(5);
    let bits: Vec<u8> = args
        .next()
        .unwrap_or_else(|| "16,24,28,32".into())
        .split(',')
        .map(str::parse)
        .collect::<Result<_, _>>()?;
    if args.next().is_some()
        || !(1..=65536).contains(&rows)
        || !(1..=100).contains(&repetitions)
        || bits.is_empty()
        || bits.iter().any(|&b| b > 32)
    {
        return Err(
            "rows must be 1..65536, repetitions 1..100 and bits 0..32; no extra arguments".into(),
        );
    }
    let mut reader = las::Reader::from_path(&input)?;
    let bounds = reader.header().bounds();
    let anchor = [bounds.min.x, bounds.min.y, bounds.min.z];
    let total_source_points = reader.header().number_of_points();
    let data = reader.read_points(rows)?;
    let positions = data
        .x()
        .zip(data.y())
        .zip(data.z())
        .map(|((x, y), z)| [x - anchor[0], y - anchor[1], z - anchor[2]])
        .collect::<Vec<_>>();
    if positions.is_empty() {
        return Err("source has no points".into());
    }
    let source = quantize_local_positions(&positions, 0.01)?;
    drop(positions);
    drop(data);
    drop(reader);
    let mut checksum = Crc32::new();
    for coordinate in source.coordinates() {
        for q in coordinate {
            checksum.update(&q.to_le_bytes());
        }
    }
    let checksum = format!("{:08x}", checksum.finish());
    for repetition in 1..=repetitions {
        let order: Vec<_> = if repetition % 2 == 0 {
            bits.iter().rev().copied().collect()
        } else {
            bits.clone()
        };
        for bit_count in order {
            let start = Instant::now();
            let candidate = QuantizedOccupancy::new(&source, bit_count)?;
            let build_ms = start.elapsed().as_secs_f64() * 1000.0;
            let start = Instant::now();
            let reconstructed = candidate.reconstruct();
            let reconstruct_ms = start.elapsed().as_secs_f64() * 1000.0;
            // Verify outside timing; no requantization from f32.
            if reconstructed != source {
                return Err("integer/transform/error roundtrip mismatch".into());
            }
            let topology = candidate.index().topology();
            println!(
                "{}",
                json!({
                    "rep": repetition, "input": input, "source_points": total_source_points,
                    "selection": "first_rows_in_source_order_not_spatial_lod_blocks", "rows": source.coordinates().len(),
                    "requested_rows": rows, "residual_bits": bit_count, "checksum_u32_crc32": checksum,
                    "quantization_origin": source.transform().origin(), "quantization_scale": source.transform().scale(),
                    "max_quantization_error": source.max_error(), "build_ms": build_ms, "reconstruct_ms": reconstruct_ms,
                    "leaves": candidate.index().leaf_count(), "nodes": topology.levels().iter().map(Vec::len).sum::<usize>(),
                    "residual_bytes": candidate.residual_bytes().len(), "canonical_xyz_bytes": source.coordinates().len() * 12,
                    "source_retained_estimate_bytes": source.estimated_memory_bytes(),
                    "candidate_retained_estimate_bytes": candidate.estimated_memory_bytes(),
                    "memory_basis": "inline_and_vector_capacities_excludes_allocator_source_build_peak_and_rss",
                    "roundtrip_exact": true, "storage_revision": "none_experimental_in_memory",
                })
            );
        }
    }
    Ok(())
}
