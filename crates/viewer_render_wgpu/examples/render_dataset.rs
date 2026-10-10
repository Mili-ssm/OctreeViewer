//! Deterministic native render diagnostics, independent of browser/UI readiness.
use std::{
    collections::HashSet,
    error::Error,
    fs::{self, File},
    io::BufWriter,
    path::PathBuf,
    time::Instant,
};
use viewer_core::{FrustumPlane, LodSelectionConfig, PointBlock, SpatialFrustum};
use viewer_format::FormatReader;
use viewer_render_wgpu::{
    OrbitCamera, RendererConfig, SceneBounds, VoxelSceneBuilder, render_offscreen,
};

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() < 2 || args.len() > 5 {
        return Err("usage: render_dataset INPUT.oct OUTPUT_DIRECTORY [0.5,1,2,4,8,16] [WIDTH=1280] [HEIGHT=960]".into());
    }
    let qualities: Vec<f64> = args
        .get(2)
        .map_or("0.5,1,2,4,8,16", String::as_str)
        .split(',')
        .map(str::parse)
        .collect::<Result<_, _>>()?;
    if qualities.is_empty()
        || qualities
            .iter()
            .any(|value| !value.is_finite() || !(0.5..=16.0).contains(value))
    {
        return Err("qualities must be finite values between 0.5 and 16 pixels".into());
    }
    let viewport = [
        args.get(3).map_or(Ok(1280), |s| s.parse())?,
        args.get(4).map_or(Ok(960), |s| s.parse())?,
    ];
    if viewport.into_iter().any(|side| side == 0 || side > 8192) {
        return Err("viewport must be 1..8192 per axis".into());
    }
    let voxel_budget = RendererConfig::default().gpu_budget.max_visible_instances;
    let mut reader = FormatReader::new(File::open(&args[0])?)?;
    let metadata = reader.format_metadata().dataset().clone();
    let hierarchy = metadata.hierarchy();
    let origin = metadata.coordinate_frame().origin();
    let root = hierarchy.stats().root_bounds;
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
    let directory = PathBuf::from(&args[1]);
    fs::create_dir_all(&directory)?;
    for target in qualities {
        let start = Instant::now();
        // The source-point budget selects detail; the renderer independently caps output voxels.
        let cut = hierarchy.select_lod(
            &world_frustum,
            LodSelectionConfig::new(target, u64::MAX)?,
            |node| {
                let bounds = node.bounds();
                let side = (0..3)
                    .map(|axis| bounds.max()[axis] - bounds.min()[axis])
                    .fold(0.0_f64, f64::max);
                camera.projected_error_pixels(bounds, side / 8.0, origin, viewport)
            },
        )?;
        if !cut.budget_limited().is_empty() {
            return Err("requested source detail exceeds the u64 point-count limit".into());
        }
        let project = |bounds| {
            if local_frustum.intersects(&bounds) {
                camera.projected_voxel_edge_pixels(bounds, [0.0; 3], viewport)
            } else {
                0.0
            }
        };
        let mut builder = VoxelSceneBuilder::new(root, origin, target, voxel_budget);
        let mut seen_blocks = HashSet::new();
        let mut source_blocks = 0_usize;
        for node_id in cut.node_ids() {
            let node = hierarchy.node(*node_id).expect("selected node exists");
            for block_id in node.blocks() {
                if !seen_blocks.insert(*block_id) {
                    continue;
                }
                // This diagnostic renders RGB only. Decode one block, fold it into the voxel map,
                // then release it instead of retaining the full selected source cut in RAM.
                let block =
                    PointBlock::new(*block_id, reader.read_geometry(*block_id)?, Vec::new());
                builder.add_block(&block, &project)?;
                source_blocks += 1;
            }
        }
        let voxelized = builder.finish(&project);
        if voxelized.scene.instance_count() == 0 || voxelized.oversized_voxels != 0 {
            return Err("empty voxel scene or projected edges exceed target".into());
        }
        let image = pollster::block_on(render_offscreen(&voxelized.scene, &camera, viewport))?;
        if image.non_background_pixels == 0 {
            return Err("GPU readback contains only background; no image saved".into());
        }
        let image_path = directory.join(format!("voxel-{target}px.png"));
        let mut encoder = png::Encoder::new(
            BufWriter::new(File::create(&image_path)?),
            viewport[0],
            viewport[1],
        );
        encoder.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header()?;
        writer.write_image_data(&image.rgba)?;
        writer.finish()?;
        let report = serde_json::json!({
            "input": args[0], "image": image_path, "viewport": viewport, "target_edge_pixels": target,
            "projected_edge_pixels": {"min": voxelized.min_edge_pixels, "max": voxelized.max_edge_pixels},
            "oversized_voxels": voxelized.oversized_voxels, "source_points": cut.point_count(),
            "source_nodes": cut.node_ids().len(), "rendered_voxels": voxelized.scene.instance_count(),
            "voxel_budget": voxel_budget, "source_point_budget": u64::MAX,
            "streamed_source_blocks": source_blocks,
            "projected_edge_cache_entries": voxelized.projected_edge_cache_entries,
            "non_background_pixels": image.non_background_pixels,
            "adapter": image.adapter.name, "backend": format!("{:?}", image.adapter.backend),
            "gpu_readback_complete": true, "elapsed_ms": start.elapsed().as_secs_f64() * 1000.0,
        });
        fs::write(
            image_path.with_extension("json"),
            serde_json::to_vec_pretty(&report)?,
        )?;
        println!("{report}");
    }
    Ok(())
}
