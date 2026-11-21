//! `v8.1-pre`: leaf blocks do not store the display colour; the reader derives it (`>> 8`) from the
//! column the header names, and the writer refuses a leaf whose colour is not that derivation.

mod common;

use std::io::Cursor;

use common::{repair_index_and_footer, root_origin};
use viewer_core::{
    AttributeColumn, AttributeDataType, AttributeDescriptor, AttributeValues, BlockId, ColorRgb,
    CoordinateFrame, DatasetMetadata, DatasetSource, NodeId, SpatialHierarchy, SpatialNode,
};
use viewer_format::{
    BlockPayloadKind, CrsRecord, DisplayColourSource, FormatError, FormatMetadata, FormatReader,
    FormatVersion, FormatWriter, FormatWriterOptions, GlobalFrame, NodeCell, PagedReader,
    ReadSelection, SelectedRead, SourcePointBlock,
};

const ROOT_LEVEL: u32 = 40;
const ROOT_PREFIX: [u64; 3] = [(1 << 39) + 3, (1 << 39) + 4, (1 << 39) + 5];
/// Node 0 is the LOD node; nodes 1 and 2 are its leaves. The file numbers blocks in node order.
const NODES: usize = 3;
const POINTS: usize = 3000;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Cloud {
    /// Schema `intensity`, `rgb` (`u16` x 3), `classification`.
    Colour,
    /// Schema `intensity`, `classification`: the colour comes from the intensity.
    IntensityOnly,
    /// Schema `classification` only: nothing to derive the colour from.
    Neither,
}

impl Cloud {
    fn schema(self) -> Vec<AttributeDescriptor> {
        let d = |name: &str, ty, n| AttributeDescriptor::new(name, ty, n).unwrap();
        let classification = d("classification", AttributeDataType::U8, 1);
        match self {
            Self::Colour => vec![
                d("intensity", AttributeDataType::U16, 1),
                d("rgb", AttributeDataType::U16, 3),
                classification,
            ],
            Self::IntensityOnly => vec![d("intensity", AttributeDataType::U16, 1), classification],
            Self::Neither => vec![classification],
        }
    }

    fn source(self) -> DisplayColourSource {
        match self {
            Self::Colour => DisplayColourSource::Rgb(1),
            Self::IntensityOnly => DisplayColourSource::Intensity(0),
            Self::Neither => DisplayColourSource::None,
        }
    }

    fn lod_columns(self) -> Vec<u32> {
        match self {
            Self::Colour => vec![0, 2],
            Self::IntensityOnly => vec![0, 1],
            Self::Neither => vec![0],
        }
    }
}

fn crs() -> CrsRecord {
    CrsRecord::new(
        "EPSG".to_owned(),
        32632,
        viewer_format::CrsRaw::Wkt(
            "PROJCS[\"WGS 84 / UTM zone 32N\",AUTHORITY[\"EPSG\",\"32632\"]]".into(),
        ),
    )
    .unwrap()
}

/// A pseudo-random but fixed value stream.
fn lcg(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    *state >> 33
}

/// The leaf display colour the converter produces, written independently of the format crate.
fn display_of(cloud: Cloud, intensity: &[u16], rgb: &[u16], row: usize) -> ColorRgb {
    match cloud {
        Cloud::Colour => ColorRgb::new(
            (rgb[row * 3] >> 8) as u8,
            (rgb[row * 3 + 1] >> 8) as u8,
            (rgb[row * 3 + 2] >> 8) as u8,
        ),
        Cloud::IntensityOnly => {
            let gray = (intensity[row] >> 8) as u8;
            ColorRgb::new(gray, gray, gray)
        }
        Cloud::Neither => ColorRgb::new(
            7_u8.wrapping_add(row as u8),
            9,
            200_u8.wrapping_sub(row as u8),
        ),
    }
}

