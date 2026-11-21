//! Systematic corruption sweeps over `v8` fixtures of several shapes (block sizes, a tree whose
//! pages chain child pages, both files of the pair): truncation at every length, one bit flipped
//! at every offset, extreme values in every index/page/footer integer field, and every byte of the
//! index and pages overwritten with edge values.

mod common;
#[path = "common/v8_fixture.rs"]
mod v8_fixture;
#[path = "common/v8_tree.rs"]
mod v8_tree;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::io::Cursor;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::{global_metadata, options};
use v8_fixture::with_v8_extras;
use v8_tree::{tree, tree_dataset, write_tree_pair};
use viewer_core::{
    AttributeColumn, AttributeDataType, AttributeDescriptor, AttributeValues, BlockId, ColorRgb,
    PointBatch,
};
use viewer_format::{
    CrsRaw, CrsRecord, FormatError, FormatMetadata, FormatReader, FormatReaderLimits,
    FormatVersion, FormatWriter, PagedReader, SourcePointBlock, SourceScaleOffset, crc32,
};

const BLOCKS: u64 = 3;
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
        // SAFETY: the caller upholds the `GlobalAlloc` contract for `layout`, and the call is forwarded unchanged to `System`.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` was allocated by this allocator, i.e. by `System` with the same `layout`, and the caller upholds the rest of the `GlobalAlloc` contract.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note_allocation(layout.size());
        // SAFETY: the caller upholds the `GlobalAlloc` contract for `layout`, and the call is forwarded unchanged to `System`.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note_allocation(new_size);
        // SAFETY: `ptr` was allocated by this allocator, i.e. by `System` with the same `layout`, and the caller upholds the rest of the `GlobalAlloc` contract.
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
                .map(|_| f32::from(random.next().unwrap() as u16) / 8.0)
                .collect(),
        ),
    }
}

fn source_block(index: u64, points_per_block: usize) -> SourcePointBlock {
    let mut random = stream(index + 1);
    let mut unit = move || (random.next().unwrap() >> 11) as f64 / (1_u64 << 53) as f64;
    let points = (0..points_per_block)
        .map(|_| [unit(), unit(), unit()])
        .collect();
    let mut colors = stream(index + 50);
    let colors = (0..points_per_block)
        .map(|_| {
            let v = colors.next().unwrap();
            ColorRgb::new(v as u8, (v >> 8) as u8, (v >> 16) as u8)
        })
        .collect();
    let attributes = descriptors()
        .into_iter()
        .enumerate()
        .map(|(column, descriptor)| {
            let values = column_values(index, column, points_per_block);
            AttributeColumn::new(descriptor, values, points_per_block).unwrap()
        })
        .collect();
    SourcePointBlock::new(block_id(index), points, colors, attributes)
}

