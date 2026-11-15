//! Systematic corruption sweeps over every writable revision: truncation at every length,
//! one bit flipped at every offset, and extreme values in every index/footer integer field.

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::io::Cursor;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::{global_metadata, options};
use viewer_core::{
    AttributeColumn, AttributeDataType, AttributeDescriptor, AttributeValues, BlockId, ColorRgb,
    CoordinateFrame, DatasetMetadata, NodeId, PointBatch, SpatialBounds, SpatialHierarchy,
    SpatialNode,
};
use viewer_format::{
    CrsRaw, CrsRecord, FormatError, FormatMetadata, FormatReader, FormatReaderLimits,
    FormatVersion, FormatWriter, SourcePointBlock, SourceScaleOffset, crc32,
};

const BLOCKS: u64 = 3;
const POINTS_PER_BLOCK: usize = 24;
const FOOTER: usize = 40;
/// Small enough that any allocation sized from an unchecked field would exceed it.
const LIMIT_BYTES: u64 = 64 * 1024;
const ALLOCATION_CEILING: usize = 1024 * 1024;

/// Records the largest single allocation requested by the current thread.
struct TrackingAllocator;

thread_local! {
    static LARGEST_ALLOCATION: Cell<usize> = const { Cell::new(0) };
}

fn note_allocation(size: usize) {
    let _ = LARGEST_ALLOCATION.try_with(|largest| largest.set(largest.get().max(size)));
}

// SAFETY: every call is forwarded unchanged to the system allocator.
unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note_allocation(layout.size());
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note_allocation(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note_allocation(new_size);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: TrackingAllocator = TrackingAllocator;

fn limits() -> FormatReaderLimits {
    FormatReaderLimits::new(LIMIT_BYTES, LIMIT_BYTES, 16, 16, 16, 16, 256).unwrap()
}

fn block_id(index: u64) -> BlockId {
    BlockId::new(index + 1)
}

/// Deterministic pseudo-random stream so the fixture needs no extra dependency.
fn stream(seed: u64) -> impl Iterator<Item = u64> {
    let mut state = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
    std::iter::repeat_with(move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    })
}

/// Few columns keep the sweeps fast; they still cover constant, dictionary, delta-friendly
/// and incompressible data across integer widths and floats.
fn descriptors() -> Vec<AttributeDescriptor> {
    [
        ("const_u8", AttributeDataType::U8),
        ("dictionary_u16", AttributeDataType::U16),
        ("ramp_i32", AttributeDataType::I32),
        ("random_u64", AttributeDataType::U64),
        ("random_f32", AttributeDataType::F32),
    ]
    .into_iter()
    .map(|(name, kind)| AttributeDescriptor::new(name, kind, 1).unwrap())
    .collect()
}

fn column_values(block: u64, column: usize, count: usize) -> AttributeValues {
    let mut random = stream(block * 100 + column as u64);
    match column {
        0 => AttributeValues::U8(vec![7; count]),
        1 => AttributeValues::U16((0..count).map(|r| (r % 4) as u16).collect()),
        2 => AttributeValues::I32((0..count).map(|r| r as i32 * 5 - 70).collect()),
        3 => AttributeValues::U64((0..count).map(|_| random.next().unwrap()).collect()),
        _ => AttributeValues::F32(
            (0..count)
                .map(|_| random.next().unwrap() as u16 as f32 / 8.0)
                .collect(),
        ),
    }
}

fn source_block(index: u64) -> SourcePointBlock {
    let mut random = stream(index + 1);
    let mut unit = move || (random.next().unwrap() >> 11) as f64 / (1_u64 << 53) as f64;
    let points = (0..POINTS_PER_BLOCK)
        .map(|_| [unit(), unit(), unit()])
        .collect();
    let mut colors = stream(index + 50);
    let colors = (0..POINTS_PER_BLOCK)
        .map(|_| {
            let v = colors.next().unwrap();
            ColorRgb::new(v as u8, (v >> 8) as u8, (v >> 16) as u8)
        })
        .collect();
    let attributes = descriptors()
        .into_iter()
        .enumerate()
        .map(|(column, descriptor)| {
            let values = column_values(index, column, POINTS_PER_BLOCK);
            AttributeColumn::new(descriptor, values, POINTS_PER_BLOCK).unwrap()
        })
        .collect();
    SourcePointBlock::new(block_id(index), points, colors, attributes)
}

