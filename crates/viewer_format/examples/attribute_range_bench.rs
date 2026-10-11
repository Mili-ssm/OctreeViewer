//! Repeated global-range queries over a real index; compare the same example across revisions.
use std::{fs::File, hint::black_box, time::Instant};

use viewer_format::{FormatError, FormatReader};

const SAMPLES: usize = 9;
const MAX_WORKING_SET: usize = 64;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.is_empty() || args.len() > 2 {
        return Err("usage: attribute_range_bench INPUT.oct [QUERIES_PER_SAMPLE=10000]".into());
    }
    let count = args
        .get(1)
        .map_or(Ok(10_000), |value| value.parse::<usize>())?;
    if !(1..=1_000_000).contains(&count) {
        return Err("queries per sample must be 1..1000000".into());
    }
    let reader = FormatReader::new(File::open(&args[0])?)?;
    let hierarchy = reader.format_metadata().dataset().hierarchy();
    let queries = hierarchy
        .attributes()
        .iter()
        .enumerate()
        .flat_map(|(schema_index, descriptor)| {
            (0..descriptor.component_count()).map(move |component| (schema_index as u32, component))
        })
        .take(MAX_WORKING_SET)
        .collect::<Vec<_>>();
    if queries.is_empty() {
        return Err("index has no attribute components to query".into());
    }

    // Includes each first query, then warms the entire working set before timed batches.
    let start = Instant::now();
    let expected = run(&reader, &queries, queries.len())?;
    let first_pass_us = start.elapsed().as_secs_f64() * 1_000_000.0;
    let mut samples = Vec::with_capacity(SAMPLES);
    let mut checksum = None;
    for _ in 0..SAMPLES {
        let start = Instant::now();
        let result = run(&reader, &queries, count)?;
        samples.push(start.elapsed().as_nanos() as f64 / count as f64);
        if let Some(checksum) = checksum {
            assert_eq!(result, checksum, "range results changed between samples");
        }
        checksum = Some(result);
    }
    samples.sort_unstable_by(f64::total_cmp);
    println!(
        "nodes={} components={} queries_per_sample={count} samples={SAMPLES} first_pass_us={first_pass_us:.3} ns_per_query_min={:.3} ns_per_query_median={:.3} ns_per_query_max={:.3} first_pass_checksum={expected} batch_checksum={}",
        hierarchy.stats().node_count,
        queries.len(),
        samples[0],
        samples[SAMPLES / 2],
        samples[SAMPLES - 1],
        checksum.unwrap()
    );
    Ok(())
}

fn run(
    reader: &FormatReader<File>,
    queries: &[(u32, u32)],
    count: usize,
) -> Result<u64, FormatError> {
    let mut checksum = 0_u64;
    for index in 0..count {
        let (schema_index, component) = queries[index % queries.len()];
        let range =
            black_box(reader.attribute_range(black_box(schema_index), black_box(component))?);
        let value = range.map_or(u64::MAX, |[low, high]| {
            low.to_bits() ^ high.to_bits().rotate_left(17)
        });
        checksum = checksum.wrapping_add(value);
    }
    Ok(black_box(checksum))
}
