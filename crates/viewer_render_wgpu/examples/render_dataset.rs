//! Deterministic native render diagnostics, independent of browser/UI readiness.
use std::{
    error::Error,
    fs::{self, File},
    io::BufWriter,
    path::PathBuf,
    time::Instant,
};
use viewer_core::{
    DatasetSource, DecodedBlockCache, FrustumPlane, LodSelectionConfig, SpatialFrustum,
};
use viewer_format::FormatReader;
use viewer_render_wgpu::{OrbitCamera, SceneBounds, build_voxel_scene, render_offscreen};

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() < 2 || args.len() > 5 {
        return Err("usage: render_dataset INPUT.oct OUTPUT_DIRECTORY [1,2,4,8,16] [WIDTH=1280] [HEIGHT=960]".into());
    }
    let qualities: Vec<f64> = args
        .get(2)
        .map_or("1,2,4,8,16", String::as_str)
        .split(',')
        .map(str::parse)
        .collect::<Result<_, _>>()?;
    if qualities.is_empty()
        || qualities
            .iter()
            .any(|value| !value.is_finite() || !(1.0..=16.0).contains(value))
    {
        return Err("qualities must be finite values between 1 and 16 pixels".into());
    }
    let viewport = [
        args.get(3).map_or(Ok(1280), |s| s.parse())?,
        args.get(4).map_or(Ok(960), |s| s.parse())?,
    ];
    if viewport.into_iter().any(|side| side == 0 || side > 8192) {
        return Err("viewport must be 1..8192 per axis".into());
    }
    let mut reader = FormatReader::new(File::open(&args[0])?)?;
    let metadata = reader.metadata().clone();
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
        // Same source-support tolerance and safety ceiling as the viewer. Never silently coarsen.
        let cut = hierarchy.select_lod(
            &world_frustum,
            LodSelectionConfig::new(target, 4_000_000)?,
            |node| {
                let bounds = node.bounds();
                let side = (0..3)
                    .map(|axis| bounds.max()[axis] - bounds.min()[axis])
                    .fold(0.0_f64, f64::max);
                camera.projected_error_pixels(bounds, side / 8.0, origin, viewport)
            },
        )?;
        if !cut.budget_limited().is_empty() {
            return Err("requested source detail exceeds 4M-point safety ceiling".into());
        }
        let mut cache = DecodedBlockCache::new(512 * 1024 * 1024);
        let ids: Vec<_> = cut
            .node_ids()
            .iter()
            .flat_map(|id| hierarchy.node(*id).unwrap().blocks().iter().copied())
            .collect();
        for id in &ids {
            if !cache.contains(*id) {
                if !cache.insert(reader.read_block(*id)?).cached {
                    return Err("source cut does not fit 512 MiB cache".into());
                }
                // Protect completed reads so an oversized selection fails rather than evicts detail.
                cache.set_protected_blocks(
                    ids.iter()
                        .copied()
                        .filter(|id| cache.contains(*id))
                        .collect::<Vec<_>>(),
                );
            }
        }
        let voxelized = build_voxel_scene(
            hierarchy,
            cut.node_ids(),
            &cache,
            origin,
            target,
            &|bounds| {
                if local_frustum.intersects(&bounds) {
                    camera.projected_voxel_edge_pixels(bounds, [0.0; 3], viewport)
                } else {
                    0.0
                }
            },
        );
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
