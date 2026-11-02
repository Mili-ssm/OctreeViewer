//! `v7.4-pre`: global integer frame, node prefixes and spacing, CRS record and exact export.

mod common;

use std::io::Cursor;

use common::{global_metadata, options, prefix_of, repair_index_and_footer, root_origin};
use viewer_core::{
    BlockId, ColorRgb, CoordinateFrame, DatasetMetadata, NodeId, SpatialHierarchy, SpatialNode,
};
use viewer_format::{
    CrsRaw, CrsRecord, FormatError, FormatMetadata, FormatReader, FormatVersion, FormatWriter,
    FormatWriterOptions, GlobalFrame, NodeCell, QuantizationError, SourcePointBlock,
    SourceScaleOffset, cell_origin_quanta, quantize_coordinate,
};

const V74: FormatVersion = FormatVersion::V7_PRE4;
const UNITS: f64 = 1.0 / 32_768.0;

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 11
    }
}

fn block_of(id: u64, local: Vec<[f64; 3]>) -> SourcePointBlock {
    let colors = vec![ColorRgb::new(1, 2, 3); local.len()];
    SourcePointBlock::new(BlockId::new(id), local, colors, Vec::new())
}

fn write(metadata: FormatMetadata, blocks: Vec<SourcePointBlock>, error: f64) -> Vec<u8> {
    let options = FormatWriterOptions::new(error, 1 << 24).unwrap();
    let mut writer =
        FormatWriter::new_for_version(V74, Cursor::new(Vec::new()), metadata, options).unwrap();
    for block in blocks {
        writer.write_block(block).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

fn open(bytes: Vec<u8>) -> FormatReader<Cursor<Vec<u8>>> {
    FormatReader::new(Cursor::new(bytes)).unwrap()
}

fn wkt_crs() -> CrsRecord {
    CrsRecord::new(
        "EPSG".to_owned(),
        32632,
        CrsRaw::Wkt("PROJCS[\"WGS 84 / UTM zone 32N\",AUTHORITY[\"EPSG\",\"32632\"]]".to_owned()),
    )
    .unwrap()
}

/// Root at absolute level 40 (side 512 m) with two children; returns metadata and blocks.
fn tree(
    crs: CrsRecord,
    source: Option<SourceScaleOffset>,
) -> (FormatMetadata, Vec<SourcePointBlock>) {
    let root_prefix = [3, 4, 5].map(|k| (1_u64 << 39) + k);
    let child_bits = [[0_u64, 1, 0], [1, 0, 1]];
    let child_prefix =
        |bits: [u64; 3]| std::array::from_fn(|axis| (root_prefix[axis] << 1) | bits[axis]);
    let cells = vec![
        (NodeId::new(0), NodeCell::new(root_prefix, 2.0)),
        (
            NodeId::new(1),
            NodeCell::new(child_prefix(child_bits[0]), 0.125),
        ),
        (
            NodeId::new(2),
            NodeCell::new(child_prefix(child_bits[1]), 0.0625),
        ),
    ];
    let frame = GlobalFrame::new(
        40,
        root_prefix,
        root_origin(40, root_prefix),
        root_origin(40, root_prefix).map(|q| q + 100 * 32_768),
        crs,
        source,
        cells,
    )
    .unwrap();
    let node = |id: u64, parent: Option<u64>, depth: u32, prefix, count| {
        SpatialNode::new(
            NodeId::new(id),
            parent.map(NodeId::new),
            depth,
            frame.node_bounds(depth, prefix).unwrap(),
            count,
            0.0,
            vec![BlockId::new(id + 1)],
        )
        .unwrap()
    };
    let nodes = vec![
        node(0, None, 0, root_prefix, 3),
        node(1, Some(0), 1, child_prefix(child_bits[0]), 2),
        node(2, Some(0), 1, child_prefix(child_bits[1]), 2),
    ];
    let hierarchy = SpatialHierarchy::new(nodes, Vec::new(), Some(7)).unwrap();
    let metadata = FormatMetadata::new(
        DatasetMetadata::new(
            CoordinateFrame::new(frame.frame_origin()).unwrap(),
            hierarchy,
        ),
        None,
        Some("sample-grid:256;leaf:64000".to_owned()),
        Vec::new(),
    )
    .unwrap()
    .with_global_frame(frame)
    .unwrap();
    let blocks = vec![
        block_of(
            1,
            vec![[1.0, 2.0, 3.0], [500.0, 400.0, 300.0], [0.5, 0.25, 0.125]],
        ),
        block_of(2, vec![[10.25, 300.5, 20.0], [100.0, 400.0, 30.0]]),
        block_of(3, vec![[300.0, 20.0, 400.5], [450.0, 100.0, 450.0]]),
    ];
    (metadata, blocks)
}

#[test]
fn index_round_trips_root_prefixes_spacing_crs_and_source_scale() {
    let geotiff = CrsRecord::new(
        "EPSG".to_owned(),
        25830,
        CrsRaw::GeoTiff {
            key_directory: vec![1, 0, 1, 0, 0, 0, 0, 0],
            doubles: Vec::new(),
            ascii: b"ETRS89 / UTM 30N|".to_vec(),
        },
    )
    .unwrap();
    let source =
        SourceScaleOffset::new([0.01, 0.001, 0.0001], [262_000.0, 4_424_000.0, -3.5]).unwrap();
    for (crs, source, label) in [
        (wkt_crs(), Some(source), "wkt"),
        (geotiff, Some(source), "geotiff"),
    ] {
        let (metadata, blocks) = tree(crs.clone(), source);
        let written = metadata.global_frame().unwrap().clone();
        let bytes = write(metadata, blocks, 0.01);
        let reader = open(bytes);
        let read = reader.format_metadata();
        let frame = read
            .global_frame()
            .expect("v7.4-pre carries a global frame");
        assert_eq!(frame, &written, "{label}");
        assert_eq!(frame.root_level(), 40, "{label}");
        assert_eq!(frame.crs(), &crs, "{label}");
        assert_eq!(frame.source_scale_offset().copied(), source, "{label}");
        assert_eq!(
            read.crs().map(str::to_owned),
            crs.identity().map(|(a, c)| format!("{a}:{c}"))
        );
        let root = NodeId::new(0);
        assert_eq!(frame.node_cell(root).unwrap().spacing(), 2.0);
        assert_eq!(frame.node_cell(NodeId::new(2)).unwrap().spacing(), 0.0625);
        assert_eq!(read.sampling_policy(), Some("sample-grid:256;leaf:64000"));
        // Frame origin is the root origin: 3 * 512 m, 4 * 512 m, 5 * 512 m.
        assert_eq!(
            read.dataset().coordinate_frame().origin(),
            [1536.0, 2048.0, 2560.0]
        );
        assert_eq!(
            frame.frame_origin_quanta(),
            [1536, 2048, 2560].map(|m| m * 32_768)
        );
        let hierarchy = read.dataset().hierarchy();
        assert_eq!(hierarchy.node(root).unwrap().bounds().max(), [512.0; 3]);
        assert_eq!(
            hierarchy.node(NodeId::new(1)).unwrap().bounds().min(),
            [0.0, 256.0, 0.0]
        );
    }
}

#[test]
fn split_files_and_blocks_decode_to_exact_quanta() {
    let (metadata, blocks) = tree(wkt_crs(), None);
    let mut writer = FormatWriter::new_split_for_version(
        V74,
        Cursor::new(Vec::new()),
        Cursor::new(Vec::new()),
        metadata,
        options(),
    )
    .unwrap();
    for block in blocks {
        writer.write_block(block).unwrap();
    }
    let (primary, attributes) = writer.finish_pair().unwrap();
    let mut reader = FormatReader::new(Cursor::new(primary.into_inner())).unwrap();
    reader
        .attach_attributes(Cursor::new(attributes.unwrap().into_inner()))
        .unwrap();
    let origin = [1536.0, 2048.0, 2560.0];
    let quanta = reader.read_block_quanta(BlockId::new(2)).unwrap();
    assert_eq!(
        quanta[0],
        [10.25, 300.5, 20.0]
            .iter()
            .zip(origin)
            .map(|(local, o)| quantize_coordinate(o + local).unwrap())
            .collect::<Vec<_>>()[..]
    );
    let positions = reader.read_geometry(BlockId::new(2)).unwrap();
    assert_eq!(positions.points()[0].position, [10.25, 300.5, 20.0]);
}

#[test]
fn quantization_is_exact_with_a_frame_origin_near_4_4e6_m() {
    // Level 32 nodes have a side of 131 072 m: the cell [4 325 376, 4 456 448) holds 4.4e6 m.
    let level = 32;
    let prefix = [
        prefix_of(262_100.0, level),
        prefix_of(4_400_000.0, level),
        prefix_of(10.0, level),
    ];
    let base: [i64; 3] = prefix.map(|p| cell_origin_quanta(level, p));
    assert!(base[1] > 4_300_000 * 32_768);
    let mut random = Lcg(7);
    let mut expected = Vec::new();
    let mut local = Vec::new();
    for _ in 0..500 {
        let offsets = [(); 3].map(|()| (random.next() % (1 << 31)) as i64);
        let quanta: [i64; 3] = std::array::from_fn(|axis| base[axis] + offsets[axis]);
        expected.push(quanta);
        // Exact grid points: frame-local coordinates in f64 are multiples of 2^-15.
        local.push(std::array::from_fn(|axis| {
            (quanta[axis] - base[axis]) as f64 * UNITS
        }));
    }
    let build = || {
        global_metadata(
            level,
            prefix,
            vec![BlockId::new(1)],
            500,
            Vec::new(),
            wkt_crs(),
            None,
        )
    };
    // Max error 0.01 (the CLI default) holds at 4.4e6 m: the f32 rounding of values up to
    // 65 536 m local is below 0.004 m.
    let bytes = write(build(), vec![block_of(1, local.clone())], 0.01);
    let mut reader = open(bytes);
    assert_eq!(reader.read_block_quanta(BlockId::new(1)).unwrap(), expected);
    let positions = reader.read_positions(BlockId::new(1)).unwrap();
    for (point, local) in positions.points().iter().zip(&local) {
        assert_eq!(point.position, local.map(|value| value as f32));
    }
    // Off-grid positions pick the nearest quantum and stay within the grid error 2.6e-5 plus f32.
    let jittered: Vec<[f64; 3]> = local
        .iter()
        .map(|point| point.map(|value| value + (random.next() % 1000) as f64 * 1.0e-8))
        .collect();
    let options = FormatWriterOptions::new(0.01, 1 << 24).unwrap();
    let mut writer =
        FormatWriter::new_for_version(V74, Cursor::new(Vec::new()), build(), options).unwrap();
    writer.write_block(block_of(1, jittered.clone())).unwrap();
    let mut reader = open(writer.finish().unwrap().into_inner());
    let quanta = reader.read_block_quanta(BlockId::new(1)).unwrap();
    for ((quanta, jittered), expected) in quanta.iter().zip(&jittered).zip(&expected) {
        assert_eq!(quanta, expected, "{jittered:?}");
    }
    // The bound is real: a limit below the f32 rounding of the local coordinates is refused.
    let tight = FormatWriterOptions::new(1.0e-6, 1 << 24).unwrap();
    let mut writer =
        FormatWriter::new_for_version(V74, Cursor::new(Vec::new()), build(), tight).unwrap();
    assert!(matches!(
        writer.write_block(block_of(1, vec![local[0]])),
        Err(FormatError::Quantization(
            QuantizationError::ErrorLimitExceeded { .. }
        ))
    ));
}

#[test]
fn export_returns_exact_las_integers_for_common_scales() {
    let level = 32;
    let offset = [262_000.0, 4_424_000.0, 0.0];
    let prefix = [
        prefix_of(262_050.0, level),
        prefix_of(4_424_050.0, level),
        prefix_of(50.0, level),
    ];
    for scale in [0.01, 0.001, 0.0001] {
        let source = SourceScaleOffset::new([scale; 3], offset).unwrap();
        let metadata = global_metadata(
            level,
            prefix,
            vec![BlockId::new(1)],
            1000,
            Vec::new(),
            wkt_crs(),
            Some(source),
        );
        let origin = metadata.dataset().coordinate_frame().origin();
        let mut random = Lcg(11);
        let mut integers = Vec::new();
        let mut local = Vec::new();
        for _ in 0..1000 {
            let x: [i64; 3] =
                std::array::from_fn(|_| (random.next() % (100.0 / scale) as u64) as i64);
            integers.push(x);
            // Evaluated in f64 exactly as the spec states: X * scale + offset.
            local.push(std::array::from_fn(|axis| {
                x[axis] as f64 * scale + offset[axis] - origin[axis]
            }));
        }
        let mut reader = open(write(metadata, vec![block_of(1, local)], 0.01));
        let source = *reader
            .format_metadata()
            .global_frame()
            .unwrap()
            .source_scale_offset()
            .unwrap();
        let quanta = reader.read_block_quanta(BlockId::new(1)).unwrap();
        for (x, q) in integers.iter().zip(&quanta) {
            let exported: Vec<i64> = (0..3)
                .map(|axis| source.las_integer(axis, q[axis]).unwrap())
                .collect();
            assert_eq!(&exported[..], &x[..], "scale {scale}");
        }
    }
}

#[test]
fn nodes_larger_than_u32_are_split_in_cells_of_2_pow_32_quanta() {
    // Level 31: side 2^33 quanta = 262 144 m, two 2^32-quanta cells per axis.
    let level = 31;
    let prefix = [1_u64 << 30; 3];
    let cell = 131_072.0;
    let metadata = || {
        global_metadata(
            level,
            prefix,
            vec![BlockId::new(1), BlockId::new(2)],
            4,
            Vec::new(),
            wkt_crs(),
            None,
        )
    };
    let first = vec![[1.0, 2.0, 3.0], [100.0, 50.0, 25.0]];
    let second = vec![[cell + 0.5, 4.0, 8.0], [cell + 90_000.0, 5.0, 0.25]];
    // Local f32 values reach 2^18 m, so the limit has to allow its 0.016 m rounding.
    let bytes = write(
        metadata(),
        vec![block_of(1, first.clone()), block_of(2, second.clone())],
        0.1,
    );
    let mut reader = open(bytes);
    let origin = reader
        .format_metadata()
        .dataset()
        .coordinate_frame()
        .origin();
    for (id, points) in [(1, first), (2, second)] {
        let quanta = reader.read_block_quanta(BlockId::new(id)).unwrap();
        for (q, local) in quanta.iter().zip(&points) {
            let expected: Vec<i64> = (0..3)
                .map(|axis| quantize_coordinate(origin[axis] + local[axis]).unwrap())
                .collect();
            assert_eq!(&q[..], &expected[..], "block {id}");
        }
    }
    // One block cannot mix two cells, and a point outside the node is refused.
    let mixed = vec![[1.0, 2.0, 3.0], [cell + 1.0, 2.0, 3.0]];
    let options = FormatWriterOptions::new(0.1, 1 << 24).unwrap();
    let mut writer =
        FormatWriter::new_for_version(V74, Cursor::new(Vec::new()), metadata(), options).unwrap();
    assert!(matches!(
        writer.write_block(block_of(1, mixed)),
        Err(FormatError::Quantization(
            QuantizationError::BlockSpansCells
        ))
    ));
    let outside = vec![[-1.0, 2.0, 3.0]];
    assert!(matches!(
        writer.write_block(block_of(1, outside)),
        Err(FormatError::Quantization(
            QuantizationError::PointOutsideNode
        ))
    ));
}

#[test]
fn shallow_root_uses_integer_bounds_and_exact_quanta() {
    // Coordinates straddling 0 force level 0: the root origin is -2^48 CRS units, and the
    // frame-relative difference exceeds i64 in quanta once added to the side.
    let coordinates = [[-1.5, 0.25, 3.0], [1.5, -0.75, 0.0], [0.0625, 0.5, -2.0]];
    let local: Vec<[f64; 3]> = coordinates
        .iter()
        .map(|point| point.map(|value| value + 281_474_976_710_656.0))
        .collect();
    let metadata = || {
        global_metadata(
            0,
            [0; 3],
            (1..=3).map(BlockId::new).collect(),
            3,
            Vec::new(),
            wkt_crs(),
            None,
        )
    };
    // Points on different sides of 0 fall in different 2^32-quanta cells: one block each.
    let blocks = (0..3)
        .map(|index| block_of(index as u64 + 1, vec![local[index]]))
        .collect();
    let mut reader = open(write(metadata(), blocks, 1.0e9));
    let root = reader.format_metadata().dataset().hierarchy().root_id();
    let bounds = reader
        .format_metadata()
        .dataset()
        .hierarchy()
        .node(root)
        .unwrap()
        .bounds();
    assert_eq!(bounds.min(), [0.0; 3]);
    assert_eq!(bounds.max(), [562_949_953_421_312.0; 3]);
    assert_eq!(
        reader
            .format_metadata()
            .dataset()
            .coordinate_frame()
            .origin(),
        [-281_474_976_710_656.0; 3]
    );
    for (index, point) in coordinates.iter().enumerate() {
        let quanta = reader
            .read_block_quanta(BlockId::new(index as u64 + 1))
            .unwrap();
        let expected: Vec<i64> = point
            .iter()
            .map(|v| quantize_coordinate(*v).unwrap())
            .collect();
        assert_eq!(&quanta[0][..], &expected[..]);
    }
    // The f32 local frame is useless at that magnitude, and the writer says so.
    let mut writer =
        FormatWriter::new_for_version(V74, Cursor::new(Vec::new()), metadata(), options()).unwrap();
    assert!(matches!(
        writer.write_block(block_of(1, vec![local[0]])),
        Err(FormatError::Quantization(
            QuantizationError::ErrorLimitExceeded { .. }
        ))
    ));
}

/// Single-root metadata with an explicit frame origin (quanta) instead of the root corner.
fn metadata_with_origin(
    level: u32,
    prefix: [u64; 3],
    origin: [i64; 3],
    count: u64,
) -> Result<FormatMetadata, ()> {
    let cell = NodeCell::new(prefix, 2.0_f64.powi(-5));
    let frame = GlobalFrame::new(
        level,
        prefix,
        origin,
        origin,
        wkt_crs(),
        None,
        vec![(NodeId::new(0), cell)],
    )
    .map_err(|_| ())?;
    let bounds = frame.node_bounds(0, prefix).unwrap();
    let node = SpatialNode::new(
        NodeId::new(0),
        None,
        0,
        bounds,
        count,
        0.0,
        vec![BlockId::new(1)],
    )
    .unwrap();
    let hierarchy = SpatialHierarchy::new(vec![node], Vec::new(), Some(count)).unwrap();
    Ok(FormatMetadata::new(
        DatasetMetadata::new(
            CoordinateFrame::new(frame.frame_origin()).unwrap(),
            hierarchy,
        ),
        None,
        None,
        Vec::new(),
    )
    .unwrap()
    .with_global_frame(frame)
    .unwrap())
}

#[test]
fn shallow_root_with_frame_origin_at_the_cloud_minimum_writes_at_default_error() {
    // A level-0 root has its corner 2^48 m away; the frame origin sits at the cloud minimum instead.
    let min = [0.0, 5.0, 0.0];
    let origin = min.map(|v| quantize_coordinate(v).unwrap());
    let local = vec![[0.0; 3], [6.0, 7.5, 2.25], [3.0, 1.0, 0.5]];
    let metadata = metadata_with_origin(0, [0; 3], origin, 3).unwrap();
    let mut reader = open(write(metadata, vec![block_of(1, local.clone())], 0.01));
    let quanta = reader.read_block_quanta(BlockId::new(1)).unwrap();
    for (quanta, local) in quanta.iter().zip(&local) {
        let expected: Vec<i64> = (0..3)
            .map(|axis| quantize_coordinate(min[axis] + local[axis]).unwrap())
            .collect();
        assert_eq!(&quanta[..], &expected[..]);
    }
    let positions = reader.read_positions(BlockId::new(1)).unwrap();
    assert_eq!(positions.points()[1].position, [6.0, 7.5, 2.25]);
    assert_eq!(
        reader
            .format_metadata()
            .dataset()
            .coordinate_frame()
            .origin(),
        min
    );
}

#[test]
fn frame_origin_outside_the_root_node_is_refused() {
    let prefix = [1_u64 << 39; 3];
    let corner = root_origin(40, prefix);
    metadata_with_origin(40, prefix, corner, 1).unwrap();
    let inside = corner.map(|v| v + 32_768);
    metadata_with_origin(40, prefix, inside, 1).unwrap();
    let below = corner.map(|v| v - 1);
    assert!(metadata_with_origin(40, prefix, below, 1).is_err());
    let above = corner.map(|v| v + (1 << 24));
    assert!(metadata_with_origin(40, prefix, above, 1).is_err());
}

/// Offset and length of the index, read from the footer.
fn index_range(bytes: &[u8]) -> (usize, usize) {
    let footer = bytes.len() - 40;
    let at = |from: usize| {
        u64::from_le_bytes(bytes[footer + from..footer + from + 8].try_into().unwrap()) as usize
    };
    (at(8), at(16))
}

fn patched(bytes: &[u8], needle: &[u8], replacement: &[u8]) -> Result<(), FormatError> {
    patched_nth(bytes, needle, replacement, None)
}

/// Like `patched`, but with `Some(n)` the pattern may repeat and the `n`-th occurrence is patched.
fn patched_nth(
    bytes: &[u8],
    needle: &[u8],
    replacement: &[u8],
    nth: Option<usize>,
) -> Result<(), FormatError> {
    let mut bytes = bytes.to_vec();
    let (offset, length) = index_range(&bytes);
    let found: Vec<usize> = bytes[offset..offset + length]
        .windows(needle.len())
        .enumerate()
        .filter_map(|(at, window)| (window == needle).then_some(offset + at))
        .collect();
    if nth.is_none() {
        assert_eq!(
            found.len(),
            1,
            "pattern {needle:?} must be unique in the index: {found:?}"
        );
    }
    let found = [found[nth.unwrap_or(0)]];
    bytes[found[0]..found[0] + replacement.len()].copy_from_slice(replacement);
    repair_index_and_footer(&mut bytes);
    FormatReader::new(Cursor::new(bytes)).map(|_| ())
}

#[test]
fn reader_rejects_inconsistent_new_fields_even_with_valid_checksums() {
    let (metadata, blocks) = tree(wkt_crs(), None);
    let bytes = write(metadata, blocks, 0.01);
    FormatReader::new(Cursor::new(bytes.clone())).unwrap();
    let malformed = |result: Result<(), FormatError>, what: &str| {
        assert!(
            matches!(
                result,
                Err(FormatError::MalformedIndex(_) | FormatError::InvalidUtf8)
            ),
            "{what}: {result:?}"
        );
    };
    // Spacing of the root must be side / 256 = 2; a non power of two or a value of the wrong
    // exponent is refused.
    malformed(
        patched(&bytes, &2.0_f64.to_le_bytes(), &3.0_f64.to_le_bytes()),
        "spacing 3",
    );
    malformed(
        patched(&bytes, &2.0_f64.to_le_bytes(), &4.0_f64.to_le_bytes()),
        "spacing 4",
    );
    // Leaf spacing below one quantum.
    malformed(
        patched(
            &bytes,
            &0.0625_f64.to_le_bytes(),
            &2.0_f64.powi(-16).to_le_bytes(),
        ),
        "spacing below 2^-15",
    );
    // A child prefix that does not extend its parent's.
    // The first child's prefix is (2P+0, 2P+1, 2P+0); move it one cell away on x.
    let first: Vec<u8> = [(0_u64, 3_u64), (1, 4), (0, 5)]
        .iter()
        .flat_map(|(bit, k)| ((((1_u64 << 39) + k) << 1) | bit).to_le_bytes())
        .collect();
    let moved: Vec<u8> = {
        let mut moved = first.clone();
        moved[0] ^= 4;
        moved
    };
    malformed(patched(&bytes, &first, &moved), "child prefix");
    // The frame origin (here the root corner) comes first in the index; the block origin of the
    // root block, which must be the node origin, is its second occurrence.
    let origin: Vec<u8> = [1536_i64, 2048, 2560]
        .iter()
        .flat_map(|m| (m * 32_768).to_le_bytes())
        .collect();
    let mut shifted = origin.clone();
    shifted[0] ^= 1;
    malformed(
        patched_nth(&bytes, &origin[..], &shifted, Some(1)),
        "block origin",
    );
    // A frame origin outside the root node is refused; one inside it is accepted.
    malformed(
        patched_nth(&bytes, &origin[..], &shifted, Some(0)),
        "frame origin moved off the derived f64 slot",
    );
    // The data maximum (origin + 100 m) must not fall below the frame origin nor leave the root.
    let data_max: Vec<u8> = [1536_i64, 2048, 2560]
        .iter()
        .flat_map(|m| ((m + 100) * 32_768).to_le_bytes())
        .collect();
    let mut below = origin.clone();
    below[..8].copy_from_slice(&(1536_i64 * 32_768 - 1).to_le_bytes());
    below[8..16].copy_from_slice(&(2048_i64 * 32_768).to_le_bytes());
    malformed(
        patched(&bytes, &data_max, &below),
        "data maximum below origin",
    );
    let mut outside = data_max.clone();
    outside[..8].copy_from_slice(&(2048_i64 * 32_768 + (1 << 24)).to_le_bytes());
    malformed(
        patched(&bytes, &data_max, &outside),
        "data maximum outside root",
    );
    // Authority must be upper case, so a flipped letter is refused.
    malformed(
        patched(&bytes, b"EPSG\x78\x7f\0\0", b"EPsG\x78\x7f\0\0"),
        "authority case",
    );
}

#[test]
fn writer_rejects_metadata_that_breaks_the_spec() {
    let result = |spacing: f64, bound_shift: f64, crs_level: u32| {
        let prefix = [1_u64 << 39; 3];
        let frame = GlobalFrame::new(
            crs_level,
            prefix,
            root_origin(crs_level, prefix),
            root_origin(crs_level, prefix),
            wkt_crs(),
            None,
            vec![(NodeId::new(0), NodeCell::new(prefix, spacing))],
        )
        .map_err(FormatError::Metadata)?;
        let mut bounds = frame.node_bounds(0, prefix).unwrap();
        if bound_shift != 0.0 {
            bounds = viewer_core::SpatialBounds::new(
                bounds.min().map(|v| v + bound_shift),
                bounds.max().map(|v| v + bound_shift),
            )
            .unwrap();
        }
        let node = SpatialNode::new(NodeId::new(0), None, 0, bounds, 0, 0.0, vec![]).unwrap();
        let hierarchy = SpatialHierarchy::new(vec![node], Vec::new(), Some(0)).unwrap();
        let metadata = FormatMetadata::new(
            DatasetMetadata::new(
                CoordinateFrame::new(frame.frame_origin()).unwrap(),
                hierarchy,
            ),
            None,
            None,
            Vec::new(),
        )
        .unwrap()
        .with_global_frame(frame)
        .unwrap();
        FormatWriter::new_for_version(V74, Cursor::new(Vec::new()), metadata, options()).map(|_| ())
    };
    // Level 40 leaf: spacing 2^-3 is valid; 3, 0 and 2^-16 are not; bounds must be derived.
    result(0.125, 0.0, 40).unwrap();
    for spacing in [3.0, 0.0, 2.0_f64.powi(-16), f64::NAN, -0.125] {
        assert!(
            matches!(
                result(spacing, 0.0, 40),
                Err(FormatError::InvalidMetadata(_))
            ),
            "{spacing}"
        );
    }
    assert!(matches!(
        result(0.125, 1.0, 40),
        Err(FormatError::InvalidMetadata(_))
    ));
    // Root prefix 2^39 does not fit a level of 39 bits.
    assert!(matches!(
        result(0.125, 0.0, 39),
        Err(FormatError::Metadata(_))
    ));
}

#[test]
fn only_v7_4_takes_a_global_frame() {
    let (with_frame, _) = tree(wkt_crs(), None);
    let plain = common::metadata(0, Vec::new());
    for version in FormatVersion::WRITABLE {
        let given = if version == V74 { &plain } else { &with_frame };
        let outcome = FormatWriter::new_for_version(
            version,
            Cursor::new(Vec::new()),
            given.clone(),
            options(),
        );
        assert!(
            matches!(outcome, Err(FormatError::InvalidMetadata(_))),
            "{version}"
        );
    }
    assert!(V74.uses_global_frame());
    assert!(!FormatVersion::DEFAULT_OUTPUT.uses_global_frame());
    assert!(!V74.supports_block_checkpoint());
    assert!(FormatVersion::WRITABLE.contains(&V74));
}

#[test]
fn a_local_cloud_anchors_its_lattice_at_the_frame_origin() {
    let origin = [-3_000_i64, 120, -77];
    let local = CrsRecord::local();
    assert!(local.is_local());
    // Root level 40 (side 2^24 quanta) at prefix 0 starts exactly at the frame origin.
    let frame = GlobalFrame::new(
        40,
        [0; 3],
        origin,
        origin,
        local.clone(),
        None,
        vec![(NodeId::new(0), NodeCell::new([0; 3], 2.0))],
    )
    .unwrap();
    assert_eq!(frame.lattice_base(), origin);
    let bounds = frame.node_bounds(0, [0; 3]).unwrap();
    assert_eq!(bounds.min(), [0.0; 3]);
    assert_eq!(bounds.max(), [512.0; 3]);
    // The same prefix under a CRS identity is a global cell elsewhere, which cannot hold the origin.
    assert!(GlobalFrame::new(40, [0; 3], origin, origin, wkt_crs(), None, Vec::new()).is_err());
    assert!(!wkt_crs().is_local());
}

#[test]
fn data_extent_round_trips_and_the_viewer_frames_it_not_the_root_cube() {
    let (metadata, blocks) = tree(wkt_crs(), None);
    let bytes = write(metadata, blocks, 0.01);
    let reader = open(bytes);
    let metadata = reader.format_metadata();
    let frame = metadata.global_frame().unwrap();
    let origin_quanta = [1536_i64, 2048, 2560].map(|m| m * 32_768);
    assert_eq!(
        frame.data_max_quanta(),
        origin_quanta.map(|q| q + 100 * 32_768)
    );
    let world_min = [1536.0, 2048.0, 2560.0];
    let world_max = world_min.map(|m| m + 100.0);
    let data = frame.data_bounds().unwrap();
    assert_eq!((data.min(), data.max()), (world_min, world_max));
    // The viewer dataset keeps lattice node bounds and reports the tight extent separately.
    let viewer = metadata.viewer_dataset().unwrap();
    let stats = viewer.hierarchy().stats();
    assert_eq!(stats.root_bounds.min(), world_min);
    assert_eq!(stats.root_bounds.max(), world_min.map(|m| m + 512.0));
    assert_eq!(stats.data_bounds, data);
    // Other formats frame the root bounds.
    let v5 = metadata.dataset().hierarchy().stats();
    assert_eq!(v5.data_bounds, v5.root_bounds);
}

#[test]
fn data_extent_must_lie_between_the_frame_origin_and_the_root_end() {
    let origin = root_origin(40, [1 << 39; 3]);
    let build = |max: [i64; 3]| {
        GlobalFrame::new(40, [1 << 39; 3], origin, max, wkt_crs(), None, Vec::new()).map(|_| ())
    };
    assert!(build(origin).is_ok());
    assert!(build(origin.map(|q| q + (1 << 24) - 1)).is_ok());
    assert!(build(origin.map(|q| q + (1 << 24))).is_err());
    assert!(build([origin[0] - 1, origin[1], origin[2]]).is_err());
}
