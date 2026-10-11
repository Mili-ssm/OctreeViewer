use std::hint::black_box;
use std::time::{Duration, Instant};

use viewer_core::{BlockId, HierarchySemantics, SpatialBounds, SpatialHierarchy, SpatialNode};

const SAMPLES: usize = 9;
const SIZES: [usize; 4] = [64, 512, 4_096, 16_384];
const QUERIES_PER_SAMPLE: usize = 256;

// Synthetic baseline: prebuilt per-node block vectors, not the exact former HashMap traversal.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bounds = SpatialBounds::new([0.0; 3], [1.0; 3])?;
    for size in SIZES {
        let node_ids = (0..size)
            .map(|index| viewer_core::NodeId::new(index as u64 + 1))
            .collect::<Vec<_>>();
        let nodes = (0..size)
            .map(|index| {
                SpatialNode::new(
                    node_ids[index],
                    (index != 0).then_some(node_ids[0]),
                    if index == 0 { 0 } else { 1 },
                    bounds,
                    0,
                    0.0,
                    vec![BlockId::new((index as u64).wrapping_mul(3).wrapping_add(1))],
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let baseline_nodes = nodes
            .iter()
            .map(|node| node.blocks().to_vec())
            .collect::<Vec<_>>();
        let build_start = Instant::now();
        let hierarchy = SpatialHierarchy::new_with_semantics(
            nodes,
            Vec::new(),
            None,
            HierarchySemantics::Replacement,
        )?;
        // Includes full hierarchy validation/build plus catalog construction.
        let build_time = build_start.elapsed();
        let queries = (0..QUERIES_PER_SAMPLE)
            .map(|index| {
                let block_index = (index * 127 + size / 2) % size;
                if index % 2 == 0 {
                    BlockId::new((block_index as u64).wrapping_mul(3).wrapping_add(1))
                } else {
                    BlockId::new(
                        (size as u64 + index as u64 + 1)
                            .wrapping_mul(3)
                            .wrapping_add(1),
                    )
                }
            })
            .collect::<Vec<_>>();
        for query in &queries {
            assert_eq!(
                scan(&baseline_nodes, std::slice::from_ref(query)),
                lookup(&hierarchy, std::slice::from_ref(query)),
                "baseline and catalog lookup differ for size {size}, query {query:?}"
            );
        }

        // Warm both routes before collecting paired samples.
        black_box(scan(&baseline_nodes, &queries));
        black_box(lookup(&hierarchy, &queries));

        // Baseline scans a prebuilt copy of each original node block vector.
        let mut scan_samples = Vec::with_capacity(SAMPLES);
        let mut lookup_samples = Vec::with_capacity(SAMPLES);
        for sample in 0..SAMPLES {
            if sample % 2 == 0 {
                scan_samples.push(time_once(|| scan(&baseline_nodes, &queries)));
                lookup_samples.push(time_once(|| lookup(&hierarchy, &queries)));
            } else {
                lookup_samples.push(time_once(|| lookup(&hierarchy, &queries)));
                scan_samples.push(time_once(|| scan(&baseline_nodes, &queries)));
            }
        }
        let scan_median = median(scan_samples);
        let lookup_median = median(lookup_samples);
        println!(
            "nodes={size} queries_per_sample={QUERIES_PER_SAMPLE} catalog_ids={} catalog_bytes={} hierarchy_and_catalog_build_us={} scan_batch_median_us={} lookup_batch_median_us={} scan_ns_per_query={} lookup_ns_per_query={} speedup={:.2}x",
            hierarchy.block_ids().len(),
            size_of_val(hierarchy.block_ids()),
            build_time.as_micros(),
            scan_median.as_micros(),
            lookup_median.as_micros(),
            scan_median.as_nanos() / QUERIES_PER_SAMPLE as u128,
            lookup_median.as_nanos() / QUERIES_PER_SAMPLE as u128,
            scan_median.as_secs_f64() / lookup_median.as_secs_f64()
        );
    }
    Ok(())
}

fn scan(nodes: &[Vec<BlockId>], queries: &[BlockId]) -> usize {
    black_box(queries).iter().fold(0, |matches, query| {
        matches + usize::from(black_box(nodes).iter().any(|blocks| blocks.contains(query)))
    })
}

fn lookup(hierarchy: &SpatialHierarchy, queries: &[BlockId]) -> usize {
    black_box(queries).iter().fold(0, |matches, query| {
        matches + usize::from(hierarchy.references_block(*black_box(query)))
    })
}

fn time_once(run: impl FnOnce() -> usize) -> Duration {
    let start = Instant::now();
    black_box(run());
    start.elapsed()
}

fn median(mut samples: Vec<Duration>) -> Duration {
    samples.sort_unstable();
    samples[samples.len() / 2]
}
