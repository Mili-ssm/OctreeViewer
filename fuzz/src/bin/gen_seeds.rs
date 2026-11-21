//! Writes tiny seeds into `corpus/`. Run from `fuzz/`: `cargo run --bin gen_seeds`.
//! Format seeds (V8 only): small `.oct` files, and `.oct`+`.octattrs` pairs in the split layout of
//! the target (u32 LE primary length, primary, attributes). Seeds are named by content.
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

const VERSION: FormatVersion = FormatVersion::V8;

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

/// V8 omits LOD columns and needs their LOD set and the distance quantiles.
fn with_v8_fields(metadata: FormatMetadata) -> FormatMetadata {
    metadata
        .with_point_distance_quantiles(
            viewer_format::PointDistanceQuantiles::new(0.25, 0.5, 1.5).unwrap(),
        )
        .with_lod_attribute_columns(vec![0])
        .with_display_colour_source(viewer_format::DisplayColourSource::None)
}

/// One tiny two-point block written with the real writer: primary and attributes companion.
fn write_sample() -> (Vec<u8>, Vec<u8>) {
    // Root at level 48 (side 2 m); its origin is the frame origin and the bounds are derived.
    let prefix = [5, 7, 9].map(|k| (1_u64 << 47) + k);
    let crs = viewer_format::CrsRecord::new(
        "EPSG".to_owned(),
        4978,
        viewer_format::CrsRaw::Wkt("GEODCRS[\"WGS 84\",ID[\"EPSG\",4978]]".to_owned()),
    )
    .unwrap();
    let source = viewer_format::SourceScaleOffset::new([0.001; 3], [0.0; 3]).unwrap();
    let cells = vec![(NodeId::new(0), viewer_format::NodeCell::new(prefix, 0.0625))];
    let frame = viewer_format::GlobalFrame::new(
        48,
        prefix,
        prefix.map(|p| viewer_format::cell_origin_quanta(48, p)),
        prefix.map(|p| viewer_format::cell_origin_quanta(48, p) + 1_000),
        crs,
        Some(source),
        cells,
    )
    .unwrap();
    let origin = frame.frame_origin();
    let bounds = frame.node_bounds(0, frame.root_prefix()).unwrap();
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
    .unwrap()
    .with_global_frame(frame)
    .unwrap();
    let metadata = with_v8_fields(metadata);
    let attribute =
        AttributeColumn::new(descriptor, AttributeValues::U16(vec![100, 200]), 2).unwrap();
    let block = SourcePointBlock::new(
        block_id,
        vec![[0.25, 0.5, 0.0], [1.5, 0.75, 1.0]],
        vec![ColorRgb::new(10, 20, 30), ColorRgb::new(40, 50, 60)],
        vec![attribute],
    );
    let mut writer = FormatWriter::new_split_for_version(
        VERSION,
        Cursor::new(Vec::new()),
        Cursor::new(Vec::new()),
        metadata,
        FormatWriterOptions::new(0.001, 1024).unwrap(),
    )
    .unwrap();
    writer.write_block(block).unwrap();
    let (primary, attributes) = writer.finish_pair().unwrap();
    (primary.into_inner(), attributes.unwrap().into_inner())
}

