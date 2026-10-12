//! CPU-only real-data benchmark for incremental voxel scene construction.
use std::{
    cell::Cell,
    collections::{BTreeMap, HashSet},
    fs::File,
    hint::black_box,
    time::Instant,
};

use viewer_core::{FrustumPlane, LodSelectionConfig, Point, PointBlock, SpatialFrustum};
use viewer_format::FormatReader;
use viewer_render_wgpu::{
    OrbitCamera, SceneBounds, VoxelBuildLimits, VoxelScene, VoxelSceneBuilder,
};

const VIEWPORT: [u32; 2] = [1280, 720];
const TARGET_PIXELS: f64 = 4.0;
const DEFAULT_MAX_DECODED_MIB: u64 = 256;
const MAX_DECODED_MIB: u64 = 2048;

fn hash_scene(scene: &viewer_render_wgpu::RenderScene) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytemuck::cast_slice::<_, u8>(scene.instances()) {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn scaled_bounds(
    bounds: viewer_core::SpatialBounds,
    origin: [f64; 3],
    scale: f64,
) -> viewer_core::SpatialBounds {
    viewer_core::SpatialBounds::new(
        std::array::from_fn(|axis| (bounds.min()[axis] - origin[axis]) * scale),
        std::array::from_fn(|axis| (bounds.max()[axis] - origin[axis]) * scale),
    )
    .expect("dataset bounds and visual scale must be finite")
}

fn build(
    blocks: &[PointBlock],
    root: viewer_core::SpatialBounds,
    origin: [f64; 3],
    target_pixels: f64,
    project: &dyn Fn(viewer_core::SpatialBounds) -> f64,
) -> Result<(VoxelScene, f64, f64), Box<dyn std::error::Error>> {
    let accumulation_started = Instant::now();
    let mut builder =
        VoxelSceneBuilder::with_limits(root, origin, target_pixels, VoxelBuildLimits::default());
    for block in blocks {
        builder.add_block(block, project)?;
    }
    let accumulation_seconds = accumulation_started.elapsed().as_secs_f64();
    let finish_started = Instant::now();
    let scene = builder.try_finish(project)?;
    let finish_seconds = finish_started.elapsed().as_secs_f64();
    Ok((scene, accumulation_seconds, finish_seconds))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if !(1..=4).contains(&args.len()) {
        return Err(
            "usage: voxel_build_bench INPUT.oct [RUNS=3] [MAX_DECODED_MIB=256] [TARGET_PIXELS=4]"
                .into(),
        );
    }
    let runs = if let Some(value) = args.get(1) {
        value
            .to_string_lossy()
            .parse::<usize>()
            .ok()
            .filter(|runs| (1..=20).contains(runs))
            .ok_or("RUNS must be an integer from 1 to 20")?
    } else {
        3
    };
    let max_decoded_mib = args
        .get(2)
        .map(|value| value.to_string_lossy().parse::<u64>())
        .transpose()?
        .unwrap_or(DEFAULT_MAX_DECODED_MIB);
    if !(1..=MAX_DECODED_MIB).contains(&max_decoded_mib) {
        return Err(format!("MAX_DECODED_MIB must be between 1 and {MAX_DECODED_MIB}").into());
    }
    let target_pixels = args
        .get(3)
        .map(|value| value.to_string_lossy().parse::<f64>())
        .transpose()?
        .unwrap_or(TARGET_PIXELS);
    if !target_pixels.is_finite() || !(0.5..=16.0).contains(&target_pixels) {
        return Err("TARGET_PIXELS must be finite and between 0.5 and 16".into());
    }
    let max_decoded_bytes = max_decoded_mib * 1024 * 1024;

    let mut reader = FormatReader::new(File::open(&args[0])?)?;
    let metadata_visual_scale = reader.format_metadata().visual_scale();
    let metadata = reader.format_metadata().dataset();
    let hierarchy = metadata.hierarchy();
    let root = hierarchy.stats().root_bounds;
    let origin = metadata.coordinate_frame().origin();
    let hierarchy_block_count = hierarchy.block_ids().len();
    let storage = reader
        .block_storage_stats()?
        .into_iter()
        .map(|stat| (stat.id, stat.point_count))
        .collect::<BTreeMap<_, _>>();

    let mut camera = OrbitCamera::default();
    if !camera.fit(
        SceneBounds {
            min: std::array::from_fn(|axis| {
                (root.min()[axis] - origin[axis]) * metadata_visual_scale
            }),
            max: std::array::from_fn(|axis| {
                (root.max()[axis] - origin[axis]) * metadata_visual_scale
            }),
        },
        VIEWPORT,
    ) {
        return Err("cannot fit camera to dataset".into());
    }
    let local_frustum = camera
        .spatial_frustum(VIEWPORT)
        .ok_or("camera has no frustum")?;
    let world_frustum = SpatialFrustum::new(
        local_frustum
            .planes()
            .iter()
            .map(|plane| {
                FrustumPlane::new(
                    plane.normal().map(|value| value * metadata_visual_scale),
                    plane.offset()
                        - (0..3)
                            .map(|axis| plane.normal()[axis] * metadata_visual_scale * origin[axis])
                            .sum::<f64>(),
                )
            })
            .collect::<Result<_, _>>()?,
    )?;

    let cut = hierarchy.select_lod(
        &world_frustum,
        LodSelectionConfig::new(target_pixels, u64::MAX)?,
        |node| {
            let bounds = node.bounds();
            let side = (0..3)
                .map(|axis| bounds.max()[axis] - bounds.min()[axis])
                .fold(0.0_f64, f64::max);
            camera.projected_error_pixels(
                scaled_bounds(bounds, origin, metadata_visual_scale),
                side / 8.0 * metadata_visual_scale,
                [0.0; 3],
                VIEWPORT,
            )
        },
    )?;
    let mut seen_blocks = HashSet::new();
    let mut block_ids = Vec::new();
    for node_id in cut.node_ids() {
        let node = hierarchy.node(*node_id).expect("selected node exists");
        for block_id in node.blocks() {
            if seen_blocks.insert(*block_id) {
                block_ids.push(*block_id);
            }
        }
    }

    let mut selected_source_points = 0_u64;
    let mut selected_decoded_bytes_floor = 0_u64;
    for id in &block_ids {
        let point_count = *storage
            .get(id)
            .ok_or("hierarchy references an unknown block")?;
        selected_source_points = selected_source_points
            .checked_add(point_count)
            .ok_or("selected point count overflow")?;
        let block_floor = point_count
            .checked_mul(std::mem::size_of::<Point>() as u64)
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<PointBlock>() as u64))
            .ok_or("selected decoded byte count overflow")?;
        selected_decoded_bytes_floor = selected_decoded_bytes_floor
            .checked_add(block_floor)
            .ok_or("selected decoded byte count overflow")?;
    }
    if selected_decoded_bytes_floor > max_decoded_bytes {
        return Err(format!(
            "selected cut has {} unique blocks and {selected_source_points} source points; point payload plus PointBlock headers alone needs at least {selected_decoded_bytes_floor} bytes, above MAX_DECODED_MIB={max_decoded_mib}",
            block_ids.len(),
        )
        .into());
    }

    // Retaining selected blocks makes every run use the same prepared input. This is a bounded
    // diagnostic fixture, not evidence of out-of-core behavior.
    let mut blocks = Vec::new();
    let mut decoded_bytes = 0_u64;
    let mut selected_points = 0_u64;
    for id in block_ids {
        let point_count = *storage
            .get(&id)
            .ok_or("hierarchy references an unknown block")?;
        let block_bytes = point_count
            .checked_mul(std::mem::size_of::<Point>() as u64)
            .and_then(|bytes| bytes.checked_add(std::mem::size_of::<PointBlock>() as u64))
            .ok_or("decoded byte count overflow")?;
        let next_bytes = decoded_bytes
            .checked_add(block_bytes)
            .ok_or("decoded byte count overflow")?;
        if next_bytes > max_decoded_bytes {
            return Err(format!(
                "decoded blocks exceed {max_decoded_bytes} byte safety limit before block {}; raise MAX_DECODED_MIB only for a bounded diagnostic",
                id.get(),
            )
            .into());
        }
        blocks.try_reserve(1)?;
        let points = reader.read_positions(id)?;
        selected_points = selected_points
            .checked_add(u64::try_from(points.len())?)
            .ok_or("point count overflow")?;
        let block = PointBlock::new(id, points, Vec::new());
        let exact_block_bytes = u64::try_from(block.estimated_memory_bytes())?;
        let exact_next_bytes = decoded_bytes
            .checked_add(exact_block_bytes)
            .ok_or("decoded byte count overflow")?;
        if exact_next_bytes > max_decoded_bytes {
            return Err(format!(
                "decoded blocks exceed {max_decoded_bytes} byte safety limit at block {}; raise MAX_DECODED_MIB only for a bounded diagnostic",
                id.get(),
            )
            .into());
        }
        blocks.push(block);
        decoded_bytes = exact_next_bytes;
    }

    let calls = Cell::new(0_u64);
    let project = |bounds| {
        calls.set(calls.get() + 1);
        let bounds = scaled_bounds(bounds, [0.0; 3], metadata_visual_scale);
        if local_frustum.intersects(&bounds) {
            camera.projected_voxel_edge_pixels(bounds, [0.0; 3], VIEWPORT)
        } else {
            0.0
        }
    };
    let mut sample_seconds = Vec::with_capacity(runs);
    let mut accumulation_seconds = Vec::with_capacity(runs);
    let mut finish_seconds = Vec::with_capacity(runs);
    let mut callback_counts = Vec::with_capacity(runs);
    let mut expected: Option<VoxelScene> = None;
    for _ in 0..runs {
        calls.set(0);
        let started = Instant::now();
        let (voxelized, accumulated, finished) =
            build(black_box(&blocks), root, origin, target_pixels, &project)?;
        sample_seconds.push(started.elapsed().as_secs_f64());
        accumulation_seconds.push(accumulated);
        finish_seconds.push(finished);
        callback_counts.push(calls.get());
        if let Some(previous) = &expected {
            assert_eq!(voxelized.scene, previous.scene);
            assert_eq!(voxelized.min_edge_pixels, previous.min_edge_pixels);
            assert_eq!(voxelized.max_edge_pixels, previous.max_edge_pixels);
            assert_eq!(voxelized.oversized_voxels, previous.oversized_voxels);
            assert_eq!(voxelized.memory_stats, previous.memory_stats);
        } else {
            expected = Some(voxelized);
        }
    }
    let mut sorted_seconds = sample_seconds.clone();
    sorted_seconds.sort_unstable_by(f64::total_cmp);
    let scene = expected.expect("RUNS is at least one");
    let median = if sorted_seconds.len().is_multiple_of(2) {
        (sorted_seconds[sorted_seconds.len() / 2 - 1] + sorted_seconds[sorted_seconds.len() / 2])
            * 0.5
    } else {
        sorted_seconds[sorted_seconds.len() / 2]
    };
    println!(
        "{}",
        serde_json::json!({
            "input": args[0].to_string_lossy(),
            "runs": runs,
            "viewport": VIEWPORT,
            "metadata_visual_scale": metadata_visual_scale,
            "camera_visual_scale": metadata_visual_scale,
            "target_pixels": target_pixels,
            "max_decoded_bytes": max_decoded_bytes,
            "selected_blocks": blocks.len(),
            "selected_source_points_from_storage": selected_source_points,
            "selected_decoded_bytes_floor": selected_decoded_bytes_floor,
            "selected_hierarchy_points": cut.point_count(),
            "decoded_unique_block_points": selected_points,
            "hierarchy_block_count": hierarchy_block_count,
            "decoded_bytes_accounted": decoded_bytes,
            "decoded_bytes_scope": "PointBlock estimated_memory_bytes; excludes allocator and blocks-vector overhead",
            "rendered_voxels": scene.scene.instance_count(),
            "scene_fnv1a64": format!("{:016x}", hash_scene(&scene.scene)),
            "samples_seconds": sample_seconds,
            "samples_accumulate_seconds": accumulation_seconds,
            "samples_finish_seconds": finish_seconds,
            "projection_callback_calls": callback_counts,
            "seconds_min": sorted_seconds[0],
            "seconds_median": median,
            "seconds_max": sorted_seconds[sorted_seconds.len() - 1],
            "projected_edge_pixels": { "min": scene.min_edge_pixels, "max": scene.max_edge_pixels },
            "oversized_voxels": scene.oversized_voxels,
            "memory": {
                "estimated_peak_bytes": scene.memory_stats.estimated_peak_bytes,
                "instance_capacity_bytes": scene.memory_stats.instance_capacity_bytes,
                "projection_cache_peak_entries": scene.memory_stats.projection_cache_peak_entries,
            },
            "resident_blocks_are_diagnostic_only": true,
        })
    );
    Ok(())
}