/// `v7.4-pre` root at level 48 (side 2 CRS units, origin (10, 14, 18) m), so the fixture's
/// local points in [0, 1) lie inside it. Carries a WKT CRS and a source scale/offset.
fn global_fixture_metadata() -> FormatMetadata {
    let total = BLOCKS * POINTS_PER_BLOCK as u64;
    let prefix = [5, 7, 9].map(|k| (1_u64 << 47) + k);
    let crs = CrsRecord::new(
        "EPSG".to_owned(),
        32632,
        CrsRaw::Wkt("PROJCS[\"WGS 84 / UTM zone 32N\"]".to_owned()),
    )
    .unwrap();
    let source =
        SourceScaleOffset::new([0.01, 0.01, 0.001], [500_000.0, 4_000_000.0, 0.0]).unwrap();
    global_metadata(
        48,
        prefix,
        (0..BLOCKS).map(block_id).collect(),
        total,
        descriptors(),
        crs,
        Some(source),
    )
}

fn multi_block_metadata_for(version: FormatVersion) -> FormatMetadata {
    if version.uses_global_frame() {
        global_fixture_metadata()
    } else {
        multi_block_metadata()
    }
}

fn multi_block_metadata() -> FormatMetadata {
    let total = BLOCKS * POINTS_PER_BLOCK as u64;
    let bounds = SpatialBounds::new([0.0; 3], [1.0; 3]).unwrap();
    let node = SpatialNode::new(
        NodeId::new(0),
        None,
        0,
        bounds,
        total,
        0.0,
        (0..BLOCKS).map(block_id).collect(),
    )
    .unwrap();
    let hierarchy = SpatialHierarchy::new(vec![node], descriptors(), Some(total)).unwrap();
    FormatMetadata::new(
        DatasetMetadata::new(CoordinateFrame::new([0.0; 3]).unwrap(), hierarchy),
        None,
        None,
        Vec::new(),
    )
    .unwrap()
}

/// Files of one fixture: a monolithic file or a split pair.
#[derive(Clone)]
struct Fixture {
    primary: Vec<u8>,
    attributes: Option<Vec<u8>>,
}

fn monolithic(version: FormatVersion) -> Fixture {
    let mut writer = FormatWriter::new_for_version(
        version,
        Cursor::new(Vec::new()),
        multi_block_metadata_for(version),
        options(),
    )
    .unwrap();
    for index in 0..BLOCKS {
        writer.write_block(source_block(index)).unwrap();
    }
    Fixture {
        primary: writer.finish().unwrap().into_inner(),
        attributes: None,
    }
}

fn split(version: FormatVersion) -> Fixture {
    let mut writer = FormatWriter::new_split_for_version(
        version,
        Cursor::new(Vec::new()),
        Cursor::new(Vec::new()),
        multi_block_metadata_for(version),
        options(),
    )
    .unwrap();
    for index in 0..BLOCKS {
        writer.write_block(source_block(index)).unwrap();
    }
    let (primary, attributes) = writer.finish_pair().unwrap();
    Fixture {
        primary: primary.into_inner(),
        attributes: Some(attributes.unwrap().into_inner()),
    }
}