/// Paged chain: four nested nodes written with one level per page (`k` = 1), so the file has a
/// root page and three chained child pages. `page_target` picks the page size: 1 byte forces
/// `k` = 1 everywhere, the default keeps the whole chain in the root page.
fn write_paged_chain(page_target: Option<usize>) -> (Vec<u8>, Vec<u8>) {
    use viewer_format::{CrsRecord, GlobalFrame, NodeCell};
    let root_prefix = [5, 7, 9].map(|k| (1_u64 << 39) + k);
    let level = 40;
    let prefixes: Vec<[u64; 3]> = (0..4_u32)
        .scan(root_prefix, |prefix, depth| {
            let current = *prefix;
            *prefix = prefix.map(|p| (p << 1) | u64::from(depth % 2));
            Some(current)
        })
        .collect();
    let spacing = |depth: u32| {
        if depth == 3 {
            0.03125
        } else {
            2.0_f64.powi(1 - depth as i32)
        }
    };
    let cells = prefixes
        .iter()
        .enumerate()
        .map(|(i, p)| (NodeId::new(i as u64), NodeCell::new(*p, spacing(i as u32))))
        .collect();
    let origin = root_prefix.map(|p| viewer_format::cell_origin_quanta(level, p));
    let frame = GlobalFrame::new(
        level,
        root_prefix,
        origin,
        origin.map(|q| q + 1_000),
        CrsRecord::new("EPSG".to_owned(), 4978, viewer_format::CrsRaw::None).unwrap(),
        None,
        cells,
    )
    .unwrap();
    let descriptor = AttributeDescriptor::new("intensity", AttributeDataType::U16, 1).unwrap();
    let nodes: Vec<SpatialNode> = prefixes
        .iter()
        .enumerate()
        .map(|(i, p)| {
            SpatialNode::new(
                NodeId::new(i as u64),
                (i > 0).then(|| NodeId::new(i as u64 - 1)),
                i as u32,
                frame.node_bounds(i as u32, *p).unwrap(),
                2,
                0.25,
                vec![BlockId::new(i as u64)],
            )
            .unwrap()
        })
        .collect();
    let bounds: Vec<SpatialBounds> = nodes.iter().map(SpatialNode::bounds).collect();
    let hierarchy = SpatialHierarchy::new(nodes, vec![descriptor.clone()], Some(8)).unwrap();
    let metadata = FormatMetadata::new(
        DatasetMetadata::new(
            CoordinateFrame::new(frame.frame_origin()).unwrap(),
            hierarchy,
        ),
        None,
        Some("uniform voxel sampling".to_owned()),
        Vec::new(),
    )
    .unwrap()
    .with_global_frame(frame)
    .unwrap();
    let metadata = with_v8_fields(metadata);
    let mut options = FormatWriterOptions::new(0.001, 1024).unwrap();
    if let Some(target) = page_target {
        options = options.with_page_target_bytes(target);
    }
    let mut writer = FormatWriter::new_split_for_version(
        VERSION,
        Cursor::new(Vec::new()),
        Cursor::new(Vec::new()),
        metadata,
        options,
    )
    .unwrap();
    for id in 0..4_u64 {
        let attribute = AttributeColumn::new(
            descriptor.clone(),
            AttributeValues::U16(vec![id as u16, 9]),
            2,
        )
        .unwrap();
        writer
            .write_block(SourcePointBlock::new(
                BlockId::new(id),
                [0.25, 0.75]
                    .into_iter()
                    .map(|f| {
                        let b = bounds[id as usize];
                        std::array::from_fn(|axis| {
                            b.min()[axis] + (b.max()[axis] - b.min()[axis]) * f
                        })
                    })
                    .collect(),
                vec![ColorRgb::new(10, 20, 30), ColorRgb::new(40, 50, 60)],
                vec![attribute],
            ))
            .unwrap();
    }
    let (primary, attributes) = writer.finish_pair().unwrap();
    (primary.into_inner(), attributes.unwrap().into_inner())
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

    // Real V8 files, when present and small enough.
    let pairs = [
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

    // Chained pages for the paged-reader target; the format-reader target gets them too.
    let paged_dir = Path::new("corpus/fuzz_paged_reader");
    fs::create_dir_all(paged_dir).unwrap();
    // Only `seed-sample*` files are tracked (see `fuzz/.gitignore`).
    let (sample_primary, sample_attributes) = write_sample();
    fs::write(paged_dir.join("seed-sample"), &sample_primary).unwrap();
    fs::write(format_dir.join("seed-sample"), &sample_primary).unwrap();
    fs::write(
        format_dir.join("seed-sample-split"),
        join_split(&sample_primary, &sample_attributes),
    )
    .unwrap();
    for (name, (primary, attributes)) in [
        ("k5", write_paged_chain(None)),
        ("k1", write_paged_chain(Some(1))),
    ] {
        fs::write(
            paged_dir.join(format!("seed-sample-chain-{name}")),
            &primary,
        )
        .unwrap();
        let target = format!("seed-sample-paged-chain-{name}");
        fs::write(format_dir.join(&target), &primary).unwrap();
        fs::write(
            format_dir.join(format!("{target}-split")),
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
