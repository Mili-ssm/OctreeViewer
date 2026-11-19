//! Fixed-cut CPU diagnostic; includes Replacement depth limits, unlike the streaming builder bench.
use std::{collections::BTreeSet, fs::File, hint::black_box, time::Instant};
use viewer_core::{DecodedBlockCache, HierarchySemantics, PointBlock};
use viewer_format::FormatReader;
use viewer_render_wgpu::{
    OrbitCamera, SceneBounds, VoxelBuildLimits, build_voxel_scene_with_limits,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() < 2 || args.len() > 3 {
        return Err(
            "usage: regional_voxel_bench INPUT.oct held|partial|mixed|refined [RUNS=3]".into(),
        );
    }
    let runs = args.get(2).map_or(Ok(3), |s| s.parse::<usize>())?;
    if !(1..=20).contains(&runs) {
        return Err("RUNS must be 1..20".into());
    }
    let mut reader = FormatReader::new(File::open(&args[0])?)?;
    let metadata = (*reader.format_metadata().viewer_dataset()?).clone();
    let h = metadata.hierarchy();
    if h.semantics() != HierarchySemantics::Replacement {
        return Err("diagnostic requires Replacement semantics".into());
    }
    let branches = h.child_ids(h.root_id());
    let held = branches
        .iter()
        .find(|id| !h.is_leaf(**id))
        .ok_or("no coarse branch")?;
    if branches.len() < 2 {
        return Err("need two root branches".into());
    }
    let mut cut = Vec::new();
    for id in branches {
        let refine = match args[1].as_str() {
            "held" => false,
            "partial" => {
                if id == held {
                    cut.push(*id);
                    cut.push(*h.child_ids(*id).first().ok_or("held branch has no child")?);
                    continue;
                }
                false
            }
            "mixed" => id != held,
            "refined" => true,
            _ => return Err("mode must be held, partial, mixed or refined".into()),
        };
        if refine && !h.child_ids(*id).is_empty() {
            cut.extend_from_slice(h.child_ids(*id));
        } else {
            cut.push(*id);
        }
    }
    cut.sort_unstable();
    let blocks = cut
        .iter()
        .flat_map(|id| h.node(*id).unwrap().blocks())
        .copied()
        .collect::<BTreeSet<_>>();
    let points = cut
        .iter()
        .map(|id| h.node(*id).unwrap().point_count())
        .sum::<u64>();
    let cache_budget = 256 * 1024 * 1024;
    if points.saturating_mul(16) > cache_budget as u64 {
        return Err("fixed cut exceeds the 256 MiB geometry preflight".into());
    }
    let mut cache = DecodedBlockCache::new(cache_budget);
    for id in &blocks {
        let block = PointBlock::new(*id, reader.read_positions(*id)?, vec![]);
        cache.insert(block);
    }
    if blocks.iter().any(|id| !cache.contains(*id)) {
        return Err("fixed cut did not fit the cache".into());
    }
    let origin = metadata.coordinate_frame().origin();
    let root = h.stats().root_bounds;
    let mut camera = OrbitCamera::default();
    let viewport = [1280, 720];
    if !camera.fit(
        SceneBounds {
            min: std::array::from_fn(|i| root.min()[i] - origin[i]),
            max: std::array::from_fn(|i| root.max()[i] - origin[i]),
        },
        viewport,
    ) {
        return Err("camera fit failed".into());
    }
    let project = |bounds| camera.projected_voxel_edge_pixels(bounds, [0.0; 3], viewport);
    let mut samples = Vec::new();
    let mut expected = None;
    let mut metrics = serde_json::Value::Null;
    for _ in 0..runs {
        let start = Instant::now();
        let scene = build_voxel_scene_with_limits(
            h,
            &cut,
            &cache,
            origin,
            4.0,
            &project,
            VoxelBuildLimits::default(),
        )?;
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
        let hash = bytemuck::cast_slice::<_, u8>(scene.scene.instances())
            .iter()
            .fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
                (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
            });
        let result = (scene.scene.instance_count(), hash);
        if let Some(previous) = expected {
            assert_eq!(result, previous);
        }
        expected = Some(result);
        let sizes = scene
            .scene
            .instances()
            .iter()
            .map(|v| v.size)
            .fold((f32::INFINITY, 0.0_f32), |(lo, hi), s| {
                (lo.min(s), hi.max(s))
            });
        metrics = serde_json::json!({"rendered_voxels":result.0,"checksum":format!("{:016x}",result.1),"min_size":sizes.0,"max_size":sizes.1,"estimated_peak_bytes":scene.memory_stats.estimated_peak_bytes,"instance_capacity_bytes":scene.memory_stats.instance_capacity_bytes,"projection_cache_peak_entries":scene.memory_stats.projection_cache_peak_entries,"oversized_voxels":scene.oversized_voxels});
        black_box(scene);
    }
    println!(
        "{}",
        serde_json::json!({"input":args[0],"mode":args[1],"node_ids":cut.iter().map(|id|id.get()).collect::<Vec<_>>(),"source_points":points,"blocks":blocks.len(),"cache_bytes":cache.used_bytes(),"samples_ms":samples,"metrics":metrics,"target_pixels":4.0,"viewport":viewport,"gray_geometry_only":true})
    );
    Ok(())
}
