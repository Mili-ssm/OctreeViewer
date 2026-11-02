//! The viewer on a converted `v7.4-pre` file: world-space bounds, spacing-derived representative
//! depth and a pinned scene digest (the golden for the global octree).

use std::{fs, path::Path};

use las::{Builder, Point, Writer, point::Format};
use viewer_core::{BlockId, DecodedBlockCache, NodeId, PointBlock, SpatialBounds};
use viewer_format::{FormatReader, FormatVersion};
use viewer_render_wgpu::{CellGrid, VoxelBuildLimits, build_voxel_scene_with_limits};

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
    (2, 7_562_690_955_787_948_776),
    (24_102, 9_172_085_341_111_685_325),
    (24_335, 6_436_223_555_049_525_197),
];
