//! Writes tiny seeds into `corpus/`. Run from `fuzz/`: `cargo run --bin gen_seeds`.
//! Format seeds: copies of small `.oct` files (monolithic) plus `.oct`+`.octattrs` pairs in the
//! split layout of the target (u32 LE primary length, primary, attributes).
//! LAS seeds: tiny LAS/LAZ files written by `las::Writer`.

use std::fs;
use std::io::Cursor;
use std::path::Path;

use las::{Builder, LazParallelism, Point, Vlr, Writer, WriterOptions, point::Format};
use viewer_core::{
    AttributeColumn, AttributeDataType, AttributeDescriptor, AttributeValues, BlockId, ColorRgb,
    CoordinateFrame, DatasetMetadata, NodeId, SpatialBounds, SpatialHierarchy, SpatialNode,
};
use viewer_format::{
    FormatMetadata, FormatVersion, FormatWriter, FormatWriterOptions, SourcePointBlock,
};

const MAX_SEED: u64 = 1024 * 1024;

fn small(path: &Path) -> Option<Vec<u8>> {
    (fs::metadata(path).ok()?.len() <= MAX_SEED).then(|| fs::read(path).unwrap())
}

fn write_las(format: u8, compressed: bool, points: usize) -> Vec<u8> {
    write_las_version((1, 2), format, compressed, points, false)
}

fn write_las_version(
    version: (u8, u8),
    format: u8,
    compressed: bool,
    points: usize,
    evlr: bool,
) -> Vec<u8> {
    let mut builder = Builder::from(version);
    if evlr {
        builder.evlrs.push(Vlr {
            user_id: "fuzz".to_owned(),
            record_id: 1,
            description: String::new(),
            data: vec![0xAB; 16],
        });
    }
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
        if matches!(format, 2 | 3 | 5 | 7 | 8) {
            point.color = Some(las::Color::new(0, 0, 0));
        }
        if format == 8 {
            point.nir = Some(0);
        }
        writer.write_point(point).unwrap();
    }
    writer.close().unwrap();
    writer.into_inner().unwrap().into_inner()
}

/// One tiny two-point block written with the real writer for `version`: monolithic and split.
fn write_revision(version: FormatVersion) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let origin = [1_000.0, -2_000.0, 10.0];
    let bounds =
        SpatialBounds::new(origin, [origin[0] + 2.0, origin[1] + 1.0, origin[2] + 1.0]).unwrap();
    let block_id = BlockId::new(7);
    let root = SpatialNode::new(NodeId::new(0), None, 0, bounds, 2, 0.25, vec![block_id]).unwrap();
    let descriptor = AttributeDescriptor::new("intensity", AttributeDataType::U16, 1).unwrap();
    let hierarchy = SpatialHierarchy::new(vec![root], vec![descriptor.clone()], Some(2)).unwrap();
    let metadata = FormatMetadata::new(
        DatasetMetadata::new(CoordinateFrame::new(origin).unwrap(), hierarchy),
        Some("EPSG:4978".to_owned()),
        Some("uniform voxel sampling".to_owned()),
        Vec::new(),
    )
    .unwrap();
    let block = || {
        let attribute =
            AttributeColumn::new(descriptor.clone(), AttributeValues::U16(vec![100, 200]), 2)
                .unwrap();
        SourcePointBlock::new(
            block_id,
            vec![[0.25, 0.5, 0.0], [1.5, 0.75, 1.0]],
            vec![ColorRgb::new(10, 20, 30), ColorRgb::new(40, 50, 60)],
            vec![attribute],
        )
    };
    let options = || FormatWriterOptions::new(0.001, 1024).unwrap();

    let mut writer = FormatWriter::new_for_version(
        version,
        Cursor::new(Vec::new()),
        metadata.clone(),
        options(),
    )
    .unwrap();
    writer.write_block(block()).unwrap();
    let monolithic = writer.finish().unwrap().into_inner();

    let mut writer = FormatWriter::new_split_for_version(
        version,
        Cursor::new(Vec::new()),
        Cursor::new(Vec::new()),
        metadata,
        options(),
    )
    .unwrap();
    writer.write_block(block()).unwrap();
    let (primary, attributes) = writer.finish_pair().unwrap();
    (
        monolithic,
        primary.into_inner(),
        attributes.unwrap().into_inner(),
    )
}

