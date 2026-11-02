//! `--format-version v7.5-pre`: the paged hierarchy keeps the global-octree semantics of v7.4-pre and
//! the `.oct` / `.octattrs` pair.

use std::{collections::BTreeMap, fs, path::Path};

use las::{Builder, Point, Vlr, Writer, point::Format};
use viewer_format::{FormatReader, FormatVersion, OctreeReader, PagedReader};

use super::{ConvertError, ConvertOptions, convert_file};

const WKT: &str = "PROJCS[\"WGS 84 / UTM zone 32N\",GEOGCS[\"WGS 84\",AUTHORITY[\"EPSG\",\"4326\"]],AUTHORITY[\"EPSG\",\"32632\"]]";

fn options(version: FormatVersion) -> ConvertOptions {
    ConvertOptions::new(262_144, 0.01)
        .unwrap()
        .with_format_version(version)
        .unwrap()
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

fn vlr_wkt() -> Vec<Vlr> {
    vec![Vlr {
        user_id: "LASF_Projection".to_owned(),
        record_id: 2112,
        description: String::new(),
        data: WKT.as_bytes().to_vec(),
    }]
}

fn cells(reader: &FormatReader<fs::File>) -> BTreeMap<(u32, [u64; 3]), u64> {
    let metadata = reader.format_metadata();
    let frame = metadata.global_frame().unwrap();
    let hierarchy = metadata.dataset().hierarchy();
    let mut map = BTreeMap::new();
    // Paged ids are (page, index), so they are not a dense range.
    for node in hierarchy.query(hierarchy.stats().root_bounds) {
        let cell = frame.node_cell(node.id()).unwrap();
        map.insert(
            (frame.root_level() + node.level(), cell.prefix()),
            node.point_count(),
        );
    }
    map
}

#[test]
fn paged_output_has_the_node_set_and_frame_of_v7_4() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("in.las");
    // Several thousand nodes' worth would be slow; 300k points give a multi-level tree.
    write_las(
        &input,
        &cloud(300_000, [500_000.0, 4_400_000.0, 100.0], 80.0, 11),
        vlr_wkt(),
    );
    let mut readers = Vec::new();
    for (name, version) in [("a", FormatVersion::V7_PRE4), ("b", FormatVersion::V7_PRE5)] {
        let output = dir.path().join(format!("{name}.oct"));
        convert_file(&input, &output, options(version)).unwrap();
        let mut reader = FormatReader::new(fs::File::open(&output).unwrap()).unwrap();
        reader
            .attach_attributes(
                fs::File::open(crate::publish::output_attributes_path(&output)).unwrap(),
            )
            .unwrap();
        readers.push(reader);
    }
    assert_eq!(readers[1].format_version(), FormatVersion::V7_PRE5);
    let (flat, paged) = (&readers[0], &readers[1]);
    assert!(cells(flat).len() > 8);
    assert_eq!(cells(flat), cells(paged));
    let (a, b) = (
        flat.format_metadata().global_frame().unwrap(),
        paged.format_metadata().global_frame().unwrap(),
    );
    assert_eq!(a.root_level(), b.root_level());
    assert_eq!(a.root_prefix(), b.root_prefix());
    assert_eq!(a.data_bounds(), b.data_bounds());
    assert_eq!(a.crs(), b.crs());
    assert!(paged.index_size_bytes() < flat.index_size_bytes());
}

#[test]
fn paged_output_is_byte_identical_and_opens_lazily() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("in.las");
    write_las(&input, &cloud(3000, [10.0, 20.0, 1.0], 50.0, 1), Vec::new());
    for name in ["x", "y"] {
        convert_file(
            &input,
            &dir.path().join(format!("{name}.oct")),
            options(FormatVersion::V7_PRE5),
        )
        .unwrap();
    }
    for extension in ["oct", "octattrs"] {
        assert_eq!(
            fs::read(dir.path().join(format!("x.{extension}"))).unwrap(),
            fs::read(dir.path().join(format!("y.{extension}"))).unwrap()
        );
    }
    let reader = PagedReader::open(fs::File::open(dir.path().join("x.oct")).unwrap()).unwrap();
    assert_eq!(reader.header().source_point_count(), Some(3000));
}

#[test]
fn paged_format_refuses_ept_checkpoints_and_keeps_fixed_parameters() {
    let dir = tempfile::tempdir().unwrap();
    write_las(
        &dir.path().join("in.las"),
        &cloud(100, [0.0; 3], 5.0, 3),
        Vec::new(),
    );
    assert!(matches!(
        crate::convert_file_with_checkpoint(
            &dir.path().join("in.las"),
            &dir.path().join("c.oct"),
            options(FormatVersion::V7_PRE5),
            crate::CheckpointMode::Create,
            &mut |_| true,
        ),
        Err(ConvertError::UnsupportedForPagedFormat(_))
    ));
    let ept = tempfile::tempdir().unwrap();
    fs::write(ept.path().join("ept.json"), "{}").unwrap();
    let out = tempfile::tempdir().unwrap();
    let result = convert_file(
        ept.path(),
        &out.path().join("o.oct"),
        options(FormatVersion::V7_PRE5),
    );
    assert!(matches!(
        result,
        Err(ConvertError::UnsupportedForPagedFormat(_))
    ));
    assert!(matches!(
        ConvertOptions::new(1000, 0.01)
            .unwrap()
            .with_format_version(FormatVersion::V7_PRE5),
        Err(ConvertError::FixedByFormat(_))
    ));
    assert_eq!(options(FormatVersion::V7_PRE5).points_per_block(), 64_000);
}

/// A cloud around the origin has a level-0 root and chains far deeper than the 21 levels of the
/// first v7.5-pre draft; it must convert, verify and read back.
#[test]
fn cloud_straddling_the_lattice_origin_converts_with_a_deep_chain() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("in.las");
    write_las(
        &input,
        &cloud(150_000, [-40.0, -40.0, -40.0], 80.0, 5),
        vlr_wkt(),
    );
    let output = dir.path().join("deep.oct");
    let report = convert_file(&input, &output, options(FormatVersion::V7_PRE5)).unwrap();
    assert_eq!(report.point_count, 150_000);
    let mut reader = FormatReader::new(fs::File::open(&output).unwrap()).unwrap();
    reader
        .attach_attributes(fs::File::open(crate::publish::output_attributes_path(&output)).unwrap())
        .unwrap();
    let frame = reader.format_metadata().global_frame().unwrap();
    assert_eq!(frame.root_level(), 0);
    let depth = reader
        .format_metadata()
        .dataset()
        .hierarchy()
        .stats()
        .max_level;
    assert!(depth > 21, "depth {depth}");
    let stats = crate::verify_file(&output).unwrap().paged.unwrap();
    assert!(stats.nodes > 21);
}
