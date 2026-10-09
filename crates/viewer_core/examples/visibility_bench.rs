use std::hint::black_box;
use std::time::{Duration, Instant};

use viewer_core::{
    ColorRgb, Point, PointBatch, SparseVoxelStore, VisibilityQuery, VisibleVoxel,
    VoxelizationConfig,
};

const POINT_COUNT: usize = 200_000;
const SAMPLES: usize = 9;
const LIMITS: [usize; 3] = [256, 4_096, 65_536];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let points = (0..POINT_COUNT)
        .map(|index| {
            Point::new(
                [index as f32, 0.0, 0.0],
                ColorRgb::new(index as u8, (index >> 8) as u8, (index >> 16) as u8),
            )
        })
        .collect::<Vec<_>>();
    let mut store = SparseVoxelStore::new(VoxelizationConfig::new(1.0, [0.0; 3])?);
    store.insert_batch(&PointBatch::from(points))?;

    println!("candidates={POINT_COUNT} samples={SAMPLES} profile=perf");
    for limit in LIMITS {
        let query = VisibilityQuery {
            bounds: None,
            limit: Some(limit),
        };
        black_box(full_sort_limit(&store, limit));
        black_box(store.build_visible_batch(query));

        let mut baseline = Vec::with_capacity(SAMPLES);
        let mut limited = Vec::with_capacity(SAMPLES);
        for sample in 0..SAMPLES {
            if sample % 2 == 0 {
                baseline.push(time_once(|| black_box(full_sort_limit(&store, limit))));
                limited.push(time_once(|| black_box(store.build_visible_batch(query))));
            } else {
                limited.push(time_once(|| black_box(store.build_visible_batch(query))));
                baseline.push(time_once(|| black_box(full_sort_limit(&store, limit))));
            }
        }
        let baseline = median(baseline);
        let limited = median(limited);
        println!(
            "limit={limit} full_sort_median_us={} limited_query_median_us={} speedup={:.2}x",
            baseline.as_micros(),
            limited.as_micros(),
            baseline.as_secs_f64() / limited.as_secs_f64()
        );
    }
    Ok(())
}

fn time_once<T>(run: impl FnOnce() -> T) -> Duration {
    let start = Instant::now();
    black_box(run());
    start.elapsed()
}

fn median(mut samples: Vec<Duration>) -> Duration {
    samples.sort_unstable();
    samples[samples.len() / 2]
}

fn full_sort_limit(store: &SparseVoxelStore, limit: usize) -> Vec<VisibleVoxel> {
    let mut visible = store
        .iter_voxels()
        .map(|voxel| VisibleVoxel {
            center: voxel.center,
            size: voxel.size,
            color: voxel.color,
            point_count: voxel.point_count,
        })
        .collect::<Vec<_>>();
    visible.sort_by(compare_visible);
    visible.truncate(limit);
    visible
}

fn compare_visible(left: &VisibleVoxel, right: &VisibleVoxel) -> std::cmp::Ordering {
    left.center[0]
        .total_cmp(&right.center[0])
        .then_with(|| left.center[1].total_cmp(&right.center[1]))
        .then_with(|| left.center[2].total_cmp(&right.center[2]))
}