/// `tamper` is a (node, row) whose colour is changed so it is no longer derivable.
fn dataset(
    cloud: Cloud,
    tamper: Option<(usize, usize)>,
) -> (FormatMetadata, Vec<SourcePointBlock>) {
    let depths = [0_u32, 1, 1];
    let prefixes: [[u64; 3]; NODES] = [
        ROOT_PREFIX,
        std::array::from_fn(|axis| ROOT_PREFIX[axis] << 1),
        std::array::from_fn(|axis| (ROOT_PREFIX[axis] << 1) | 1),
    ];
    let cells: Vec<_> = (0..NODES)
        .map(|i| {
            let spacing = if i == 0 {
                2.0_f64.powi(41 - ROOT_LEVEL as i32)
            } else {
                2.0_f64.powi(-5)
            };
            (NodeId::new(i as u64), NodeCell::new(prefixes[i], spacing))
        })
        .collect();
    let origin = root_origin(ROOT_LEVEL, ROOT_PREFIX);
    let frame = GlobalFrame::new(
        ROOT_LEVEL,
        ROOT_PREFIX,
        origin,
        origin.map(|q| q + 100 * 32_768),
        crs(),
        None,
        cells,
    )
    .unwrap();
    let schema = cloud.schema();
    let mut state = 12345_u64;
    let mut nodes = Vec::new();
    let mut blocks = Vec::new();
    for i in 0..NODES {
        let bounds = frame.node_bounds(depths[i], prefixes[i]).unwrap();
        nodes.push(
            SpatialNode::new(
                NodeId::new(i as u64),
                (i > 0).then(|| NodeId::new(0)),
                depths[i],
                bounds,
                POINTS as u64,
                0.0,
                vec![BlockId::new(i as u64 + 1)],
            )
            .unwrap(),
        );
        // Clustered points in blob-interleaved order, some repeated: the best source-order encoding
        // is `CompactOccupied` and sorting by Morton key is smaller, so the hybrid row order applies.
        let centres: Vec<[f64; 3]> = (0..30)
            .map(|_| std::array::from_fn(|_| 0.1 + (lcg(&mut state) % 800) as f64 / 1000.0))
            .collect();
        let mut fractions: Vec<[f64; 3]> = (0..POINTS)
            .map(|row| {
                let centre = centres[row % 30];
                std::array::from_fn(|axis| {
                    centre[axis] + (lcg(&mut state) % 1000) as f64 / 1_000_000.0
                })
            })
            .collect();
        for row in (0..POINTS).step_by(7) {
            fractions[POINTS - 1 - row / 7] = fractions[row];
        }
        let positions: Vec<[f64; 3]> = fractions
            .iter()
            .map(|fraction| {
                std::array::from_fn(|axis| {
                    bounds.min()[axis] + (bounds.max()[axis] - bounds.min()[axis]) * fraction[axis]
                })
            })
            .collect();
        let intensity: Vec<u16> = (0..POINTS).map(|_| lcg(&mut state) as u16).collect();
        let rgb: Vec<u16> = (0..POINTS * 3).map(|_| lcg(&mut state) as u16).collect();
        let classification: Vec<u8> = (0..POINTS).map(|_| lcg(&mut state) as u8).collect();
        let mut colors: Vec<ColorRgb> = (0..POINTS)
            .map(|row| {
                if i == 0 {
                    // LOD colours are cell means: nothing derives them.
                    ColorRgb::new(row as u8, 3, 250_u8.wrapping_sub(row as u8))
                } else {
                    display_of(cloud, &intensity, &rgb, row)
                }
            })
            .collect();
        if let Some((node, row)) = tamper.filter(|(node, _)| *node == i) {
            colors[row].g ^= 1;
            assert_eq!(node, i);
        }
        let columns = schema
            .iter()
            .map(|descriptor| {
                let values = match descriptor.name() {
                    "intensity" => AttributeValues::U16(intensity.clone()),
                    "rgb" => AttributeValues::U16(rgb.clone()),
                    _ => AttributeValues::U8(classification.clone()),
                };
                AttributeColumn::new(descriptor.clone(), values, POINTS).unwrap()
            })
            .collect();
        blocks.push(SourcePointBlock::new(
            BlockId::new(i as u64 + 1),
            positions,
            colors,
            columns,
        ));
    }
    let hierarchy = SpatialHierarchy::new(nodes, schema, Some(6)).unwrap();
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
    (metadata, blocks)
}

type Pair = (Vec<u8>, Vec<u8>);

fn write(
    version: FormatVersion,
    cloud: Cloud,
    tamper: Option<(usize, usize)>,
    source: Option<DisplayColourSource>,
) -> Result<Pair, FormatError> {
    write_with(
        version,
        cloud,
        tamper,
        source,
        FormatWriterOptions::new(0.01, 1 << 24).unwrap(),
    )
}