fn fixtures(version: FormatVersion) -> [(&'static str, Fixture); 2] {
    [
        ("monolithic", monolithic(version)),
        ("split", split(version)),
    ]
}

#[derive(Debug, PartialEq)]
struct BlockData {
    positions: PointBatch,
    colors: Vec<ColorRgb>,
    columns: Vec<AttributeColumn>,
}

/// Opens the pair and decodes XYZ, colors and every attribute column of every block.
/// Records the largest allocation made while doing so in `LARGEST_ALLOCATION`.
fn read_all(fixture: &Fixture) -> Result<Vec<BlockData>, FormatError> {
    LARGEST_ALLOCATION.with(|largest| largest.set(0));
    let mut reader = FormatReader::with_limits(Cursor::new(fixture.primary.clone()), limits())?;
    if let Some(attributes) = &fixture.attributes {
        reader.attach_attributes(Cursor::new(attributes.clone()))?;
    }
    (0..BLOCKS)
        .map(|index| {
            let id = block_id(index);
            let positions = reader.read_positions(id)?;
            let colors = reader.read_display_colors(id)?;
            let columns = (0..descriptors().len() as u32)
                .map(|column| reader.read_attribute(id, column))
                .collect::<Result<_, _>>()?;
            Ok(BlockData {
                positions,
                colors,
                columns,
            })
        })
        .collect()
}

fn largest_allocation() -> usize {
    LARGEST_ALLOCATION.with(Cell::get)
}

/// Which file of the fixture a corruption applies to.
#[derive(Clone, Copy, Debug)]
enum Target {
    Primary,
    Attributes,
}

fn file_mut(fixture: &mut Fixture, target: Target) -> &mut Vec<u8> {
    match target {
        Target::Primary => &mut fixture.primary,
        Target::Attributes => fixture.attributes.as_mut().unwrap(),
    }
}

fn targets(fixture: &Fixture) -> Vec<Target> {
    let mut targets = vec![Target::Primary];
    if fixture.attributes.is_some() {
        targets.push(Target::Attributes);
    }
    targets
}

/// Recomputes the footer and (when the footer still points inside the file) index checksums.
/// Unlike the shared helper it tolerates footers that were deliberately made nonsensical.
fn repair(bytes: &mut [u8], target: Target) {
    let footer = bytes.len() - FOOTER;
    if matches!(target, Target::Primary) {
        let index = u64::from_le_bytes(bytes[footer + 8..footer + 16].try_into().unwrap());
        let length = u64::from_le_bytes(bytes[footer + 16..footer + 24].try_into().unwrap());
        let end = index.checked_add(length);
        if let Some(end) = end.filter(|end| *end <= footer as u64) {
            let crc = crc32(&bytes[index as usize..end as usize]);
            bytes[footer + 32..footer + 36].copy_from_slice(&crc.to_le_bytes());
        }
    }
    let crc = crc32(&bytes[footer..footer + 36]);
    bytes[footer + 36..].copy_from_slice(&crc.to_le_bytes());
}

/// Runs `body` for every writable revision on its own thread and fails with all findings.
fn sweep_versions(body: impl Fn(FormatVersion) -> Vec<String> + Sync) {
    let findings: Vec<String> = std::thread::scope(|scope| {
        let body = &body;
        // `v7.5-pre` and `v7` have no monolithic layout and a paged index: `tests/v7_5.rs` covers
        // their corruption.
        let handles: Vec<_> = FormatVersion::WRITABLE
            .into_iter()
            .filter(|version| !version.uses_pages())
            .map(|version| scope.spawn(move || body(version)))
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().unwrap())
            .collect()
    });
    assert!(findings.is_empty(), "{}", findings.join("\n"));
}

#[test]
fn truncation_at_every_prefix_is_an_error_without_panic() {
    sweep_versions(|version| {
        let mut findings = Vec::new();
        for (kind, fixture) in fixtures(version) {
            let expected = read_all(&fixture).unwrap_or_else(|e| panic!("{version:?} {kind}: {e}"));
            assert_eq!(expected.len(), BLOCKS as usize);
            for target in targets(&fixture) {
                let length = file_len(&fixture, target);
                for prefix in 0..length {
                    let mut broken = fixture.clone();
                    file_mut(&mut broken, target).truncate(prefix);
                    if read_all(&broken).is_ok() {
                        findings.push(format!(
                            "{version:?} {kind} {target:?}: prefix {prefix}/{length} was accepted"
                        ));
                    }
                }
            }
        }
        findings
    });
}

fn file_len(fixture: &Fixture, target: Target) -> usize {
    match target {
        Target::Primary => fixture.primary.len(),
        Target::Attributes => fixture.attributes.as_ref().unwrap().len(),
    }
}

