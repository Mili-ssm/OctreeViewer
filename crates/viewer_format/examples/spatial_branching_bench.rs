use std::{collections::BTreeMap, time::Instant};

use viewer_format::compress_block;

const POINT_COUNT: usize = 100_000;
const LEAF_POINT_LIMIT: usize = 2_048;
const MAX_DEPTH: u32 = 24;
const BRANCH_FACTORS: [usize; 3] = [2, 4, 8];

#[derive(Clone, Copy)]
struct Point {
    position: [f64; 3],
    intensity: u16,
}

#[derive(Clone, Copy)]
struct Bounds {
    min: [f64; 3],
    max: [f64; 3],
}

#[derive(Default)]
struct Metrics {
    nodes: usize,
    leaves: Vec<(Bounds, usize)>,
    raw_bytes: usize,
    stored_bytes: usize,
    max_depth: u32,
}

fn main() {
    println!(
        "distribution,factor,nodes,leaves,max_depth,min_leaf_points,mean_leaf_points,max_leaf_points,raw_bytes,stored_bytes,lz4_ratio,query10_mean_candidate_ratio,query10_p95_candidate_ratio,build_and_encode_ms"
    );
    for distribution in ["uniform", "clustered", "plane", "line", "grid"] {
        let points = generate_points(distribution);
        for factor in BRANCH_FACTORS {
            let started = Instant::now();
            let mut metrics = Metrics::default();
            build_tree(
                points.clone(),
                Bounds {
                    min: [0.0; 3],
                    max: [1.0; 3],
                },
                factor,
                0,
                &mut metrics,
            );
            let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
            assert_eq!(
                metrics
                    .leaves
                    .iter()
                    .map(|(_, point_count)| point_count)
                    .sum::<usize>(),
                POINT_COUNT,
                "partition must preserve every source point"
            );
            assert!(
                metrics
                    .leaves
                    .iter()
                    .all(|(_, point_count)| *point_count <= LEAF_POINT_LIMIT),
                "partition must respect the leaf point limit"
            );
            let leaf_count = metrics.leaves.len();
            let mean_leaf_points = POINT_COUNT as f64 / leaf_count as f64;
            let min_leaf_points = metrics
                .leaves
                .iter()
                .map(|(_, point_count)| *point_count)
                .min()
                .unwrap_or(0);
            let max_leaf_points = metrics
                .leaves
                .iter()
                .map(|(_, point_count)| *point_count)
                .max()
                .unwrap_or(0);
            let (query_mean_ratio, query_p95_ratio) = query_stats(&metrics.leaves);
            let compression_ratio = metrics.stored_bytes as f64 / metrics.raw_bytes as f64;
            println!(
                "{distribution},{factor},{},{leaf_count},{},{min_leaf_points},{mean_leaf_points:.1},{max_leaf_points},{},{},{compression_ratio:.4},{query_mean_ratio:.4},{query_p95_ratio:.4},{elapsed_ms:.2}",
                metrics.nodes, metrics.max_depth, metrics.raw_bytes, metrics.stored_bytes,
            );
        }
    }
}

fn generate_points(distribution: &str) -> Vec<Point> {
    let mut points = Vec::with_capacity(POINT_COUNT);
    let mut state = 0xA341_316C_u32;
    for index in 0..POINT_COUNT {
        let random = [
            next_random(&mut state),
            next_random(&mut state),
            next_random(&mut state),
        ];
        let uniform = random.map(|value| value as f64 / u32::MAX as f64);
        let position = match distribution {
            "clustered" if index % 10 != 0 => uniform.map(|value| 0.45 + value * 0.1),
            "plane" => [uniform[0], uniform[1], 0.5 + (uniform[2] - 0.5) * 0.001],
            "line" => {
                let along = uniform[0];
                [
                    along,
                    (along + (uniform[1] - 0.5) * 0.002).clamp(0.0, 1.0),
                    (along + (uniform[2] - 0.5) * 0.002).clamp(0.0, 1.0),
                ]
            }
            "grid" => {
                let x = index % 100;
                let y = (index / 100) % 100;
                let z = index / 10_000;
                [x as f64 / 99.0, y as f64 / 99.0, z as f64 / 9.0]
            }
            _ => uniform,
        };
        points.push(Point {
            position,
            intensity: (position[0] * f64::from(u16::MAX)) as u16,
        });
    }
    points
}

