//! The viewer on a converted `v7.4-pre` file: world-space bounds, spacing-derived representative
//! depth and a pinned scene digest (the golden for the global octree).

use std::{fs, path::Path};

use las::{Builder, Point, Writer, point::Format};
use viewer_core::{
    BlockId, DecodedBlockCache, FrustumPlane, LodSelectionConfig, NodeId, PointBlock,
    SpatialBounds, SpatialFrustum,
};
use viewer_format::{FormatReader, FormatVersion};
use viewer_render_wgpu::{
    CellGrid, OrbitCamera, SceneBounds, VoxelBuildLimits, build_voxel_scene_with_limits,
};

use super::{ConvertOptions, convert_file};

const ORIGIN: [f64; 3] = [300_000.0, 4_000_000.0, 0.0];

fn write_las(path: &Path, count: usize) {
    let mut builder = Builder::from((1, 2));
    builder.point_format = Format::new(0).unwrap();
    let transform = |offset| las::Transform {
        scale: 0.001,
        offset,
    };
    builder.transforms = las::Vector {
        x: transform(ORIGIN[0]),
        y: transform(ORIGIN[1]),
        z: transform(ORIGIN[2]),
    };
    let mut writer = Writer::from_path(path, builder.into_header().unwrap()).unwrap();
    let mut state = 7_u64;
    let mut next = || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((state >> 33) % 100_000) as f64 / 100_000.0
    };
    for _ in 0..count {
        let c = [next(), next(), next()];
        writer
            .write_point(Point {
                x: ORIGIN[0] + (c[0] * 100_000.0).round() / 1000.0,
                y: ORIGIN[1] + (c[1] * 100_000.0).round() / 1000.0,
                z: ORIGIN[2] + (c[2] * 10_000.0).round() / 1000.0,
                ..Point::default()
            })
            .unwrap();
    }
}

#[test]
fn the_viewer_dataset_of_a_v7_4_file_is_world_space_with_stored_spacing() {
    let dir = tempfile::tempdir().unwrap();
    write_las(&dir.path().join("a.las"), 140_000);
    let output = dir.path().join("a.oct");
    let options = ConvertOptions::new(262_144, 0.01)
        .unwrap()
        .with_format_version(FormatVersion::V7_PRE4)
        .unwrap();
    convert_file(&dir.path().join("a.las"), &output, options).unwrap();
    let mut reader = FormatReader::new(fs::File::open(&output).unwrap()).unwrap();
    reader
        .attach_attributes(fs::File::open(crate::publish::output_attributes_path(&output)).unwrap())
        .unwrap();
    let metadata = reader.format_metadata();
    let frame = metadata.global_frame().unwrap();
    let origin = metadata.dataset().coordinate_frame().origin();
    let stored = metadata.dataset().hierarchy();
    let dataset = metadata.viewer_dataset().unwrap();
    let hierarchy = dataset.hierarchy();
    let root_id = hierarchy.root_id();
    assert!(
        !hierarchy.child_ids(root_id).is_empty(),
        "needs internal nodes"
    );

    // Bounds moved to world space; the spacing is the stored one, 1/256 of an internal side.
    for id in (0..)
        .map(NodeId::new)
        .take_while(|id| stored.node(*id).is_some())
    {
        let (local, world) = (stored.node(id).unwrap(), hierarchy.node(id).unwrap());
        for (axis, origin) in origin.iter().enumerate() {
            assert_eq!(
                world.bounds().min()[axis],
                local.bounds().min()[axis] + origin
            );
            assert_eq!(
                world.bounds().max()[axis],
                local.bounds().max()[axis] + origin
            );
        }
        assert_eq!(
            world.spacing(),
            Some(frame.node_cell(id).unwrap().spacing())
        );
        if !hierarchy.child_ids(id).is_empty() {
            let side = world.bounds().max()[0] - world.bounds().min()[0];
            assert_eq!(side / world.representative_spacing(), 256.0);
        }
    }
    let root = hierarchy.node(root_id).unwrap();
    let grid = CellGrid {
        root_min: std::array::from_fn(|axis| root.bounds().min()[axis] - origin[axis]),
        root_side: root.bounds().max()[0] - root.bounds().min()[0],
    };
    // Internal nodes land 8 levels below their own depth, relative to the stored root.
    for id in (0..)
        .map(NodeId::new)
        .take_while(|id| hierarchy.node(*id).is_some())
    {
        let node = hierarchy.node(id).unwrap();
        if !hierarchy.child_ids(id).is_empty() {
            let depth = viewer_render_wgpu::representative_depth_for_spacing(
                &grid,
                node.representative_spacing(),
            );
            assert_eq!(u32::from(depth), node.level() + 8);
        }
    }

    // Scene digests over a root cut, a children cut and a mixed cut.
    let mut cache = DecodedBlockCache::new(256 * 1024 * 1024);
    for id in (0..)
        .map(NodeId::new)
        .take_while(|id| hierarchy.node(*id).is_some())
    {
        for block in hierarchy.node(id).unwrap().blocks() {
            let points = reader.read_positions(*block).unwrap();
            cache.insert(PointBlock::new(
                BlockId::new(block.get()),
                points,
                Vec::new(),
            ));
        }
    }
    let children = hierarchy.child_ids(root_id).to_vec();
    // 4 px per metre: a 4 px target asks for cells of about one metre.
    let project = |bounds: SpatialBounds| (bounds.max()[0] - bounds.min()[0]) * 4.0;
    let mut mixed = vec![root_id];
    mixed.push(children[0]);
    let digests: Vec<(usize, u64)> = [vec![root_id], children.clone(), mixed]
        .iter()
        .map(|cut| {
            let scene = build_voxel_scene_with_limits(
                hierarchy,
                cut,
                &cache,
                origin,
                4.0,
                &project,
                VoxelBuildLimits::default(),
            )
            .unwrap();
            (scene.scene.instance_count(), scene.scene.scene_digest())
        })
        .collect();
    assert!(digests.iter().all(|(count, _)| *count > 0));
    assert_eq!(digests, GOLDEN, "actual: {digests:?}");
}

