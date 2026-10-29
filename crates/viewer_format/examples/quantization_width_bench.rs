use std::fs::File;

use viewer_core::{BlockId, DatasetSource};
use viewer_format::FormatReader;

const CANDIDATE_BITS: u32 = 16;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("usage: quantization_width_bench DATASET.oct [max-error]")?;
    let max_allowed_error = std::env::args()
        .nth(2)
        .map(|value| value.parse())
        .transpose()?
        .unwrap_or(0.01_f64);
    if !max_allowed_error.is_finite() || max_allowed_error < 0.0 {
        return Err("max-error must be finite and non-negative".into());
    }

    let mut reader = FormatReader::new(File::open(path)?)?;
    let blocks = reader.block_storage_stats()?;
    let mut points_read = 0_u64;
    let mut max_candidate_error = 0.0_f64;
    let mut errors_over_limit = 0_u64;
    let mut constant_blocks = 0_u64;
    let mut fallback_blocks = 0_u64;
    let mut fallback_points = 0_u64;

    for block in blocks {
        let point_block = reader.read_block(BlockId::new(block.id.get()))?;
        let points = point_block.points().points();
        if points.is_empty() {
            continue;
        }
        points_read += points.len() as u64;

        let mut min = [f64::INFINITY; 3];
        let mut max = [f64::NEG_INFINITY; 3];
        for point in points {
            for axis in 0..3 {
                let value = f64::from(point.position[axis]);
                min[axis] = min[axis].min(value);
                max[axis] = max[axis].max(value);
            }
        }
        let scales = std::array::from_fn::<_, 3, _>(|axis| {
            (max[axis] - min[axis]) / f64::from((1_u32 << CANDIDATE_BITS) - 1)
        });
        let mut block_max_error = 0.0_f64;
        if scales.iter().all(|scale| *scale == 0.0) {
            constant_blocks += 1;
        }

        for point in points {
            let mut error = 0.0_f64;
            for axis in 0..3 {
                let value = f64::from(point.position[axis]);
                let decoded = if scales[axis] == 0.0 {
                    min[axis] as f32
                } else {
                    let quantized = ((value - min[axis]) / scales[axis]).round();
                    (min[axis] + quantized * scales[axis]) as f32
                };
                #[expect(
                    clippy::disallowed_methods,
                    reason = "benchmark-only error estimate; nothing it computes is written to a file"
                )]
                let combined = error.hypot(value - f64::from(decoded));
                error = combined;
            }
            block_max_error = block_max_error.max(error);
            max_candidate_error = max_candidate_error.max(error);
            if error > max_allowed_error {
                errors_over_limit += 1;
            }
        }
        if block_max_error > max_allowed_error {
            fallback_blocks += 1;
            fallback_points += points.len() as u64;
        }
    }

    println!("dataset_blocks={}", reader.block_storage_stats()?.len());
    println!("points_read={points_read}");
    println!("candidate_bits={CANDIDATE_BITS}");
    println!("candidate_max_error={max_candidate_error:.9}");
    println!("allowed_error={max_allowed_error:.9}");
    println!("points_over_allowed_error={errors_over_limit}");
    println!("constant_blocks={constant_blocks}");
    println!("fallback_blocks={fallback_blocks}");
    println!("fallback_points={fallback_points}");
    println!("u16_eligible_points={}", points_read - fallback_points);
    println!(
        "hybrid_coordinate_saving_bytes={}",
        (points_read - fallback_points) * 6
    );
    println!("coordinate_bytes_per_point: u32=12 candidate_u16=6 theoretical_saving=6");
    println!("passes_candidate_error_check={}", errors_over_limit == 0);
    println!(
        "note=error is measured against the existing reader's f32 output, not original source coordinates"
    );
    Ok(())
}
