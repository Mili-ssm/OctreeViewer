//! Nearest-neighbour distance samples of a `v8.1-pre` leaf, aggregated by the planner into the
//! dataset's p1/p5/p50 quantiles. The sampling and search are integer arithmetic on quantized
//! positions (1 quantum = 2^-15 units), with one conversion to units at the end, so the result
//! depends neither on the order points arrive in nor on
//! the number of workers or the executor.

use super::node_job::spread_bits;

/// Points the sample aims at over the whole source (a technical call, tunable after measuring).
/// 2^16 keeps about 650 samples under p1 and costs a small part of the leaf jobs; the ring search
/// cost scales with this number.
pub const SAMPLE_TARGET_POINTS: u64 = 1 << 16;

/// Fixed seed of the sampling hash: changing it changes every `v8.1-pre` quantile.
const SAMPLE_SEED: u64 = 0x6a09_e667_f3bc_c908;

/// Ring search budget in leaf-spacing cells. A sample with no neighbour within this many rings
/// does not count: only isolated points are dropped (accepted), p1 and p5 are unaffected, and the
/// budget keeps an isolated point from scanning the whole leaf box ring by ring.
const MAX_EMPTY_RINGS: i128 = 16;

const QUANTA_PER_UNIT: f64 = 32_768.0;

/// Hash threshold for sampling about [`SAMPLE_TARGET_POINTS`] of `point_count` points: a point
/// is sampled when its hash is `<=` the threshold. `u64::MAX` samples every point.
#[must_use]
pub fn sample_threshold(point_count: u64) -> u64 {
    if point_count <= SAMPLE_TARGET_POINTS {
        return u64::MAX;
    }
    let scaled = (u128::from(SAMPLE_TARGET_POINTS) << 64) / u128::from(point_count);
    u64::try_from(scaled).unwrap_or(u64::MAX)
}