/// Flips one bit at every offset of every file of every revision, optionally recomputing the
/// index/footer checksums afterwards. An accepted file must decode like the original unless
/// the checksums were repaired.
fn flip_sweep(repaired: bool) {
    sweep_versions(|version| {
        let mut findings = Vec::new();
        for (kind, fixture) in fixtures(version) {
            let expected = read_all(&fixture).unwrap();
            for target in targets(&fixture) {
                for offset in 0..file_len(&fixture, target) {
                    let mut broken = fixture.clone();
                    let bytes = file_mut(&mut broken, target);
                    bytes[offset] ^= 1 << (offset % 8);
                    if repaired {
                        repair(bytes, target);
                    }
                    if let Ok(decoded) = read_all(&broken)
                        && !repaired
                        && decoded != expected
                    {
                        findings.push(format!(
                            "{version:?} {kind} {target:?} offset {offset}: different data accepted"
                        ));
                    }
                }
            }
        }
        findings
    });
}

#[test]
fn single_bit_flip_never_panics() {
    // Without repair, a file is rejected or (for bytes outside every checksum) decodes to the
    // original data.
    flip_sweep(false);
    // CRC repair can legitimately yield a different valid file (checksums are not
    // authentication), so only the absence of panics is required here.
    flip_sweep(true);
}

/// A named byte region of the index; integers are what the extreme-value test mutates.
#[derive(Debug, Clone)]
struct Field {
    name: &'static str,
    offset: usize,
    width: usize,
    integer: bool,
    /// Setting this field to u32/u64::MAX must make the file unreadable.
    max_is_fatal: bool,
}

/// Walks the fixture index (one root node) and lists every region, in file order.
fn index_fields(index: &[u8], version: FormatVersion) -> Vec<Field> {
    struct Walker<'a> {
        bytes: &'a [u8],
        at: usize,
        fields: Vec<Field>,
    }
    impl Walker<'_> {
        fn raw(&mut self, name: &'static str, width: usize) {
            self.fields.push(Field {
                name,
                offset: self.at,
                width,
                integer: false,
                max_is_fatal: false,
            });
            self.at += width;
        }
        fn int(&mut self, name: &'static str, width: usize, max_is_fatal: bool) -> u64 {
            let mut value = [0; 8];
            value[..width].copy_from_slice(&self.bytes[self.at..self.at + width]);
            self.fields.push(Field {
                name,
                offset: self.at,
                width,
                integer: true,
                max_is_fatal,
            });
            self.at += width;
            u64::from_le_bytes(value)
        }
    }
    let mut walker = Walker {
        bytes: index,
        at: 0,
        fields: Vec::new(),
    };
    walker.raw("index_magic", 8);
    walker.raw("dataset_origin", 24);
    let global = version.uses_global_frame();
    if global {
        walker.int("root_level", 4, true);
        for _ in 0..3 {
            walker.int("root_prefix", 8, true);
        }
        walker.raw("frame_origin_quanta", 24);
        walker.raw("data_max_quanta", 24);
    }
    let has_source_count = walker.bytes[walker.at] == 1;
    walker.raw("source_count_tag", 1);
    if has_source_count {
        walker.int("source_point_count", 8, false);
    }
    if global {
        let authority = walker.int("crs_authority_length", 4, true);
        walker.raw("crs_authority", authority as usize);
        walker.int("crs_code", 4, false);
        walker.raw("crs_raw_encoding", 1);
        let raw = walker.int("crs_raw_length", 4, true);
        walker.raw("crs_raw", raw as usize);
        let has_source_scale = walker.bytes[walker.at] == 1;
        walker.raw("source_scale_offset_tag", 1);
        if has_source_scale {
            walker.raw("source_scale_offset", 48);
        }
    }
    for name in ["crs_length", "sampling_length"] {
        if global && name == "crs_length" {
            continue;
        }
        let length = walker.int(name, 4, true);
        walker.raw("string_bytes", length as usize);
    }
    walker.raw("hierarchy_semantics", 1);
    walker.raw("visual_scale", 8);
    let attributes = walker.int("attribute_count", 4, true);
    let mut widths = Vec::new();
    for _ in 0..attributes {
        let length = walker.int("attribute_name_length", 4, true);
        walker.raw("attribute_name", length as usize);
        widths.push(match walker.bytes[walker.at] {
            0 | 1 => 1,
            2 | 3 => 2,
            4 | 5 | 8 => 4,
            _ => 8,
        });
        walker.raw("attribute_type", 1);
        walker.int("component_count", 4, true);
    }
    let nodes = walker.int("node_count", 8, true);
    for _ in 0..nodes {
        walker.int("node_id", 8, false);
        let has_parent = walker.bytes[walker.at] == 1;
        walker.raw("node_parent_tag", 1);
        if has_parent {
            walker.int("node_parent", 8, false);
        }
        walker.int("node_level", 4, false);
        if global {
            for _ in 0..3 {
                walker.int("node_prefix", 8, false);
            }
        }
        walker.raw("node_bounds", 48);
        walker.int("node_point_count", 8, true);
        walker.raw("node_geometric_error", 8);
        if global {
            walker.raw("node_spacing", 8);
        }
        let blocks = walker.int("node_block_count", 4, true);
        for _ in 0..blocks {
            walker.int("node_block_id", 8, true);
        }
        let ranges = walker.int("node_range_count", 4, true);
        for _ in 0..ranges {
            let schema_index = walker.int("range_schema_index", 4, false);
            let width = widths[schema_index as usize];
            walker.raw("range_min", width);
            walker.raw("range_max", width);
        }
    }
    let blocks = walker.int("block_count", 8, true);
    for _ in 0..blocks {
        walker.int("block_id", 8, true);
        walker.int("block_offset", 8, true);
        walker.int("block_payload_length", 8, true);
        walker.int("block_point_count", 8, true);
        if version != FormatVersion::V5 {
            walker.raw("block_geometry_encoding", 1);
        }
        walker.raw("block_quantization_error", 8);
        if global {
            for _ in 0..3 {
                walker.int("block_origin_quanta", 8, false);
            }
        } else {
            walker.raw("block_origin", 24);
            walker.raw("block_scale", 24);
        }
        walker.raw("block_reserved_u8", 1);
        walker.int("block_reserved_u32", 4, false);
        for _ in 0..attributes + 2 {
            walker.int("payload_offset", 8, true);
            walker.int("payload_stored_length", 8, true);
            walker.int("payload_raw_length", 8, true);
            walker.raw("payload_codec", 1);
            walker.int("payload_checksum", 4, false);
        }
    }
    assert_eq!(walker.at, index.len(), "index walker lost track of layout");
    walker.fields
}

