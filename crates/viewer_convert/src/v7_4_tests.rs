//! `--format v7.4-pre`: global integer octree, CRS record and shallow roots.

use std::{collections::BTreeMap, fs, path::Path};

use las::{Builder, Point, Vlr, Writer, point::Format};
use viewer_core::NodeId;
use viewer_format::{CrsRaw, FormatReader, FormatVersion, cell_origin_quanta};

use super::{ConvertError, ConvertOptions, convert_file};

const WKT: &str = "PROJCS[\"WGS 84 / UTM zone 32N\",GEOGCS[\"WGS 84\",AUTHORITY[\"EPSG\",\"4326\"]],AUTHORITY[\"EPSG\",\"32632\"]]";

fn options() -> ConvertOptions {
    ConvertOptions::new(262_144, 0.01)
        .unwrap()
        .with_format_version(FormatVersion::V7_PRE4)
        .unwrap()
}

fn vlr(record_id: u16, data: Vec<u8>) -> Vlr {
    Vlr {
        user_id: "LASF_Projection".to_owned(),
        record_id,
        description: String::new(),
        data,
    }
}

fn write_las(path: &Path, points: &[[f64; 3]], vlrs: Vec<Vlr>) {
    let mut builder = Builder::from((1, 2));
    builder.point_format = Format::new(0).unwrap();
    let offset = |axis: usize| {
        points
            .iter()
            .map(|p| p[axis])
            .fold(f64::MAX, f64::min)
            .floor()
    };
    let transform = |axis| las::Transform {
        scale: 0.001,
        offset: offset(axis),
    };
    builder.transforms = las::Vector {
        x: transform(0),
        y: transform(1),
        z: transform(2),
    };
    builder.vlrs = vlrs;
    let mut writer = Writer::from_path(path, builder.into_header().unwrap()).unwrap();
    for p in points {
        writer
            .write_point(Point {
                x: p[0],
                y: p[1],
                z: p[2],
                ..Point::default()
            })
            .unwrap();
    }
}

fn cloud(count: usize, origin: [f64; 3], extent: f64, seed: u64) -> Vec<[f64; 3]> {
    let mut state = seed;
    let mut next = || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((state >> 33) % 100_000) as f64 / 100_000.0
    };
    (0..count)
        .map(|_| {
            let c = [next(), next(), next()];
            [0, 1, 2].map(|a| origin[a] + (c[a] * extent * 1000.0).round() / 1000.0)
        })
        .collect()
}

fn convert(points: &[[f64; 3]], vlrs: Vec<Vlr>, dir: &Path, name: &str) -> FormatReader<fs::File> {
    let input = dir.join(format!("{name}.las"));
    let output = dir.join(format!("{name}.oct"));
    write_las(&input, points, vlrs);
    convert_file(&input, &output, options()).unwrap();
    let mut reader = FormatReader::new(fs::File::open(&output).unwrap()).unwrap();
    reader
        .attach_attributes(fs::File::open(crate::publish::output_attributes_path(&output)).unwrap())
        .unwrap();
    reader
}

#[test]
fn same_input_gives_identical_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let points = cloud(3000, [500_000.0, 4_400_000.0, 100.0], 50.0, 1);
    write_las(
        &dir.path().join("a.las"),
        &points,
        vec![vlr(2112, WKT.as_bytes().to_vec())],
    );
    for name in ["x", "y"] {
        convert_file(
            &dir.path().join("a.las"),
            &dir.path().join(format!("{name}.oct")),
            options(),
        )
        .unwrap();
    }
    for ext in ["oct", "octattrs"] {
        assert_eq!(
            fs::read(dir.path().join(format!("x.{ext}"))).unwrap(),
            fs::read(dir.path().join(format!("y.{ext}"))).unwrap()
        );
    }
}