fn join_split(primary: &[u8], attributes: &[u8]) -> Vec<u8> {
    let mut joined = (primary.len() as u32).to_le_bytes().to_vec();
    joined.extend_from_slice(primary);
    joined.extend_from_slice(attributes);
    joined
}

fn main() {
    let root = Path::new("..");
    let format_dir = Path::new("corpus/fuzz_format_reader");
    let las_dir = Path::new("corpus/fuzz_las_preflight");
    fs::create_dir_all(format_dir).unwrap();
    fs::create_dir_all(las_dir).unwrap();

    let pairs = [
        (
            "sample",
            "examples/web-host/sample.oct",
            "examples/web-host/sample.octattrs",
        ),
        (
            "sample-v7",
            "examples/web-host/sample-v7.oct",
            "examples/web-host/sample-v7.octattrs",
        ),
        (
            "bunny",
            "data/samples/stanford_bunny_bun000_unstructured.oct",
            "data/samples/stanford_bunny_bun000_unstructured.octattrs",
        ),
        (
            "fs-e430",
            "data/samples/fs_e430_n4008_18.oct",
            "data/samples/fs_e430_n4008_18.octattrs",
        ),
        (
            "fs-e432",
            "data/samples/fs_e432_n4002_18.oct",
            "data/samples/fs_e432_n4002_18.octattrs",
        ),
    ];
    for (name, primary, attributes) in pairs {
        let Some(primary) = small(&root.join(primary)) else {
            eprintln!("skip {name}: primary > 1 MiB or missing");
            continue;
        };
        fs::write(format_dir.join(format!("seed-{name}")), &primary).unwrap();
        if let Some(attributes) = small(&root.join(attributes)) {
            fs::write(
                format_dir.join(format!("seed-{name}-split")),
                join_split(&primary, &attributes),
            )
            .unwrap();
        }
    }

    for (index, version) in FormatVersion::WRITABLE.into_iter().enumerate() {
        let (monolithic, primary, attributes) = write_revision(version);
        fs::write(
            format_dir.join(format!("seed-sample-rev{index}")),
            monolithic,
        )
        .unwrap();
        fs::write(
            format_dir.join(format!("seed-sample-rev{index}-split")),
            join_split(&primary, &attributes),
        )
        .unwrap();
    }

    for format in [6, 7, 8] {
        for compressed in [false, true] {
            for evlr in [false, true] {
                let name = format!(
                    "{}-14-f{format}{}",
                    if compressed { "laz" } else { "las" },
                    if evlr { "-evlr" } else { "" }
                );
                let bytes = write_las_version((1, 4), format, compressed, 8, evlr);
                assert!(bytes.len() <= 64 * 1024, "{name} too large");
                fs::write(las_dir.join(format!("seed-{name}")), bytes).unwrap();
            }
        }
    }

    for (name, format, compressed, points) in [
        ("las-f0", 0, false, 8),
        ("las-f3", 3, false, 8),
        ("laz-f0", 0, true, 8),
        ("laz-f3", 3, true, 64),
    ] {
        fs::write(
            las_dir.join(format!("seed-{name}")),
            write_las(format, compressed, points),
        )
        .unwrap();
    }
    if let Ok(entries) = fs::read_dir(root.join("data/samples")) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "las")
                && let Some(bytes) = small(&path)
            {
                fs::write(
                    las_dir.join(format!(
                        "seed-{}",
                        path.file_stem().unwrap().to_string_lossy()
                    )),
                    bytes,
                )
                .unwrap();
            }
        }
    }
}