fn max_for(width: usize) -> u64 {
    if width == 4 {
        u64::from(u32::MAX)
    } else {
        u64::MAX
    }
}

fn candidates(width: usize, file_length: usize, original: u64) -> Vec<u64> {
    let max = max_for(width);
    let length = file_length as u64;
    let mut values = vec![0, 1, max, length - 1, length + 1];
    values.retain(|value| *value != original && *value <= max);
    values.dedup();
    values
}

fn set_int(bytes: &mut [u8], offset: usize, width: usize, value: u64) {
    bytes[offset..offset + width].copy_from_slice(&value.to_le_bytes()[..width]);
}

fn get_int(bytes: &[u8], offset: usize, width: usize) -> u64 {
    let mut value = [0; 8];
    value[..width].copy_from_slice(&bytes[offset..offset + width]);
    u64::from_le_bytes(value)
}

#[test]
fn index_and_footer_fields_extreme_values_are_rejected() {
    let mutations = AtomicUsize::new(0);
    sweep_versions(|version| {
        let mut violations = Vec::new();
        for (kind, fixture) in fixtures(version) {
            let expected = read_all(&fixture).unwrap();
            let length = fixture.primary.len();
            let footer = length - FOOTER;
            let index_offset = get_int(&fixture.primary, footer + 8, 8) as usize;
            let index_length = get_int(&fixture.primary, footer + 16, 8) as usize;
            let mut fields = index_fields(
                &fixture.primary[index_offset..index_offset + index_length],
                version,
            );
            for field in &mut fields {
                field.offset += index_offset;
            }
            fields.retain(|field| field.integer);
            // Footer: offset, length and declared file length of the primary (checksums are
            // recomputed by the repair, so they cannot hold an independent value).
            for (name, at) in [("footer_index_offset", 8), ("footer_index_length", 16)]
                .into_iter()
                .chain([("footer_file_length", 24)])
            {
                fields.push(Field {
                    name,
                    offset: footer + at,
                    width: 8,
                    integer: true,
                    max_is_fatal: true,
                });
            }
            let mut cases = fields
                .into_iter()
                .map(|field| (Target::Primary, field))
                .collect::<Vec<_>>();
            if let Some(attributes) = &fixture.attributes {
                let footer = attributes.len() - FOOTER;
                for (name, at, width) in [
                    ("attributes_footer_index_offset", 8, 8),
                    ("attributes_footer_index_length", 16, 8),
                    ("attributes_footer_file_length", 24, 8),
                    ("attributes_footer_index_checksum", 32, 4),
                ] {
                    cases.push((
                        Target::Attributes,
                        Field {
                            name,
                            offset: footer + at,
                            width,
                            integer: true,
                            max_is_fatal: true,
                        },
                    ));
                }
            }
            for (target, field) in cases {
                let file = match target {
                    Target::Primary => &fixture.primary,
                    Target::Attributes => fixture.attributes.as_ref().unwrap(),
                };
                let original = get_int(file, field.offset, field.width);
                for value in candidates(field.width, file.len(), original) {
                    let mut broken = fixture.clone();
                    let bytes = file_mut(&mut broken, target);
                    set_int(bytes, field.offset, field.width, value);
                    // The index checksum field itself is the value under test for the
                    // companion; every other case needs valid checksums to reach the parser.
                    if field.name == "attributes_footer_index_checksum" {
                        let footer = bytes.len() - FOOTER;
                        let crc = crc32(&bytes[footer..footer + 36]);
                        bytes[footer + 36..].copy_from_slice(&crc.to_le_bytes());
                    } else {
                        repair(bytes, target);
                    }
                    mutations.fetch_add(1, Ordering::Relaxed);
                    let result = read_all(&broken);
                    let largest = largest_allocation();
                    let context = format!(
                        "{version:?} {kind} {target:?} {} @{} = {value:#x}",
                        field.name, field.offset
                    );
                    if largest > ALLOCATION_CEILING {
                        violations.push(format!("{context}: allocated {largest} bytes"));
                    }
                    match result {
                        // Every field must fail through format validation: the fixtures
                        // are in-memory cursors, so `Io` (including `UnexpectedEof`) would
                        // mean a bound was checked by the stream instead of the parser.
                        Err(FormatError::Io(error)) => {
                            violations.push(format!(
                                "{context}: I/O error instead of validation: {error}"
                            ));
                        }
                        Err(_) => {}
                        Ok(_) if field.max_is_fatal && value == max_for(field.width) => {
                            violations.push(format!("{context}: MAX accepted"));
                        }
                        Ok(decoded) if decoded != expected => {
                            violations.push(format!("{context}: different data accepted"));
                        }
                        Ok(_) => {}
                    }
                }
            }
        }
        violations
    });
    assert!(
        mutations.into_inner() > 1000,
        "too few mutations were exercised"
    );
}

