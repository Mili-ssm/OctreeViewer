//! Real-index LOD selection with the renderer's fitted camera; no payload reads or GPU device.
use std::{cell::Cell, fs::File, hint::black_box, time::Instant};

use viewer_core::{FrustumPlane, LodSelectionConfig, NodeId, SpatialFrustum};
use viewer_format::FormatReader;
use viewer_render_wgpu::{OrbitCamera, SceneBounds};

fn ids(values: &[NodeId]) -> Vec<u64> {
    values.iter().map(|id| id.get()).collect()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 1 {
        return Err("usage: lod_selection_bench INPUT.oct".into());
    }
    let reader = FormatReader::new(File::open(&args[0])?)?;
    let metadata = reader.format_metadata().dataset();
    let hierarchy = metadata.hierarchy();
    let origin = metadata.coordinate_frame().origin();
    let root = hierarchy.stats().root_bounds;
    let viewport = [640, 480];
    let mut camera = OrbitCamera::default();
    if !camera.fit(
        SceneBounds {
            min: std::array::from_fn(|axis| root.min()[axis] - origin[axis]),
            max: std::array::from_fn(|axis| root.max()[axis] - origin[axis]),
        },
        viewport,
    ) {
        return Err("cannot fit camera to dataset".into());
    }
    let local_frustum = camera
        .spatial_frustum(viewport)
        .ok_or("camera has no frustum")?;
    let world_frustum = SpatialFrustum::new(
        local_frustum
            .planes()
            .iter()
            .map(|plane| {
                FrustumPlane::new(
                    plane.normal(),
                    plane.offset()
                        - (0..3)
                            .map(|axis| plane.normal()[axis] * origin[axis])
                            .sum::<f64>(),
                )
            })
            .collect::<Result<_, _>>()?,
    )?;
    for target in [1.0, 4.0, 16.0] {
        for point_budget in [1_000_000, 4_000_000, u64::MAX] {
            let config = LodSelectionConfig::new(target, point_budget)?;
            let calls = Cell::new(0_u64);
            let project = |node: &viewer_core::SpatialNode| {
                calls.set(calls.get() + 1);
                let bounds = node.bounds();
                let side = (0..3)
                    .map(|axis| bounds.max()[axis] - bounds.min()[axis])
                    .fold(0.0_f64, f64::max);
                camera.projected_error_pixels(bounds, side / 8.0, origin, viewport)
            };
            let expected = hierarchy.select_lod(&world_frustum, config, project)?;
            let expected_calls = calls.get();
            let mut samples = Vec::with_capacity(9);
            for _ in 0..9 {
                calls.set(0);
                let started = Instant::now();
                let selection = hierarchy.select_lod(black_box(&world_frustum), config, project)?;
                samples.push(started.elapsed().as_secs_f64() * 1_000_000.0);
                assert_eq!(selection, expected);
                assert_eq!(calls.get(), expected_calls);
                black_box(selection);
            }
            samples.sort_unstable_by(f64::total_cmp);
            println!(
                "{}",
                serde_json::json!({
                    "target_pixels": target, "point_budget": point_budget,
                    "viewport": viewport, "hierarchy_nodes": hierarchy.stats().node_count,
                    "selected_points": expected.point_count(), "node_ids": ids(expected.node_ids()),
                    "budget_limited": ids(expected.budget_limited()),
                    "resolution_limited": ids(expected.resolution_limited()),
                    "max_error_bits": expected.max_error_pixels().to_bits(),
                    "callback_calls": expected_calls, "samples": samples.len(),
                    "us_min": samples[0], "us_median": samples[4], "us_max": samples[8],
                })
            );
        }
    }
    Ok(())
}