fn next_random(state: &mut u32) -> u32 {
    *state ^= *state << 13;
    *state ^= *state >> 17;
    *state ^= *state << 5;
    *state
}

fn build_tree(
    points: Vec<Point>,
    bounds: Bounds,
    factor: usize,
    depth: u32,
    metrics: &mut Metrics,
) {
    metrics.nodes += 1;
    metrics.max_depth = metrics.max_depth.max(depth);
    if points.len() <= LEAF_POINT_LIMIT || depth == MAX_DEPTH {
        let payload = encode_leaf(&points, bounds);
        let compressed = compress_block(&payload);
        metrics.raw_bytes += payload.len();
        metrics.stored_bytes += compressed.bytes().len();
        metrics.leaves.push((bounds, points.len()));
        return;
    }

    let mut children = BTreeMap::<usize, Vec<Point>>::new();
    for point in points {
        let cell: [usize; 3] = std::array::from_fn(|axis| {
            let span = bounds.max[axis] - bounds.min[axis];
            (((point.position[axis] - bounds.min[axis]) / span * factor as f64) as usize)
                .min(factor - 1)
        });
        let key = (cell[0] * factor + cell[1]) * factor + cell[2];
        children.entry(key).or_default().push(point);
    }

    for (key, child_points) in children {
        let cell = [
            key / (factor * factor),
            (key / factor) % factor,
            key % factor,
        ];
        let child_bounds = Bounds {
            min: std::array::from_fn(|axis| {
                let step = (bounds.max[axis] - bounds.min[axis]) / factor as f64;
                bounds.min[axis] + cell[axis] as f64 * step
            }),
            max: std::array::from_fn(|axis| {
                let step = (bounds.max[axis] - bounds.min[axis]) / factor as f64;
                bounds.min[axis] + (cell[axis] + 1) as f64 * step
            }),
        };
        build_tree(child_points, child_bounds, factor, depth + 1, metrics);
    }
}

fn encode_leaf(points: &[Point], bounds: Bounds) -> Vec<u8> {
    let mut payload = Vec::with_capacity(points.len() * 14);
    for point in points {
        for axis in 0..3 {
            let span = bounds.max[axis] - bounds.min[axis];
            let quantized = if span == 0.0 {
                0
            } else {
                (((point.position[axis] - bounds.min[axis]) / span) * u32::MAX as f64).round()
                    as u32
            };
            payload.extend_from_slice(&quantized.to_le_bytes());
        }
        payload.extend_from_slice(&point.intensity.to_le_bytes());
    }
    payload
}

fn query_stats(leaves: &[(Bounds, usize)]) -> (f64, f64) {
    const QUERY_COUNT: usize = 100;
    const QUERY_SIDE: f64 = 0.1;
    let mut state = 0x9E37_79B9_u32;
    let mut ratios = Vec::with_capacity(QUERY_COUNT);
    for _ in 0..QUERY_COUNT {
        let query_min: [f64; 3] = std::array::from_fn(|_| {
            (next_random(&mut state) as f64 / u32::MAX as f64) * (1.0 - QUERY_SIDE)
        });
        let query_max = query_min.map(|value| value + QUERY_SIDE);
        let candidate_points = leaves
            .iter()
            .filter(|(bounds, _)| {
                (0..3).all(|axis| {
                    bounds.min[axis] <= query_max[axis] && query_min[axis] <= bounds.max[axis]
                })
            })
            .map(|(_, point_count)| point_count)
            .sum::<usize>();
        ratios.push(candidate_points as f64 / POINT_COUNT as f64);
    }
    ratios.sort_by(f64::total_cmp);
    let mean = ratios.iter().sum::<f64>() / QUERY_COUNT as f64;
    let p95 = ratios[(QUERY_COUNT * 95 / 100).min(QUERY_COUNT - 1)];
    (mean, p95)
}
