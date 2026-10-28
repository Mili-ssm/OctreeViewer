//! Writes tiny seeds into `corpus/`. Run from `fuzz/`: `cargo run --bin gen_seeds`.
//! Format seeds: copies of small `.oct` files (monolithic) plus `.oct`+`.octattrs` pairs in the
//! split layout of the target (u32 LE primary length, primary, attributes).
//! LAS seeds: tiny LAS/LAZ files written by `las::Writer`.

use std::fs;
use std::io::Cursor;
use std::path::Path;

use las::{Builder, LazParallelism, Point, Writer, WriterOptions, point::Format};

const MAX_SEED: u64 = 1024 * 1024;

fn small(path: &Path) -> Option<Vec<u8>> {
    (fs::metadata(path).ok()?.len() <= MAX_SEED).then(|| fs::read(path).unwrap())
}

fn write_las(format: u8, compressed: bool, points: usize) -> Vec<u8> {
    let mut builder = Builder::from((1, 2));
    builder.point_format = Format::new(format).unwrap();
    builder.point_format.is_compressed = compressed;
    let header = builder.into_header().unwrap();
    let options = WriterOptions::default().with_laz_parallelism(LazParallelism::No);
    let mut writer = Writer::with_options(Cursor::new(Vec::new()), header, options).unwrap();
    for i in 0..points {
        let mut point = Point {
            x: i as f64,
            y: (i * 2) as f64,
            z: 1.0,
            ..Point::default()
        };
        if format != 0 {
            point.gps_time = Some(0.0);
        }
        if format == 3 {
            point.color = Some(las::Color::new(0, 0, 0));
        }
        writer.write_point(point).unwrap();
    }
    writer.close().unwrap();
    writer.into_inner().unwrap().into_inner()
}

fn main() {
    let root = Path::new("..");
    let format_dir = Path::new("corpus/fuzz_format_reader");
    let las_dir = Path::new("corpus/fuzz_las_preflight");
    fs::create_dir_all(format_dir).unwrap();
    fs::create_dir_all(las_dir).unwrap();

    let pairs = [
        ("sample", "examples/web-host/sample.oct", "examples/web-host/sample.octattrs"),
        ("sample-v7", "examples/web-host/sample-v7.oct", "examples/web-host/sample-v7.octattrs"),
        ("bunny", "data/samples/stanford_bunny_bun000_unstructured.oct", "data/samples/stanford_bunny_bun000_unstructured.octattrs"),
        ("fs-e430", "data/samples/fs_e430_n4008_18.oct", "data/samples/fs_e430_n4008_18.octattrs"),
        ("fs-e432", "data/samples/fs_e432_n4002_18.oct", "data/samples/fs_e432_n4002_18.octattrs"),
    ];
    for (name, primary, attributes) in pairs {
        let Some(primary) = small(&root.join(primary)) else {
            eprintln!("skip {name}: primary > 1 MiB or missing");
            continue;
        };
        fs::write(format_dir.join(format!("seed-{name}")), &primary).unwrap();
        if let Some(attributes) = small(&root.join(attributes)) {
            let mut joined = (primary.len() as u32).to_le_bytes().to_vec();
            joined.extend_from_slice(&primary);
            joined.extend_from_slice(&attributes);
            fs::write(format_dir.join(format!("seed-{name}-split")), joined).unwrap();
        }
    }

    for (name, format, compressed, points) in [
        ("las-f0", 0, false, 8),
        ("las-f3", 3, false, 8),
        ("laz-f0", 0, true, 8),
        ("laz-f3", 3, true, 64),
    ] {
        fs::write(las_dir.join(format!("seed-{name}")), write_las(format, compressed, points)).unwrap();
    }
    if let Ok(entries) = fs::read_dir(root.join("data/samples")) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "las")
                && let Some(bytes) = small(&path)
            {
                fs::write(las_dir.join(format!("seed-{}", path.file_stem().unwrap().to_string_lossy())), bytes).unwrap();
            }
        }
    }
}