/// Explicit splitmix64 finalizer (never `DefaultHasher`, whose output is not stable).
fn splitmix64(value: u64) -> u64 {
    let mut z = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

fn position_hash(quantized: [i64; 3]) -> u64 {
    quantized
        .iter()
        .fold(splitmix64(SAMPLE_SEED), |hash, &axis| {
            splitmix64(hash ^ axis as u64)
        })
}

/// The leaf's `(Morton code, point index)` pairs sorted at `code_depth` levels, built once by
/// the leaf spacing measurement. The cell of a shallower depth is the contiguous range of codes
/// sharing its prefix.
pub struct MortonOrder {
    pub code_depth: u32,
    pub sorted: Vec<(u64, usize)>,
}

/// Squared nearest-neighbour distances, in quanta² and without zeros, of the sampled points of
/// one leaf.
///
/// `cells` are the leaf's points in quanta relative to `origin` (the leaf's minimum corner), the
/// leaf side is `2^side_exponent` quanta and the search grid has `2^depth` cells per axis (the
/// measured leaf spacing). A sampled point looks for its nearest neighbour at non-zero distance
/// in growing rings of cells clipped to the leaf box, and stops once the best distance is within
/// the ring radius. A point without such a neighbour contributes nothing. Distances saturate at
/// `u64::MAX` (only for neighbours of the order of the leaf side).
///
/// `order` is the leaf's Morton order when its depth search succeeded (then `depth` is at most
/// its code depth and no cell is clipped); otherwise the function sorts the cell keys itself.
/// Both give the same samples.
#[must_use]
pub fn nearest_distance_samples(
    cells: &[[i128; 3]],
    origin: [i64; 3],
    side_exponent: u32,
    depth: u32,
    threshold: u64,
    order: Option<MortonOrder>,
) -> Vec<u64> {
    let sampled: Vec<usize> = (0..cells.len())
        .filter(|&index| {
            let absolute =
                std::array::from_fn(|axis| (cells[index][axis] + i128::from(origin[axis])) as i64);
            position_hash(absolute) <= threshold
        })
        .collect();
    if sampled.is_empty() {
        return Vec::new();
    }
    let depth = depth.min(side_exponent).min(32);
    let shift = side_exponent - depth;
    let cell_quanta = 1_i128 << shift;
    let limit = 1_i128 << depth;
    let grid_cell = |index: usize| -> [i128; 3] {
        cells[index].map(|value| (value >> shift).clamp(0, limit - 1))
    };
    let mut samples = Vec::new();
    // An order coarser than the search grid cannot list its cells: use the own sort instead.
    if let Some(order) = order.filter(|order| depth <= order.code_depth) {
        let below = 3 * (order.code_depth - depth);
        let sorted = &order.sorted;
        search(
            cells,
            &sampled,
            limit,
            cell_quanta,
            &mut samples,
            |cell, visit| {
                let prefix = spread_bits(cell[0] as u64) << 2
                    | spread_bits(cell[1] as u64) << 1
                    | spread_bits(cell[2] as u64);
                let (low, high) = (prefix << below, (prefix + 1) << below);
                let start = sorted.partition_point(|&(code, _)| code < low);
                for &(code, other) in &sorted[start..] {
                    if code >= high {
                        break;
                    }
                    visit(other);
                }
            },
        );
    } else {
        let key =
            |cell: [i128; 3]| (cell[0] as u128) << 64 | (cell[1] as u128) << 32 | cell[2] as u128;
        let mut index: Vec<(u128, usize)> =
            (0..cells.len()).map(|i| (key(grid_cell(i)), i)).collect();
        index.sort_unstable();
        search(
            cells,
            &sampled,
            limit,
            cell_quanta,
            &mut samples,
            |cell, visit| {
                let wanted = key(cell);
                let start = index.partition_point(|&(k, _)| k < wanted);
                for &(k, other) in &index[start..] {
                    if k != wanted {
                        break;
                    }
                    visit(other);
                }
            },
        );
    }
    samples
}

/// The ring search of every sampled point; `each_in_cell` calls its callback with the index of
/// each point in a grid cell.
fn search(
    cells: &[[i128; 3]],
    sampled: &[usize],
    limit: i128,
    cell_quanta: i128,
    samples: &mut Vec<u64>,
    each_in_cell: impl Fn([i128; 3], &mut dyn FnMut(usize)),
) {
    let grid_shift = cell_quanta.trailing_zeros();
    for &point in sampled {
        let home = cells[point].map(|value| (value >> grid_shift).clamp(0, limit - 1));
        let mut best: Option<u128> = None;
        let visit = |cell: [i128; 3], best: &mut Option<u128>| {
            each_in_cell(cell, &mut |other| {
                // Saturating: with a leaf side of 2^64 quanta a squared distance can pass u128.
                let squared = (0..3)
                    .map(|axis| cells[point][axis].abs_diff(cells[other][axis]))
                    .map(|delta| delta.saturating_mul(delta))
                    .fold(0_u128, u128::saturating_add);
                if squared != 0 && best.is_none_or(|b| squared < b) {
                    *best = Some(squared);
                }
            });
        };
        for ring in 0..=limit {
            if ring > MAX_EMPTY_RINGS && best.is_none() {
                break;
            }
            for dx in -ring..=ring {
                for dy in -ring..=ring {
                    let step = if dx.abs() == ring || dy.abs() == ring {
                        1
                    } else {
                        (2 * ring).max(1)
                    };
                    let mut dz = -ring;
                    while dz <= ring {
                        let cell = [home[0] + dx, home[1] + dy, home[2] + dz];
                        if cell.iter().all(|c| (0..limit).contains(c)) {
                            visit(cell, &mut best);
                        }
                        dz += step;
                    }
                }
            }
            // Every unvisited point is more than `ring` cells away.
            let reach = (ring * cell_quanta) as u128;
            if best.is_some_and(|b| b <= reach.saturating_mul(reach)) {
                break;
            }
        }
        if let Some(squared) = best {
            samples.push(u64::try_from(squared).unwrap_or(u64::MAX));
        }
    }
}

/// p1, p5 and p50 in dataset units of the pooled squared distances; all zeros when there are
/// none (the defined "not measured" value). Total order and nearest-rank quantiles, with the
/// square root and the conversion to units taken once at the end.
#[must_use]
pub fn quantiles_from_samples(mut samples: Vec<u64>) -> [f64; 3] {
    if samples.is_empty() {
        return [0.0; 3];
    }
    samples.sort_unstable();
    let count = samples.len() as u64;
    [1_u64, 5, 50].map(|percent| {
        let rank = (count * percent).div_ceil(100).max(1);
        (samples[rank as usize - 1] as f64).sqrt() / QUANTA_PER_UNIT
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIDE_EXPONENT: u32 = 32;
    /// Cells of 256 quanta.
    const DEPTH: u32 = 24;

    fn grid(side: i128, step: i128) -> Vec<[i128; 3]> {
        (0..side)
            .flat_map(|x| (0..side).flat_map(move |y| (0..side).map(move |z| [x, y, z])))
            .map(|cell| cell.map(|value| value * step))
            .collect()
    }

    fn quantiles(cells: &[[i128; 3]]) -> [f64; 3] {
        let samples = nearest_distance_samples(cells, [0; 3], SIDE_EXPONENT, DEPTH, u64::MAX, None);
        quantiles_from_samples(samples)
    }

    #[test]
    fn quantiles_of_a_regular_grid() {
        let step = 200;
        let measured = quantiles(&grid(10, step));
        let expected = step as f64 / QUANTA_PER_UNIT;
        for value in measured {
            assert!(
                (value - expected).abs() <= 1.0 / QUANTA_PER_UNIT,
                "{measured:?}"
            );
        }
    }

    #[test]
    fn a_neighbour_several_rings_away_is_found_exactly() {
        // Step 700 quanta over cells of 256: the nearest neighbour lies two cells away (ring 2),
        // and the search only stops after ring 3, the first whose radius reaches 700.
        let cells = grid(4, 700);
        let samples =
            nearest_distance_samples(&cells, [0; 3], SIDE_EXPONENT, DEPTH, u64::MAX, None);
        assert_eq!(samples, vec![700 * 700; cells.len()]);
    }

    #[test]
    fn distances_at_the_largest_leaf_side_saturate() {
        // Side 2^64: the squared distance between opposite corners passes u128.
        let far = (1_i128 << 64) - 1;
        let cells = [[0; 3], [far; 3]];
        let samples = nearest_distance_samples(&cells, [0; 3], 64, 0, u64::MAX, None);
        assert_eq!(samples, vec![u64::MAX; 2]);
    }

    #[test]
    fn duplicates_do_not_count() {
        let cells = grid(10, 200);
        let mut with_duplicates = cells.clone();
        with_duplicates.extend(cells.iter().step_by(3));
        with_duplicates.extend(cells.iter().step_by(7));
        assert_eq!(quantiles(&with_duplicates), quantiles(&cells));
        // The duplicates are sampled too (each has a neighbour), they just never are a neighbour.
        let count = |cells: &[[i128; 3]]| {
            nearest_distance_samples(cells, [0; 3], SIDE_EXPONENT, DEPTH, u64::MAX, None).len()
        };
        assert_eq!(count(&cells), cells.len());
        assert_eq!(count(&with_duplicates), with_duplicates.len());
        // A point's exact duplicate is not its neighbour: all three see the point 295 away.
        let pair_and_one = [[5, 5, 5], [5, 5, 5], [300, 5, 5]];
        let samples =
            nearest_distance_samples(&pair_and_one, [0; 3], SIDE_EXPONENT, DEPTH, u64::MAX, None);
        assert_eq!(samples, vec![295 * 295; 3]);
        assert_eq!(quantiles(&[[5, 5, 5]; 4]), [0.0; 3]);
        assert_eq!(quantiles(&[[5, 5, 5]]), [0.0; 3]);
    }

    #[test]
    fn isolated_and_outlier_points_terminate() {
        // One point alone in the leaf, and a cluster with a far outlier in the opposite corner.
        assert_eq!(quantiles(&[[1 << 31, 1 << 31, 1 << 31]]), [0.0; 3]);
        let mut cells = grid(6, 200);
        cells.push([(1 << 32) - 1; 3]);
        let with_outlier = quantiles(&cells);
        assert_eq!(with_outlier, quantiles(&grid(6, 200)));
        assert!(with_outlier[0] > 0.0);
    }

    #[test]
    fn the_sample_depends_on_positions_only() {
        let mut cells = grid(12, 200);
        let threshold = u64::MAX / 16;
        let measured = |cells: &[[i128; 3]]| {
            let mut samples =
                nearest_distance_samples(cells, [7, -3, 11], SIDE_EXPONENT, DEPTH, threshold, None);
            samples.sort_unstable();
            samples
        };
        let forward = measured(&cells);
        assert!(!forward.is_empty() && forward.len() < cells.len() / 8);
        cells.reverse();
        assert_eq!(measured(&cells), forward);
    }

    #[test]
    fn threshold_keeps_everything_for_small_sources() {
        assert_eq!(sample_threshold(SAMPLE_TARGET_POINTS), u64::MAX);
        let rate = sample_threshold(SAMPLE_TARGET_POINTS * 4) as f64 / u64::MAX as f64;
        assert!((rate - 0.25).abs() < 1e-9);
    }

    #[test]
    fn quantiles_use_the_nearest_rank() {
        // 100 distances of 1..=100 quanta: p1 is the 1st, p5 the 5th, p50 the 50th.
        let samples = (1..=100_u64).rev().map(|d| d * d).collect();
        let [p1, p5, p50] = quantiles_from_samples(samples);
        assert_eq!(
            [p1, p5, p50].map(|v| (v * QUANTA_PER_UNIT).round()),
            [1.0, 5.0, 50.0]
        );
        assert_eq!(quantiles_from_samples(Vec::new()), [0.0; 3]);
    }
}
