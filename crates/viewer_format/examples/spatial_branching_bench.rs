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

struct BuildConfig {
    root_bounds: Bounds,
    factor: usize,
    min_depth: u32,
    fixed_resolution: bool,
}

#[derive(Default)]
struct Metrics {
    nodes: usize,
    leaves: Vec<(Bounds, usize)>,
    raw_bytes: usize,
    stored_bytes: usize,
    max_depth: u32,
    build_and_encode_ms: f64,
}

fn main() {
    println!(
        "distribution,mode,factor,min_depth,nodes,leaves,max_depth,min_leaf_points,mean_leaf_points,max_leaf_points,raw_bytes,stored_bytes,lz4_ratio,query10_mean_candidate_ratio,query10_p95_candidate_ratio,query10_mean_overfetch,query10_p95_overfetch,build_and_encode_ms"
    );
    for distribution in ["uniform", "clustered", "plane", "line", "grid"] {
        let points = generate_points(distribution);
        for factor in BRANCH_FACTORS {
            let metrics = run_case(&points, factor, 0, false);
            print_metrics(distribution, "point_cap", factor, 0, &points, &metrics);
        }

        let extent_4 = [run_case(&points, 2, 2, true), run_case(&points, 4, 1, true)];
        assert_equivalent_partition(&extent_4[0], &extent_4[1]);
        for (metrics, factor, min_depth) in [(&extent_4[0], 2, 2), (&extent_4[1], 4, 1)] {
            print_metrics(
                distribution,
                "same_extent_4",
                factor,
                min_depth,
                &points,
                metrics,
            );
        }
        let extent_8 = [run_case(&points, 2, 3, true), run_case(&points, 8, 1, true)];
        assert_equivalent_partition(&extent_8[0], &extent_8[1]);
        for (metrics, factor, min_depth) in [(&extent_8[0], 2, 3), (&extent_8[1], 8, 1)] {
            print_metrics(
                distribution,
                "same_extent_8",
                factor,
                min_depth,
                &points,
                metrics,
            );
        }
    }
}

fn run_case(points: &[Point], factor: usize, min_depth: u32, fixed_resolution: bool) -> Metrics {
    let started = Instant::now();
    let mut metrics = Metrics::default();
    let root_bounds = Bounds {
        min: [0.0; 3],
        max: [1.0; 3],
    };
    let config = BuildConfig {
        root_bounds,
        factor,
        min_depth,
        fixed_resolution,
    };
    build_tree(points.to_vec(), root_bounds, 0, &config, &mut metrics);
    metrics.build_and_encode_ms = started.elapsed().as_secs_f64() * 1000.0;
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
        metrics.leaves.iter().all(|(_, point_count)| {
            fixed_resolution || *point_count <= LEAF_POINT_LIMIT || metrics.max_depth == MAX_DEPTH
        }),
        "partition must respect the leaf point limit"
    );
    metrics
}