fn write_with(
    version: FormatVersion,
    cloud: Cloud,
    tamper: Option<(usize, usize)>,
    source: Option<DisplayColourSource>,
    options: FormatWriterOptions,
) -> Result<Pair, FormatError> {
    let (metadata, blocks) = dataset(cloud, tamper);
    let metadata = if matches!(version, FormatVersion::V8_PRE1) {
        let metadata = metadata
            .with_point_distance_quantiles(
                viewer_format::PointDistanceQuantiles::new(0.25, 0.5, 1.5).unwrap(),
            )
            .with_lod_attribute_columns(cloud.lod_columns());
        match source {
            Some(source) => metadata.with_display_colour_source(source),
            None => metadata,
        }
    } else {
        metadata
    };
    let mut writer = FormatWriter::new_split_for_version(
        version,
        Cursor::new(Vec::new()),
        Cursor::new(Vec::new()),
        metadata,
        options,
    )?;
    for block in blocks {
        writer.write_block(block)?;
    }
    let (primary, attributes) = writer.finish_pair()?;
    Ok((primary.into_inner(), attributes.unwrap().into_inner()))
}

fn v8(cloud: Cloud) -> Pair {
    write(FormatVersion::V8_PRE1, cloud, None, Some(cloud.source())).unwrap()
}

fn v7(cloud: Cloud) -> Pair {
    write(FormatVersion::V7, cloud, None, None).unwrap()
}

fn open(file: &Pair) -> FormatReader<Cursor<Vec<u8>>> {
    let mut reader = FormatReader::new(Cursor::new(file.0.clone())).unwrap();
    reader
        .attach_attributes(Cursor::new(file.1.clone()))
        .unwrap();
    reader
}

/// Every block reads back the same values as v7 through every API that returns the colour.
fn assert_reads_like_v7(cloud: Cloud) {
    let (mut old, mut new) = (open(&v7(cloud)), open(&v8(cloud)));
    for block in 0..NODES as u64 {
        let id = BlockId::new(block);
        assert_eq!(
            new.read_display_colors(id).unwrap(),
            old.read_display_colors(id).unwrap(),
            "{cloud:?} block {block} display colours"
        );
        assert_eq!(
            new.read_geometry(id).unwrap().points(),
            old.read_geometry(id).unwrap().points(),
            "{cloud:?} block {block} geometry"
        );
        for selection in [ReadSelection::DisplayColors, ReadSelection::Geometry] {
            let read = |reader: &mut FormatReader<Cursor<Vec<u8>>>| match reader
                .read_selected(id, selection, 1 << 30)
                .unwrap()
            {
                SelectedRead::DisplayColors(colors) => colors,
                SelectedRead::Geometry(batch) => {
                    batch.points().iter().map(|point| point.color).collect()
                }
                _ => unreachable!(),
            };
            assert_eq!(
                read(&mut new),
                read(&mut old),
                "{selection:?} block {block}"
            );
        }
        let (old_block, new_block) = (old.read_block(id).unwrap(), new.read_block(id).unwrap());
        assert_eq!(new_block.points().points(), old_block.points().points());
        for column in 0..cloud.schema().len() as u32 {
            // Leaves keep every attribute column; the LOD block keeps only its listed ones.
            let expected = block != 0 || cloud.lod_columns().contains(&column);
            assert_eq!(
                new.attribute_present(id, column).unwrap(),
                expected,
                "{cloud:?} block {block} column {column} presence"
            );
            if expected {
                assert_eq!(
                    new.read_attribute(id, column).unwrap(),
                    old.read_attribute(id, column).unwrap(),
                    "{cloud:?} block {block} column {column}"
                );
            }
        }
    }
}

fn stored_kinds(file: &Pair, block: u64) -> Vec<BlockPayloadKind> {
    open(file)
        .block_payload_storage_stats()
        .unwrap()
        .into_iter()
        .filter(|stat| stat.id == BlockId::new(block))
        .map(|stat| stat.kind)
        .collect()
}