#[test]
fn crs_is_read_from_wkt_geotiff_keys_and_absent() {
    let dir = tempfile::tempdir().unwrap();
    let points = cloud(50, [10.0, 20.0, 1.0], 5.0, 2);
    let reader = convert(
        &points,
        vec![vlr(2112, WKT.as_bytes().to_vec())],
        dir.path(),
        "wkt",
    );
    let frame = reader.format_metadata().global_frame().unwrap();
    assert_eq!(frame.crs().identity(), Some(("EPSG", 32632)));
    assert!(matches!(frame.crs().raw(), CrsRaw::Wkt(text) if text == WKT));
    let scale = frame.source_scale_offset().unwrap();
    assert_eq!(scale.scale(), [0.001; 3]);

    let words: [u16; 12] = [1, 1, 0, 2, 1024, 0, 1, 1, 3072, 0, 1, 25832];
    let directory: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let reader = convert(
        &points,
        vec![vlr(34735, directory.clone())],
        dir.path(),
        "geotiff",
    );
    let crs = reader
        .format_metadata()
        .global_frame()
        .unwrap()
        .crs()
        .clone();
    assert_eq!(crs.identity(), Some(("EPSG", 25832)));
    assert!(
        matches!(crs.raw(), CrsRaw::GeoTiff { key_directory, .. } if *key_directory == directory)
    );

    let reader = convert(&points, Vec::new(), dir.path(), "local");
    let crs = reader
        .format_metadata()
        .global_frame()
        .unwrap()
        .crs()
        .clone();
    assert_eq!(crs.identity(), None);
    assert!(matches!(crs.raw(), CrsRaw::None));
}

#[test]
fn cloud_centred_on_zero_converts_at_the_default_error() {
    let dir = tempfile::tempdir().unwrap();
    let points = cloud(2000, [-50.0, -50.0, -5.0], 100.0, 3);
    let reader = convert(
        &points,
        vec![vlr(2112, WKT.as_bytes().to_vec())],
        dir.path(),
        "centred",
    );
    let frame = reader.format_metadata().global_frame().unwrap();
    assert_eq!(
        frame.root_level(),
        0,
        "a cloud straddling the sign bit has a level-0 root"
    );
    // The stored data extent hugs the points while the root stays a lattice cube.
    let data = frame.data_bounds().unwrap();
    for axis in 0..3 {
        let extent = data.max()[axis] - data.min()[axis];
        assert!(
            (0.0..=100.0 + 1e-3).contains(&extent),
            "axis {axis}: {extent}"
        );
    }
    assert!(data.max()[0] - data.min()[0] > 90.0);
    assert!(
        reader
            .format_metadata()
            .viewer_dataset()
            .unwrap()
            .hierarchy()
            .stats()
            .root_bounds
            .contains(&data)
    );
    assert_eq!(
        reader
            .format_metadata()
            .dataset()
            .hierarchy()
            .stats()
            .source_point_count,
        Some(2000)
    );
}

#[test]
fn local_cloud_near_zero_has_its_root_at_its_own_first_split() {
    let dir = tempfile::tempdir().unwrap();
    // 0.2 m wide and straddling every high bit: the global lattice would need a level-0 root.
    let points = cloud(5000, [-0.1, -0.1, -0.1], 0.2, 6);
    let reader = convert(&points, Vec::new(), dir.path(), "local-zero");
    let metadata = reader.format_metadata();
    let frame = metadata.global_frame().unwrap();
    assert!(frame.crs().is_local());
    // The cloud spans about 6 600 quanta (< 2^13), so the root is the first node that holds it.
    assert!(
        frame.root_level() >= 50,
        "root level {}",
        frame.root_level()
    );
    assert_eq!(frame.root_prefix(), [0; 3]);
    assert_eq!(frame.lattice_base(), frame.frame_origin_quanta());
    let hierarchy = metadata.dataset().hierarchy();
    #[expect(
        clippy::maybe_infinite_iter,
        reason = "ends at the first node id the hierarchy does not hold"
    )]
    let nodes = (0..)
        .map(NodeId::new)
        .take_while(|id| hierarchy.node(*id).is_some())
        .count();
    let deepest = (0..nodes)
        .map(|index| hierarchy.node(NodeId::new(index as u64)).unwrap().level())
        .max()
        .unwrap();
    assert!(
        deepest <= 3,
        "no chain from a straddled high bit: {deepest}"
    );
}