/// `v7.4-pre` root at level 48 (side 2 CRS units, origin (10, 14, 18) m), so the fixture's
/// local points in [0, 1) lie inside it. Carries a WKT CRS and a source scale/offset.
fn global_fixture_metadata(points_per_block: usize) -> FormatMetadata {
    let total = BLOCKS * points_per_block as u64;
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

/// Files of one fixture: a split pair.
#[derive(Clone)]
struct Fixture {
    primary: Vec<u8>,
    attributes: Option<Vec<u8>>,
    /// Id of the first block in the file: `v8` numbers blocks from 0 in node order.
    first_id: u64,
    /// Number of blocks.
    blocks: u64,
    /// Encoded length of the root page of the paged primary file (it ends at the footer).
    root_page: Option<usize>,
    /// Absolute `(offset, length)` of every child page of the primary file.
    child_pages: Vec<(usize, usize)>,
}

fn fixture_of(primary: Vec<u8>, attributes: Vec<u8>, blocks: u64) -> Fixture {
    let mut reader = PagedReader::open(Cursor::new(primary.clone())).unwrap();
    let root_page = reader.root_page().encoded_len();
    let mut pending = vec![reader.root_page().clone()];
    let mut child_pages = Vec::new();
    while let Some(page) = pending.pop() {
        for (anchor, pointer) in page.child_pages() {
            child_pages.push((pointer.offset as usize, pointer.length as usize));
            pending.push(reader.read_child_page(anchor, pointer).unwrap());
        }
    }
    Fixture {
        primary,
        attributes: Some(attributes),
        first_id: 0,
        blocks,
        root_page: Some(root_page),
        child_pages,
    }
}

/// One root node holding `BLOCKS` blocks of `points_per_block` points.
fn split(points_per_block: usize) -> Fixture {
    let mut writer = FormatWriter::new_split_for_version(
        FormatVersion::V8,
        Cursor::new(Vec::new()),
        Cursor::new(Vec::new()),
        with_v8_extras(global_fixture_metadata(points_per_block)),
        options(),
    )
    .unwrap();
    for index in 0..BLOCKS {
        writer
            .write_block(source_block(index, points_per_block))
            .unwrap();
    }
    let (primary, attributes) = writer.finish_pair().unwrap();
    fixture_of(
        primary.into_inner(),
        attributes.unwrap().into_inner(),
        BLOCKS,
    )
}

/// A 13-node tree, one block per node, each node in a page of its own: the root page points to
/// child pages that point to further child pages.
fn paged_tree() -> Fixture {
    let nodes = tree(2, |_, octant| octant < 3);
    let (metadata, blocks) = tree_dataset(&nodes, &descriptors(), |node, rows| {
        descriptors()
            .into_iter()
            .enumerate()
            .map(|(column, descriptor)| {
                let values = column_values(node.id, column, rows);
                AttributeColumn::new(descriptor, values, rows).unwrap()
            })
            .collect()
    });
    // Interior nodes keep every column, so the sweep can read them all from every block.
    let metadata = metadata.with_lod_attribute_columns((0..descriptors().len() as u32).collect());
    let (primary, attributes) = write_tree_pair(metadata, blocks, Some(1));
    let fixture = fixture_of(primary, attributes, nodes.len() as u64);
    assert!(fixture.child_pages.len() > 2, "the tree must chain pages");
    fixture
}

fn fixtures(_version: FormatVersion) -> Vec<(&'static str, Fixture)> {
    vec![
        ("split", split(24)),
        ("single-point", split(1)),
        ("large-blocks", split(150)),
        ("paged-tree", paged_tree()),
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
    (0..fixture.blocks)
        .map(|index| {
            let id = BlockId::new(fixture.first_id + index);
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

/// Recomputes the page, footer and (when the footer still points inside the file) index
/// checksums. Unlike the shared helper it tolerates footers that were deliberately made nonsensical.
///
/// The checksum of every page of a paged primary file is recomputed first, so that only the
/// semantic checks can reject the edit.
fn repair(bytes: &mut [u8], target: Target, fixture: &Fixture) {
    let footer = bytes.len() - FOOTER;
    if let (Target::Primary, Some(length)) = (target, fixture.root_page) {
        let start = footer - length;
        let crc = crc32(&bytes[start..footer - 4]);
        bytes[footer - 4..footer].copy_from_slice(&crc.to_le_bytes());
        for &(start, length) in &fixture.child_pages {
            let end = start + length;
            let crc = crc32(&bytes[start..end - 4]);
            bytes[end - 4..end].copy_from_slice(&crc.to_le_bytes());
        }
    }
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

/// Runs `body` for every fixture on its own thread and fails with all findings.
fn sweep(body: impl Fn(FormatVersion, &str, &Fixture) -> Vec<String> + Sync) {
    let findings: Vec<String> = std::thread::scope(|scope| {
        let body = &body;
        let handles: Vec<_> = fixtures(FormatVersion::V8)
            .into_iter()
            .map(|(kind, fixture)| scope.spawn(move || body(FormatVersion::V8, kind, &fixture)))
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
    sweep(|version, kind, fixture| {
        let mut findings = Vec::new();
        let expected = read_all(fixture).unwrap_or_else(|e| panic!("{version:?} {kind}: {e}"));
        assert_eq!(expected.len(), fixture.blocks as usize);
        for target in targets(fixture) {
            let length = file_len(fixture, target);
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
    sweep(|version, kind, fixture| {
        let mut findings = Vec::new();
        let expected = read_all(fixture).unwrap();
        for target in targets(fixture) {
            for offset in 0..file_len(fixture, target) {
                let mut broken = fixture.clone();
                let bytes = file_mut(&mut broken, target);
                bytes[offset] ^= 1 << (offset % 8);
                if repaired {
                    repair(bytes, target, fixture);
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
    /// Setting this field to `u32/u64::MAX` must make the file unreadable.
    max_is_fatal: bool,
}

/// Integer fields of the index header and of the fixed start of every page: its anchor, level
/// count, node count, first block id and the first node record (a page holds at least one node).
fn paged_fields(index_offset: usize, page_starts: &[usize]) -> Vec<Field> {
    let field = |name, offset, width, max_is_fatal| Field {
        name,
        offset,
        width,
        integer: true,
        max_is_fatal,
    };
    let mut fields = vec![field("root_level", index_offset + 32, 4, true)];
    for axis in 0..3 {
        fields.push(field("root_prefix", index_offset + 36 + axis * 8, 8, true));
    }
    for &page in page_starts {
        fields.extend([
            field("page_anchor_level", page, 1, true),
            field("page_level_count", page + 25, 1, true),
            field("page_node_count", page + 26, 4, true),
            field("page_first_block_id", page + 30, 8, false),
            field("first_node_depth", page + 38, 1, true),
            field("first_node_path", page + 39, 2, true),
            field("first_node_flags", page + 41, 1, true),
            field("first_node_point_count", page + 42, 8, true),
            field("first_node_block_count", page + 58, 4, true),
        ]);
    }
    fields
}

fn max_for(width: usize) -> u64 {
    if width >= 8 {
        u64::MAX
    } else {
        (1 << (8 * width)) - 1
    }
}

fn candidates(width: usize, file_length: usize, original: u64) -> Vec<u64> {
    let max = max_for(width);
    let length = file_length as u64;
    let mut values = vec![
        0,
        1,
        2,
        3,
        max,
        max - 1,
        max >> 1,
        (max >> 1) + 1,
        255,
        256,
        65_535,
        65_536,
        length - 1,
        length,
        length + 1,
        original.wrapping_add(1),
        original.wrapping_sub(1),
        original ^ (max >> 1),
    ];
    values.retain(|value| *value != original && *value <= max);
    values.sort_unstable();
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
    sweep(|version, kind, fixture| {
        let mut violations = Vec::new();
        let expected = read_all(fixture).unwrap();
        let length = fixture.primary.len();
        let footer = length - FOOTER;
        let index_offset = get_int(&fixture.primary, footer + 8, 8) as usize;
        let root_page = fixture.root_page.expect("v8 is paged");
        let page_starts: Vec<usize> = std::iter::once(length - FOOTER - root_page)
            .chain(fixture.child_pages.iter().map(|&(start, _)| start))
            .collect();
        let mut fields = paged_fields(index_offset, &page_starts);
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
                    repair(bytes, target, fixture);
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
                #[expect(
                    clippy::match_same_arms,
                    reason = "the arms document distinct outcomes even where the bodies coincide"
                )]
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
        violations
    });
    let mutations = mutations.into_inner();
    assert!(
        mutations > 1000,
        "too few mutations were exercised: {mutations}"
    );
}

/// A tiny index declaring the default maximum of nodes must be rejected before any
/// allocation sized from that count.
#[test]
fn node_count_beyond_index_bytes_is_rejected_without_reserving() {
    let default_max_nodes = 2_000_000_u64;
    // The root page declares its node count as a u32 after its anchor and level count, before
    // the first block id; both a count above the page maximum and a valid one that the page
    // bytes cannot hold must fail before the node vectors are reserved.
    for (kind, fixture) in fixtures(FormatVersion::V8) {
        let root_page = fixture.root_page.unwrap();
        let at = fixture.primary.len() - FOOTER - root_page + 26;
        for node_count in [default_max_nodes as u32, 30_000] {
            let mut bytes = fixture.primary.clone();
            bytes[at..at + 4].copy_from_slice(&node_count.to_le_bytes());
            repair(&mut bytes, Target::Primary, &fixture);
            LARGEST_ALLOCATION.with(|largest| largest.set(0));
            let result =
                FormatReader::with_limits(Cursor::new(bytes), FormatReaderLimits::default());
            assert!(
                matches!(
                    result,
                    Err(FormatError::MalformedIndex("page node count is invalid"))
                ),
                "{kind}: root page declaring {node_count} nodes: {:?}",
                result.err()
            );
            assert!(
                largest_allocation() < ALLOCATION_CEILING,
                "{kind}: allocated {} bytes",
                largest_allocation()
            );
        }
    }
}

/// Every byte of the headers, the index and every page, one at a time, takes edge values with the
/// checksums repaired so that only the parser's checks can reject it. A parser may accept an edit
/// that yields another valid file; it must neither panic nor size an allocation from the value.
#[test]
fn every_index_and_page_byte_overwritten_never_panics_or_over_allocates() {
    let mutations = AtomicUsize::new(0);
    sweep(|version, kind, fixture| {
        let mut violations = Vec::new();
        let footer = fixture.primary.len() - FOOTER;
        let index_offset = get_int(&fixture.primary, footer + 8, 8) as usize;
        let mut regions = vec![
            (Target::Primary, 0..24),
            (Target::Primary, index_offset..footer),
        ];
        regions.extend(
            fixture
                .child_pages
                .iter()
                .map(|&(start, length)| (Target::Primary, start..start + length)),
        );
        let attributes = fixture.attributes.as_ref().unwrap().len();
        regions.push((Target::Attributes, 0..24));
        regions.push((Target::Attributes, attributes - FOOTER..attributes));
        for (target, region) in regions {
            for offset in region {
                let original = byte_at(fixture, target, offset);
                for value in [0x00, 0xff, original ^ 0x80, original.wrapping_add(1)] {
                    if value == original {
                        continue;
                    }
                    let mut broken = fixture.clone();
                    let bytes = file_mut(&mut broken, target);
                    bytes[offset] = value;
                    repair(bytes, target, fixture);
                    mutations.fetch_add(1, Ordering::Relaxed);
                    let _ = read_all(&broken);
                    let largest = largest_allocation();
                    if largest > ALLOCATION_CEILING {
                        violations.push(format!(
                            "{version:?} {kind} {target:?} @{offset} = {value:#x}: allocated {largest} bytes"
                        ));
                    }
                }
            }
        }
        violations
    });
    let mutations = mutations.into_inner();
    assert!(
        mutations > 1000,
        "too few mutations were exercised: {mutations}"
    );
}

fn byte_at(fixture: &Fixture, target: Target, offset: usize) -> u8 {
    match target {
        Target::Primary => fixture.primary[offset],
        Target::Attributes => fixture.attributes.as_ref().unwrap()[offset],
    }
}