/// The hybrid row order really permuted each leaf: its stored rows are a different order of the
/// source rows (the source columns are the reference, v7 sorts too), and the display colour of
/// every stored row still derives from that same row's own source column.
fn assert_leaves_were_resorted(file: &Pair) {
    let (_, sources) = dataset(Cloud::Colour, None);
    let mut reader = open(file);
    for leaf in [1_u64, 2] {
        let id = BlockId::new(leaf);
        let source = &sources[leaf as usize].attributes();
        let stored = |reader: &mut FormatReader<Cursor<Vec<u8>>>, column| {
            reader.read_attribute(id, column).unwrap()
        };
        let (intensity, rgb) = (stored(&mut reader, 0), stored(&mut reader, 1));
        let (AttributeValues::U16(source_intensity), AttributeValues::U16(stored_intensity)) =
            (source[0].values(), intensity.values())
        else {
            panic!("intensity is u16");
        };
        assert_ne!(
            stored_intensity, source_intensity,
            "leaf {leaf} kept source order"
        );
        let (mut a, mut b) = (stored_intensity.clone(), source_intensity.clone());
        a.sort_unstable();
        b.sort_unstable();
        assert_eq!(a, b, "leaf {leaf} rows are not a permutation");
        let AttributeValues::U16(stored_rgb) = rgb.values() else {
            panic!("rgb is u16");
        };
        let colors = reader.read_display_colors(id).unwrap();
        assert_eq!(colors.len(), POINTS);
        for (row, color) in colors.iter().enumerate() {
            assert_eq!(
                *color,
                display_of(Cloud::Colour, stored_intensity, stored_rgb, row),
                "leaf {leaf} row {row}"
            );
        }
    }
}

#[test]
fn leaf_display_reads_back_identical_to_v7() {
    assert_reads_like_v7(Cloud::Colour);
    let file = v8(Cloud::Colour);
    assert_leaves_were_resorted(&file);
    for leaf in [1, 2] {
        assert!(!stored_kinds(&file, leaf).contains(&BlockPayloadKind::DisplayColor));
    }
    // The LOD block keeps its own colour.
    assert!(stored_kinds(&file, 0).contains(&BlockPayloadKind::DisplayColor));
    let paged = PagedReader::open(Cursor::new(file.0)).unwrap();
    assert_eq!(
        paged.header().display_colour_source(),
        Some(DisplayColourSource::Rgb(1))
    );
}

#[test]
fn leaf_display_from_intensity_without_colour() {
    assert_reads_like_v7(Cloud::IntensityOnly);
    let file = v8(Cloud::IntensityOnly);
    assert!(!stored_kinds(&file, 1).contains(&BlockPayloadKind::DisplayColor));
    // r = g = b from the intensity, and the intensity itself is recomposed unchanged.
    let mut reader = open(&file);
    let colors = reader.read_display_colors(BlockId::new(1)).unwrap();
    assert!(colors.iter().all(|c| c.r == c.g && c.g == c.b));
    let paged = PagedReader::open(Cursor::new(file.0)).unwrap();
    assert_eq!(
        paged.header().display_colour_source(),
        Some(DisplayColourSource::Intensity(0))
    );
}

#[test]
fn a_cloud_without_a_source_column_keeps_the_stored_colour() {
    assert_reads_like_v7(Cloud::Neither);
    let file = v8(Cloud::Neither);
    for block in 0..NODES as u64 {
        assert!(stored_kinds(&file, block).contains(&BlockPayloadKind::DisplayColor));
    }
}

#[test]
fn the_read_plan_counts_the_source_column() {
    let mut reader = open(&v8(Cloud::Colour));
    let leaf = BlockId::new(1);
    let plan = reader
        .plan_read(leaf, ReadSelection::DisplayColors)
        .unwrap();
    // One range: the 16-bit RGB column (6 bytes per point), not a stored colour (3 bytes per point).
    assert_eq!(plan.ranges().len(), 1);
    assert_eq!(plan.memory().decoded_bytes(), (POINTS * 6) as u64);
    assert_eq!(plan.memory().output_bytes(), (POINTS * 3) as u64);
    let lod = reader
        .plan_read(BlockId::new(0), ReadSelection::DisplayColors)
        .unwrap();
    assert_eq!(lod.memory().decoded_bytes(), (POINTS * 3) as u64);
    // A budget below the plan's peak refuses the read.
    let peak = plan.memory().peak_bytes();
    assert!(
        reader
            .read_selected(leaf, ReadSelection::DisplayColors, peak - 1)
            .is_err()
    );
    assert!(
        reader
            .read_selected(leaf, ReadSelection::DisplayColors, peak)
            .is_ok()
    );
}