#[test]
fn box_crossing_2_pow_22_has_the_smallest_containing_root() {
    let dir = tempfile::tempdir().unwrap();
    let edge = 4_194_304.0;
    let points = cloud(2000, [edge - 20.0, 1000.0, 0.0], 40.0, 4);
    let reader = convert(
        &points,
        vec![vlr(2112, WKT.as_bytes().to_vec())],
        dir.path(),
        "edge",
    );
    let frame = reader.format_metadata().global_frame().unwrap();
    // x straddles 2^22 m = 2^37 quanta, so the root side is 2^38 quanta (a = 26) at most: a
    // single-child chain may follow, but no level above the smallest containing node exists.
    assert_eq!(frame.root_level(), 26);
}

#[test]
fn overlapping_crops_share_node_cells_in_the_common_area() {
    let dir = tempfile::tempdir().unwrap();
    let all = cloud(140_000, [300_000.0, 4_000_000.0, 0.0], 100.0, 5);
    let crop = |low: f64, high: f64| -> Vec<[f64; 3]> {
        all.iter()
            .copied()
            .filter(|p| p[0] >= 300_000.0 + low && p[0] < 300_000.0 + high)
            .collect()
    };
    let cells = |reader: &FormatReader<fs::File>| {
        let metadata = reader.format_metadata();
        let frame = metadata.global_frame().unwrap();
        let hierarchy = metadata.dataset().hierarchy();
        let mut map = BTreeMap::new();
        for id in (0..)
            .map(NodeId::new)
            .take_while(|id| hierarchy.node(*id).is_some())
        {
            let node = hierarchy.node(id).unwrap();
            let cell = frame.node_cell(id).unwrap();
            map.insert(
                (frame.root_level() + node.level(), cell.prefix()),
                node.point_count(),
            );
        }
        map
    };
    let a = cells(&convert(
        &crop(0.0, 101.0),
        vec![vlr(2112, WKT.as_bytes().to_vec())],
        dir.path(),
        "a",
    ));
    let b = cells(&convert(
        &crop(20.0, 101.0),
        vec![vlr(2112, WKT.as_bytes().to_vec())],
        dir.path(),
        "b",
    ));
    // A node whose x range lies inside [20, 101) m holds the same points in both crops.
    let (low, high) = (
        (300_020.0_f64 * 32_768.0) as i128,
        (300_101.0_f64 * 32_768.0) as i128,
    );
    let mut compared = 0;
    for ((level, prefix), count) in &a {
        let origin = i128::from(cell_origin_quanta(*level, prefix[0]));
        let side = 1_i128 << (64 - level);
        if origin >= low
            && origin + side <= high
            && let Some(other) = b.get(&(*level, *prefix))
        {
            assert_eq!(count, other, "node level {level} prefix {prefix:?}");
            compared += 1;
        }
    }
    assert!(
        compared > 0,
        "the crops must share at least one node in the common area"
    );
    let shared = a.keys().filter(|key| b.contains_key(*key)).count();
    assert!(shared > 1);
}

#[test]
fn fixed_parameters_cannot_be_overridden_and_ept_is_refused() {
    let base = ConvertOptions::new(262_144, 0.01).unwrap();
    assert!(matches!(
        base.with_spatial_divisions(4)
            .unwrap()
            .with_format_version(FormatVersion::V7_PRE4)
            .unwrap()
            .validate(),
        Err(ConvertError::FixedByFormat(_))
    ));
    assert!(matches!(
        ConvertOptions::new(1000, 0.01)
            .unwrap()
            .with_format_version(FormatVersion::V7_PRE4)
            .unwrap()
            .validate(),
        Err(ConvertError::FixedByFormat(_))
    ));
    assert!(matches!(
        options().with_sample_grid(8).unwrap().validate(),
        Err(ConvertError::FixedByFormat(_))
    ));
    assert_eq!(options().points_per_block(), 64_000);
}
