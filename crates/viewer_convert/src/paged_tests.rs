//! The paged hierarchy of the V8 `.oct` / `.octattrs` pair: global-octree semantics, lazy pages,
//! deep chains and distance quantiles.

use std::{collections::BTreeMap, fs, path::Path};

use las::{Builder, Point, Vlr, Writer, point::Format};
use viewer_format::{FormatReader, FormatVersion, OctreeReader, PagedReader};

use super::{ConvertError, ConvertOptions, convert_file};

const WKT: &str = "PROJCS[\"WGS 84 / UTM zone 32N\",GEOGCS[\"WGS 84\",AUTHORITY[\"EPSG\",\"4326\"]],AUTHORITY[\"EPSG\",\"32632\"]]";

fn options() -> ConvertOptions {
    ConvertOptions::default()
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
fn paged_output_has_a_multi_level_node_set_and_a_global_frame() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("in.las");
    // Several thousand nodes' worth would be slow; 300k points give a multi-level tree.
    write_las(
        &input,
        &cloud(300_000, [500_000.0, 4_400_000.0, 100.0], 80.0, 11),
        vlr_wkt(),
    );
    let output = dir.path().join("a.oct");
    convert_file(&input, &output, options()).unwrap();
    let mut reader = FormatReader::new(fs::File::open(&output).unwrap()).unwrap();
    reader
        .attach_attributes(fs::File::open(crate::publish::output_attributes_path(&output)).unwrap())
        .unwrap();
    assert_eq!(reader.format_version(), FormatVersion::DEFAULT_OUTPUT);
    assert!(cells(&reader).len() > 8);
    let frame = reader.format_metadata().global_frame().unwrap();
    assert_eq!(frame.crs().identity(), Some(("EPSG", 32632)));
}

#[test]
fn paged_output_is_byte_identical_and_opens_lazily() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("in.las");
    write_las(&input, &cloud(3000, [10.0, 20.0, 1.0], 50.0, 1), Vec::new());
    for name in ["x", "y"] {
        convert_file(&input, &dir.path().join(format!("{name}.oct")), options()).unwrap();
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
fn ept_input_is_removed() {
    let ept = tempfile::tempdir().unwrap();
    fs::write(ept.path().join("ept.json"), "{}").unwrap();
    let out = tempfile::tempdir().unwrap();
    let result = convert_file(ept.path(), &out.path().join("o.oct"), options());
    assert!(matches!(result, Err(ConvertError::EptRemoved)));
}

/// A cloud around the origin has a level-0 root and chains far deeper than the 21 levels of the
/// first paged-format draft; it must convert, verify and read back.
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
    let report = convert_file(&input, &output, options()).unwrap();
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

#[test]
fn a_page_target_splits_a_small_cloud_into_more_pages() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("in.las");
    write_las(
        &input,
        &cloud(300_000, [500_000.0, 4_400_000.0, 100.0], 80.0, 11),
        Vec::new(),
    );
    let mut pages = Vec::new();
    for (name, target) in [("a", None), ("b", Some(2_000))] {
        let output = dir.path().join(format!("{name}.oct"));
        let mut options = options();
        if let Some(bytes) = target {
            options = options.with_page_target_bytes(bytes);
        }
        convert_file(&input, &output, options).unwrap();
        let report = crate::verify::verify_file(&output).unwrap();
        pages.push(report.paged.unwrap().pages);
    }
    assert!(pages[1] > pages[0], "{pages:?}");
}

/// Distance quantiles of a plan built with `workers` workers, on the threaded or serial executor.
fn planned_quantiles(
    input: &Path,
    workers: usize,
    serial: bool,
) -> Option<viewer_format::PointDistanceQuantiles> {
    use crate::{
        las_source::{FileBytes, inspect_source, open_las_reader},
        pipeline::{
            clock::SystemClock,
            point_source::{LasFactory, LasPointSource, RangeSource},
            spool_store::MemSpoolStore,
        },
    };
    use viewer_convert_core::spatial::{build_spatial_plan_from_source, executor::SerialExecutor};

    let reader = open_las_reader(input).unwrap();
    let options = options().with_spatial_workers(workers).unwrap();
    let source = inspect_source(&reader)
        .unwrap()
        .for_options(options)
        .unwrap();
    let factory = LasFactory::new(FileBytes {
        path: input.to_owned(),
        len: fs::metadata(input).unwrap().len(),
    });
    let mut first = RangeSource::Las(LasPointSource::full(reader));
    let store = MemSpoolStore::new(true);
    let plan = if serial {
        build_spatial_plan_from_source(
            &mut first,
            &factory,
            &source,
            options,
            &store,
            &SystemClock,
            &SerialExecutor,
            &mut |_| true,
            false,
        )
    } else {
        build_spatial_plan_from_source(
            &mut first,
            &factory,
            &source,
            options,
            &store,
            &SystemClock,
            &crate::executor::ThreadedExecutor,
            &mut |_| true,
            false,
        )
    }
    .unwrap();
    plan.metadata.point_distance_quantiles()
}

#[test]
fn quantiles_do_not_depend_on_workers() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("in.las");
    write_las(
        &input,
        &cloud(150_000, [-40.0, -40.0, -40.0], 80.0, 5),
        vlr_wkt(),
    );
    let reference = planned_quantiles(&input, 1, false).unwrap();
    // Uniform random points in 80^3 with 150 000 points: the mean nearest-neighbour distance is
    // about 0.83, so the quantiles are of that order and ordered.
    assert!(
        0.05 < reference.p1 && reference.p1 <= reference.p5 && reference.p5 <= reference.p50,
        "{reference:?}"
    );
    assert!(reference.p50 > 0.4 && reference.p50 < 1.5, "{reference:?}");
    for workers in [1, 2, 8] {
        for serial in [false, true] {
            assert_eq!(
                planned_quantiles(&input, workers, serial),
                Some(reference),
                "workers {workers}, serial {serial}"
            );
        }
    }
}

#[test]
fn a_cloud_of_duplicates_stores_the_not_measured_value() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("in.las");
    write_las(&input, &vec![[3.0, 4.0, 5.0]; 100], vlr_wkt());
    let quantiles = planned_quantiles(&input, 1, false).unwrap();
    assert_eq!((quantiles.p1, quantiles.p5, quantiles.p50), (0.0, 0.0, 0.0));
}