#[test]
fn display_mismatch_is_rejected_by_the_writer() {
    for cloud in [Cloud::Colour, Cloud::IntensityOnly] {
        // Node 2 is block 3; its first row, then its last row.
        for row in [5, POINTS - 1] {
            let result = write(
                FormatVersion::V8_PRE1,
                cloud,
                Some((2, row)),
                Some(cloud.source()),
            );
            assert!(
                matches!(result, Err(FormatError::DisplayColourNotDerivable(id)) if id == BlockId::new(3)),
                "{cloud:?} row {row}: {result:?}"
            );
        }
        let result = write(
            FormatVersion::V8_PRE1,
            cloud,
            Some((1, POINTS - 1)),
            Some(cloud.source()),
        );
        assert!(
            matches!(result, Err(FormatError::DisplayColourNotDerivable(id)) if id == BlockId::new(2)),
            "{cloud:?} first leaf: {result:?}"
        );
        // The LOD block stores its colour, so nothing derives it and any value is accepted.
        assert!(
            write(
                FormatVersion::V8_PRE1,
                cloud,
                Some((0, 5)),
                Some(cloud.source())
            )
            .is_ok()
        );
    }
    // The same cloud with the colour stored is accepted, and v7 never checks.
    assert!(
        write(
            FormatVersion::V8_PRE1,
            Cloud::Colour,
            Some((2, 5)),
            Some(DisplayColourSource::None)
        )
        .is_ok()
    );
    assert!(write(FormatVersion::V7, Cloud::Colour, Some((2, 5)), None).is_ok());
}

#[test]
fn the_writer_rejects_a_wrong_or_missing_source() {
    let bad = [
        DisplayColourSource::Rgb(0),
        DisplayColourSource::Rgb(9),
        DisplayColourSource::Intensity(1),
        DisplayColourSource::Intensity(2),
    ];
    for source in bad {
        let result = write(FormatVersion::V8_PRE1, Cloud::Colour, None, Some(source));
        assert!(
            matches!(result, Err(FormatError::InvalidMetadata(_))),
            "{source:?}: {result:?}"
        );
    }
    assert!(matches!(
        write(FormatVersion::V8_PRE1, Cloud::Colour, None, None),
        Err(FormatError::InvalidMetadata(_))
    ));
    // A real 16-bit column whose high byte is not the stored colour, and a u8 column.
    let result = write(
        FormatVersion::V8_PRE1,
        Cloud::Colour,
        None,
        Some(DisplayColourSource::Intensity(0)),
    );
    assert!(
        matches!(result, Err(FormatError::DisplayColourNotDerivable(_))),
        "intensity on the colour cloud: {result:?}"
    );
    let result = write(
        FormatVersion::V8_PRE1,
        Cloud::IntensityOnly,
        None,
        Some(DisplayColourSource::Rgb(1)),
    );
    assert!(
        matches!(result, Err(FormatError::InvalidMetadata(_))),
        "rgb naming the u8 column: {result:?}"
    );
    // Revisions that store the colour take no source.
    let (metadata, _) = dataset(Cloud::Colour, None);
    let result = FormatWriter::new_split_for_version(
        FormatVersion::V7,
        Cursor::new(Vec::new()),
        Cursor::new(Vec::new()),
        metadata.with_display_colour_source(DisplayColourSource::Rgb(1)),
        FormatWriterOptions::new(0.01, 1 << 24).unwrap(),
    );
    assert!(matches!(result, Err(FormatError::InvalidMetadata(_))));
}

/// Offset of the source field: it follows the LOD list, which follows the three f64 quantiles.
fn source_offset(bytes: &[u8], lod_len: usize) -> usize {
    let footer = bytes.len() - 40;
    let index = u64::from_le_bytes(bytes[footer + 8..footer + 16].try_into().unwrap()) as usize;
    let length = u64::from_le_bytes(bytes[footer + 16..footer + 24].try_into().unwrap()) as usize;
    let reader = PagedReader::open(Cursor::new(bytes.to_vec())).unwrap();
    let spacing = reader.header().reference_spacing().to_le_bytes();
    bytes[index..index + length]
        .windows(8)
        .position(|window| window == spacing)
        .unwrap()
        + index
        + 8
        + 24
        + 4
        + 4 * lod_len
}

