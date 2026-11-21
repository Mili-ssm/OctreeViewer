//! Systematic corruption sweeps over every writable revision: truncation at every length,
//! one bit flipped at every offset, and extreme values in every index/footer integer field.

mod common;
#[path = "common/v8_fixture.rs"]
mod v8_fixture;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::io::Cursor;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::{global_metadata, options};
use v8_fixture::with_v8_extras;
use viewer_core::{
    AttributeColumn, AttributeDataType, AttributeDescriptor, AttributeValues, BlockId, ColorRgb,
    PointBatch,
};
use viewer_format::{
    CrsRaw, CrsRecord, FormatError, FormatMetadata, FormatReader, FormatReaderLimits,
    FormatVersion, FormatWriter, PagedReader, SourcePointBlock, SourceScaleOffset, crc32,
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

fn multi_block_metadata_for(_version: FormatVersion) -> FormatMetadata {
    with_v8_extras(global_fixture_metadata())
}

/// Files of one fixture: a monolithic file or a split pair.
#[derive(Clone)]
struct Fixture {
    primary: Vec<u8>,
    attributes: Option<Vec<u8>>,
    /// Id of the first block in the file: `v8` numbers blocks from 0, earlier revisions keep the
    /// source ids.
    first_id: u64,
    /// Encoded length of the root page of a paged primary file (it ends at the footer).
    root_page: Option<usize>,
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
    let primary = primary.into_inner();
    let root_page = Some({
        PagedReader::open(Cursor::new(primary.clone()))
            .unwrap()
            .root_page()
            .encoded_len()
    });
    Fixture {
        primary,
        attributes: Some(attributes.unwrap().into_inner()),
        first_id: 0,
        root_page,
    }
}

fn fixtures(version: FormatVersion) -> Vec<(&'static str, Fixture)> {
    vec![("split", split(version))]
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

/// Recomputes the footer and (when the footer still points inside the file) index checksums.
/// Unlike the shared helper it tolerates footers that were deliberately made nonsensical.
///
/// `root_page` is the encoded length of the root page of a paged primary file: its own checksum is
/// recomputed first, so that only the semantic checks can reject the edit.
fn repair(bytes: &mut [u8], target: Target, root_page: Option<usize>) {
    let footer = bytes.len() - FOOTER;
    if let (Target::Primary, Some(length)) = (target, root_page) {
        let start = footer - length;
        let crc = crc32(&bytes[start..footer - 4]);
        bytes[footer - 4..footer].copy_from_slice(&crc.to_le_bytes());
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

/// Runs `body` for every writable revision on its own thread and fails with all findings.
fn sweep_versions(body: impl Fn(FormatVersion) -> Vec<String> + Sync) {
    let findings: Vec<String> = std::thread::scope(|scope| {
        let body = &body;
        // `v7.5-pre` and `v7` have no monolithic layout and a paged index: `tests/v7_5.rs` covers
        // their corruption. `v8` is paged too and is swept here as a pair.
        let handles: Vec<_> = FormatVersion::WRITABLE
            .into_iter()
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
                        repair(bytes, target, fixture.root_page);
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
    /// Setting this field to `u32/u64::MAX` must make the file unreadable.
    max_is_fatal: bool,
}

fn paged_fields(index_offset: usize, root_start: usize) -> Vec<Field> {
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
    fields.push(field("page_node_count", root_start + 26, 4, true));
    fields.push(field("page_first_block_id", root_start + 30, 8, false));
    fields
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
            let root_page = fixture.root_page.expect("v8 is paged");
            let mut fields = paged_fields(index_offset, length - FOOTER - root_page);
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
                        repair(bytes, target, fixture.root_page);
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
        }
        violations
    });
    let mutations = mutations.into_inner();
    assert!(
        mutations > 50,
        "too few mutations were exercised: {mutations}"
    );
}