/// A tiny index declaring the default maximum of nodes must be rejected before any
/// allocation sized from that count.
#[test]
fn node_count_beyond_index_bytes_is_rejected_without_reserving() {
    let default_max_nodes = 2_000_000_u64;
    for version in FormatVersion::WRITABLE {
        if version.uses_pages() {
            continue; // paged index: covered by `tests/v7_5.rs`
        }
        let mut fixture = monolithic(version);
        let bytes = &mut fixture.primary;
        let footer = bytes.len() - FOOTER;
        let index_offset = get_int(bytes, footer + 8, 8) as usize;
        let index_length = get_int(bytes, footer + 16, 8) as usize;
        let node_count = index_fields(&bytes[index_offset..index_offset + index_length], version)
            .into_iter()
            .find(|field| field.name == "node_count")
            .unwrap();
        let at = index_offset + node_count.offset;
        bytes[at..at + 8].copy_from_slice(&default_max_nodes.to_le_bytes());
        repair(bytes, Target::Primary);

        LARGEST_ALLOCATION.with(|largest| largest.set(0));
        let result = FormatReader::with_limits(
            Cursor::new(fixture.primary.clone()),
            FormatReaderLimits::default(),
        );
        assert!(
            result.is_err(),
            "{version:?}: tiny index declaring 2 M nodes opened"
        );
        assert!(
            largest_allocation() < ALLOCATION_CEILING,
            "{version:?}: allocated {} bytes",
            largest_allocation()
        );
    }
}