fn assert_equivalent_partition(left: &Metrics, right: &Metrics) {
    assert_eq!(left.leaves.len(), right.leaves.len());
    assert_eq!(left.raw_bytes, right.raw_bytes);
    assert_eq!(left.stored_bytes, right.stored_bytes);
    let mut left_leaves = left.leaves.iter().collect::<Vec<_>>();
    let mut right_leaves = right.leaves.iter().collect::<Vec<_>>();
    let sort_by_bounds = |leaves: &mut Vec<&(Bounds, usize)>| {
        leaves.sort_by(|(left, _), (right, _)| {
            left.min
                .iter()
                .zip(right.min)
                .find_map(|(left, right)| {
                    let order = left.total_cmp(&right);
                    (order != std::cmp::Ordering::Equal).then_some(order)
                })
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    };
    sort_by_bounds(&mut left_leaves);
    sort_by_bounds(&mut right_leaves);
    for ((left_bounds, left_count), (right_bounds, right_count)) in
        left_leaves.into_iter().zip(right_leaves)
    {
        assert_eq!(left_count, right_count);
        for axis in 0..3 {
            assert_eq!(left_bounds.min[axis], right_bounds.min[axis]);
            assert_eq!(left_bounds.max[axis], right_bounds.max[axis]);
        }
    }
}

fn print_metrics(
    distribution: &str,
    mode: &str,
    factor: usize,
    min_depth: u32,
    points: &[Point],
    metrics: &Metrics,
) {
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
    let (query_mean_ratio, query_p95_ratio, query_mean_overfetch, query_p95_overfetch) =
        query_stats(&metrics.leaves, points);
    let compression_ratio = metrics.stored_bytes as f64 / metrics.raw_bytes as f64;
    println!(
        "{distribution},{mode},{factor},{min_depth},{},{leaf_count},{},{min_leaf_points},{mean_leaf_points:.1},{max_leaf_points},{},{},{compression_ratio:.4},{query_mean_ratio:.4},{query_p95_ratio:.4},{query_mean_overfetch:.2},{query_p95_overfetch:.2},{:.2}",
        metrics.nodes,
        metrics.max_depth,
        metrics.raw_bytes,
        metrics.stored_bytes,
        metrics.build_and_encode_ms,
    );
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
        let uniform = random.map(|value| f64::from(value) / f64::from(u32::MAX));
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
    depth: u32,
    config: &BuildConfig,
    metrics: &mut Metrics,
) {
    metrics.nodes += 1;
    metrics.max_depth = metrics.max_depth.max(depth);
    if ((points.len() <= LEAF_POINT_LIMIT || config.fixed_resolution) && depth >= config.min_depth)
        || depth == MAX_DEPTH
    {
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
            let grid_size = config.factor.saturating_pow(depth + 1);
            let root_span = config.root_bounds.max[axis] - config.root_bounds.min[axis];
            let normalized = (point.position[axis] - config.root_bounds.min[axis]) / root_span;
            let scaled = normalized * grid_size as f64;
            let nearest_boundary = scaled.round();
            let stable_scaled = if (scaled - nearest_boundary).abs() < 1e-10 {
                nearest_boundary
            } else {
                scaled.floor()
            };
            (stable_scaled as usize).min(grid_size - 1) % config.factor
        });
        let key = (cell[0] * config.factor + cell[1]) * config.factor + cell[2];
        children.entry(key).or_default().push(point);
    }

    for (key, child_points) in children {
        let cell = [
            key / (config.factor * config.factor),
            (key / config.factor) % config.factor,
            key % config.factor,
        ];
        let child_bounds = Bounds {
            min: std::array::from_fn(|axis| {
                let step = (bounds.max[axis] - bounds.min[axis]) / config.factor as f64;
                bounds.min[axis] + cell[axis] as f64 * step
            }),
            max: std::array::from_fn(|axis| {
                let step = (bounds.max[axis] - bounds.min[axis]) / config.factor as f64;
                bounds.min[axis] + (cell[axis] + 1) as f64 * step
            }),
        };
        build_tree(child_points, child_bounds, depth + 1, config, metrics);
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
                (((point.position[axis] - bounds.min[axis]) / span) * f64::from(u32::MAX)).round()
                    as u32
            };
            payload.extend_from_slice(&quantized.to_le_bytes());
        }
        payload.extend_from_slice(&point.intensity.to_le_bytes());
    }
    payload
}

fn query_stats(leaves: &[(Bounds, usize)], points: &[Point]) -> (f64, f64, f64, f64) {
    const QUERY_COUNT: usize = 100;
    const QUERY_SIDE: f64 = 0.1;
    let mut state = 0x9E37_79B9_u32;
    let mut ratios = Vec::with_capacity(QUERY_COUNT);
    let mut overfetch = Vec::with_capacity(QUERY_COUNT);
    for _ in 0..QUERY_COUNT {
        let anchor_index = next_random(&mut state) as usize % points.len();
        let anchor = points[anchor_index].position;
        let query_min = anchor.map(|value| (value - QUERY_SIDE / 2.0).clamp(0.0, 1.0 - QUERY_SIDE));
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
        let actual_points = points
            .iter()
            .filter(|point| {
                (0..3).all(|axis| {
                    query_min[axis] <= point.position[axis]
                        && point.position[axis] <= query_max[axis]
                })
            })
            .count();
        ratios.push(candidate_points as f64 / POINT_COUNT as f64);
        overfetch.push(candidate_points as f64 / actual_points.max(1) as f64);
    }
    ratios.sort_by(f64::total_cmp);
    overfetch.sort_by(f64::total_cmp);
    let mean = ratios.iter().sum::<f64>() / QUERY_COUNT as f64;
    let p95 = ratios[(QUERY_COUNT * 95 / 100).min(QUERY_COUNT - 1)];
    let mean_overfetch = overfetch.iter().sum::<f64>() / QUERY_COUNT as f64;
    let p95_overfetch = overfetch[(QUERY_COUNT * 95 / 100).min(QUERY_COUNT - 1)];
    (mean, p95, mean_overfetch, p95_overfetch)
}