const GOLDEN: [(usize, u64); 3] = [
    (75_378, 7_110_426_578_556_197_141),
    (75_378, 7_110_426_578_556_197_141),
    (75_378, 7_110_426_578_556_197_141),
];

/// A 15 cm cloud straddling 0 (like a scanned object in its own frame), 10 um LAS scale.
fn write_small_local_las(path: &Path, count: usize) {
    let mut builder = Builder::from((1, 2));
    builder.point_format = Format::new(0).unwrap();
    let transform = las::Transform {
        scale: 0.00001,
        offset: 0.0,
    };
    builder.transforms = las::Vector {
        x: transform,
        y: transform,
        z: transform,
    };
    let mut writer = Writer::from_path(path, builder.into_header().unwrap()).unwrap();
    let mut state = 11_u64;
    let mut next = || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((state >> 33) % 100_000) as f64 / 100_000.0
    };
    for _ in 0..count {
        writer
            .write_point(Point {
                x: -0.095 + next() * 0.156,
                y: 0.036 + next() * 0.152,
                z: -0.059 + next() * 0.118,
                ..Point::default()
            })
            .unwrap();
    }
}

#[test]
fn a_local_cloud_near_zero_has_a_tight_root_and_every_point_is_reachable() {
    const COUNT: usize = 30_000;
    let dir = tempfile::tempdir().unwrap();
    write_small_local_las(&dir.path().join("a.las"), COUNT);
    let output = dir.path().join("a.oct");
    let options = ConvertOptions::new(262_144, 0.01)
        .unwrap()
        .with_format_version(FormatVersion::V7_PRE4)
        .unwrap();
    convert_file(&dir.path().join("a.las"), &output, options).unwrap();
    let reader = FormatReader::new(fs::File::open(&output).unwrap()).unwrap();
    let metadata = reader.format_metadata();
    let frame = metadata.global_frame().unwrap();
    // The extent is about 0.156 m = 5112 quanta, so the root side is 2^13 quanta (a = 51), not
    // the level-0 root that the sign bit of a cloud straddling 0 would give.
    assert_eq!(frame.root_level(), 51);
    assert_eq!(frame.root_prefix(), [0; 3]);
    let stored = metadata.dataset().hierarchy();
    assert!(
        stored.stats().node_count < 50,
        "no chain of single-child nodes: {}",
        stored.stats().node_count
    );

    let dataset = (*metadata.viewer_dataset().unwrap()).clone();
    let hierarchy = dataset.hierarchy();
    let origin = dataset.coordinate_frame().origin();
    let root = hierarchy.stats().root_bounds;
    let viewport = [1280_u32, 960];
    let mut camera = OrbitCamera::default();
    assert!(camera.fit(
        SceneBounds {
            min: std::array::from_fn(|axis| root.min()[axis] - origin[axis]),
            max: std::array::from_fn(|axis| root.max()[axis] - origin[axis]),
        },
        viewport,
    ));
    let local = camera.spatial_frustum(viewport).unwrap();
    let world = SpatialFrustum::new(
        local
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
            .collect::<Result<_, _>>()
            .unwrap(),
    )
    .unwrap();
    let cut = hierarchy
        .select_lod(
            &world,
            LodSelectionConfig::new(0.5, u64::MAX).unwrap(),
            |node| {
                camera.projected_error_pixels(
                    node.bounds(),
                    node.representative_spacing(),
                    origin,
                    viewport,
                )
            },
        )
        .unwrap();
    assert_eq!(cut.point_count(), COUNT as u64);
}