#[test]
fn corrupt_display_source_is_rejected_on_read() {
    let primary = v8(Cloud::Colour).0;
    let at = source_offset(&primary, Cloud::Colour.lod_columns().len());
    assert_eq!(primary[at], 1);
    assert_eq!(&primary[at + 1..at + 5], &1_u32.to_le_bytes());
    let patch = |kind: u8, index: u32| {
        let mut bytes = vec![kind];
        bytes.extend_from_slice(&index.to_le_bytes());
        bytes
    };
    let patches = [
        ("index outside the schema", patch(1, 3)),
        ("huge index", patch(2, u32::MAX)),
        ("rgb names the intensity", patch(1, 0)),
        ("intensity names the rgb", patch(2, 1)),
        ("intensity names a u8 column", patch(2, 2)),
        ("unknown kind", patch(3, 0)),
        ("none with an index", patch(0, 1)),
    ];
    for (name, patch) in patches {
        let mut bytes = primary.clone();
        bytes[at..at + 5].copy_from_slice(&patch);
        repair_index_and_footer(&mut bytes);
        assert!(
            matches!(
                PagedReader::open(Cursor::new(bytes.clone())),
                Err(FormatError::MalformedIndex(_))
            ),
            "{name}"
        );
        assert!(FormatReader::new(Cursor::new(bytes)).is_err(), "{name}");
    }
    for cut in (0..primary.len()).step_by(61) {
        assert!(
            PagedReader::open(Cursor::new(primary[..cut].to_vec())).is_err(),
            "{cut}"
        );
    }
}

#[test]
fn the_file_is_smaller_by_the_leaf_display_colour() {
    let (old, new) = (v7(Cloud::Colour), v8(Cloud::Colour));
    let omitted = |file: &Pair| -> u64 {
        open(file)
            .block_payload_storage_stats()
            .unwrap()
            .into_iter()
            .filter(|stat| stat.kind == BlockPayloadKind::DisplayColor)
            .map(|stat| stat.stored_bytes)
            .sum()
    };
    assert!(omitted(&old) > omitted(&new));
    assert!(new.1.len() < old.1.len());
}

#[test]
fn parallel_and_budgeted_reads_return_the_v7_display_colours() {
    use std::ops::ControlFlow;
    use viewer_format::{ParallelReadStatus, ReadBudget, read_selected_parallel};

    for cloud in [Cloud::Colour, Cloud::IntensityOnly] {
        let file = v8(cloud);
        let mut old = open(&v7(cloud));
        let mut readers = vec![open(&file)];
        let mut fork = readers[0].fork_reader(Cursor::new(file.0.clone()));
        fork.attach_attributes(Cursor::new(file.1.clone())).unwrap();
        readers.push(fork);
        let requests: Vec<_> = (0..NODES as u64)
            .map(|block| (BlockId::new(block), ReadSelection::DisplayColors))
            .collect();
        let budget = ReadBudget::new(u64::MAX);
        let mut seen = 0;
        let progress = read_selected_parallel(&mut readers, &requests, &budget, |index, read| {
            let SelectedRead::DisplayColors(colors) = read else {
                panic!("wrong variant at {index}");
            };
            let expected = old.read_display_colors(requests[index].0).unwrap();
            assert_eq!(*colors, expected, "{cloud:?} request {index}");
            seen += 1;
            ControlFlow::Continue(())
        })
        .unwrap();
        assert_eq!(
            (seen, progress.status),
            (NODES, ParallelReadStatus::Complete)
        );
        assert_eq!(budget.reserved_bytes(), 0);
        for (id, selection) in requests {
            let read = readers[0]
                .try_read_selected(id, selection, &budget)
                .unwrap()
                .expect("budget is unlimited");
            let SelectedRead::DisplayColors(colors) = read.selected() else {
                panic!("wrong variant");
            };
            assert_eq!(*colors, old.read_display_colors(id).unwrap());
        }
    }
}
