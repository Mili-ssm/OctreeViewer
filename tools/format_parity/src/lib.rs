//! Encoding digest used to compare `viewer_format` output across targets.
//!
//! The same deterministic input is encoded for every writable format version and the resulting
//! primary/attributes pair is hashed. Native and `wasm32-unknown-unknown` builds must agree.

use std::io::Cursor;

use viewer_core::{
    AttributeColumn, AttributeDataType, AttributeDescriptor, AttributeValues, BlockId, ColorRgb,
    CoordinateFrame, DatasetMetadata, NodeId, SpatialBounds, SpatialHierarchy, SpatialNode,
};
use viewer_format::{
    FormatMetadata, FormatVersion, FormatWriter, FormatWriterOptions, OctreeWriter,
    SourcePointBlock,
};

const SEED: u64 = 0x5eed;
const BLOCKS: u64 = 6;
// Geometry above 64 KiB selects lz4_flex's large hash table, where the 32/64-bit hashes differ.
const POINTS_PER_BLOCK: usize = 16384;
const BLOCK_LIMIT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_QUANTIZATION_ERROR: f64 = 0.01;
const ORIGIN: [f64; 3] = [1_000.5, 2_000.25, 10.0];
const EXTENT: f64 = 64.0;

/// Fixed-seed LCG (Knuth MMIX constants); identical on every target.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 16
    }

    fn unit(&mut self) -> f64 {
        (self.next() & 0xffff_ffff) as f64 / 4_294_967_296.0
    }
}

fn descriptors() -> Vec<AttributeDescriptor> {
    [
        ("gps_time", AttributeDataType::F64, 1),
        ("rgb16", AttributeDataType::U16, 3),
        ("nir", AttributeDataType::U16, 1),
        ("extra_bytes", AttributeDataType::U8, 4),
    ]
    .into_iter()
    .map(|(name, data_type, components)| {
        AttributeDescriptor::new(name, data_type, components).expect("valid descriptor")
    })
    .collect()
}

/// Single-root metadata covering `blocks`; shared by the synthetic and real-block fixtures.
fn metadata_for(
    origin: [f64; 3],
    bounds: SpatialBounds,
    blocks: Vec<BlockId>,
    total: u64,
    schema: Vec<AttributeDescriptor>,
) -> FormatMetadata {
    let root =
        SpatialNode::new(NodeId::new(0), None, 0, bounds, total, 0.25, blocks).expect("valid root");
    let hierarchy =
        SpatialHierarchy::new(vec![root], schema, Some(total)).expect("valid hierarchy");
    FormatMetadata::new(
        DatasetMetadata::new(
            CoordinateFrame::new(origin).expect("valid frame"),
            hierarchy,
        ),
        Some("EPSG:25830".to_owned()),
        Some("parity fixture".to_owned()),
        Vec::new(),
    )
    .expect("valid metadata")
}

fn metadata() -> FormatMetadata {
    let bounds = SpatialBounds::new(
        ORIGIN,
        [ORIGIN[0] + EXTENT, ORIGIN[1] + EXTENT, ORIGIN[2] + EXTENT],
    )
    .expect("valid bounds");
    let blocks = (0..BLOCKS).map(BlockId::new).collect();
    metadata_for(
        ORIGIN,
        bounds,
        blocks,
        BLOCKS * POINTS_PER_BLOCK as u64,
        descriptors(),
    )
}

/// Block `index % 3`: 0 = regular grid with constant attributes (highly compressible),
/// 1 = LCG noise in every field (incompressible), 2 = jittered grid with small-alphabet
/// attributes (partially compressible: LZ4 match candidates depend on the hash function).
fn block(index: u64, rng: &mut Lcg) -> SourcePointBlock {
    let n = POINTS_PER_BLOCK;
    let pattern = index % 3;
    let mut positions = Vec::with_capacity(n);
    let mut colors = Vec::with_capacity(n);
    let mut gps = Vec::with_capacity(n);
    let mut rgb16 = Vec::with_capacity(n * 3);
    let mut nir = Vec::with_capacity(n);
    let mut extra = Vec::with_capacity(n * 4);
    for i in 0..n {
        if pattern == 0 {
            let (x, y, z) = (i % 16, (i / 16) % 16, i / 256);
            positions.push([x as f64 * 0.37, y as f64 * 0.37, z as f64 * 0.37 + 0.123]);
            colors.push(ColorRgb::new(40, 80, 120));
            gps.push(1_000.0);
            rgb16.extend_from_slice(&[10_280, 20_560, 30_840]);
            nir.push(500);
            extra.extend_from_slice(&[1, 2, 3, 4]);
        } else if pattern == 2 {
            let (x, y, z) = (i % 16, (i / 16) % 16, i / 256);
            positions.push([
                x as f64 * 0.37 + (rng.next() % 8) as f64 * 0.01,
                y as f64 * 0.37 + (rng.next() % 8) as f64 * 0.01,
                z as f64 * 0.37 + (rng.next() % 8) as f64 * 0.01,
            ]);
            colors.push(ColorRgb::new(
                (rng.next() % 6) as u8 * 20,
                (rng.next() % 6) as u8 * 20,
                (rng.next() % 6) as u8 * 20,
            ));
            gps.push(1_000.0 + (i / 3) as f64 * 0.001);
            rgb16.extend((0..3).map(|_| (rng.next() % 6) as u16 * 4_000));
            nir.push((rng.next() % 16) as u16 * 100);
            extra.extend((0..4).map(|_| (rng.next() % 4) as u8));
        } else {
            positions.push([
                rng.unit() * (EXTENT - 1.0),
                rng.unit() * (EXTENT - 1.0),
                rng.unit() * (EXTENT - 1.0),
            ]);
            let r = rng.next();
            colors.push(ColorRgb::new(r as u8, (r >> 8) as u8, (r >> 16) as u8));
            gps.push(rng.unit() * 1.0e6);
            rgb16.extend((0..3).map(|_| rng.next() as u16));
            nir.push(rng.next() as u16);
            extra.extend((0..4).map(|_| rng.next() as u8));
        }
    }
    let values = [
        AttributeValues::F64(gps),
        AttributeValues::U16(rgb16),
        AttributeValues::U16(nir),
        AttributeValues::U8(extra),
    ];
    let attributes = descriptors()
        .into_iter()
        .zip(values)
        .map(|(descriptor, values)| {
            AttributeColumn::new(descriptor, values, n).expect("valid column")
        })
        .collect();
    SourcePointBlock::new(BlockId::new(index), positions, colors, attributes)
}

fn encode_with(
    version: FormatVersion,
    metadata: FormatMetadata,
    blocks: impl IntoIterator<Item = SourcePointBlock>,
) -> (Vec<u8>, Vec<u8>) {
    let options =
        FormatWriterOptions::new(MAX_QUANTIZATION_ERROR, BLOCK_LIMIT_BYTES).expect("valid options");
    let mut writer = FormatWriter::new_split_for_version(
        version,
        Cursor::new(Vec::new()),
        Cursor::new(Vec::new()),
        metadata,
        options,
    )
    .expect("writer");
    for block in blocks {
        OctreeWriter::write_block(&mut writer, block).expect("write block");
    }
    let (primary, attributes) = OctreeWriter::finish_pair(writer).expect("finish");
    (
        primary.into_inner(),
        attributes.expect("split companion").into_inner(),
    )
}

fn encode(version: FormatVersion) -> (Vec<u8>, Vec<u8>) {
    let mut rng = Lcg(SEED);
    let blocks = (0..BLOCKS)
        .map(|index| block(index, &mut rng))
        .collect::<Vec<_>>();
    encode_with(version, metadata(), blocks)
}

fn fnv1a(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

fn digest_versions(mut encode_version: impl FnMut(FormatVersion) -> (Vec<u8>, Vec<u8>)) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325;
    for version in FormatVersion::WRITABLE {
        let (primary, attributes) = encode_version(version);
        for file in [primary, attributes] {
            fnv1a(&mut hash, format!("{version:?}").as_bytes());
            fnv1a(&mut hash, &(file.len() as u64).to_le_bytes());
            fnv1a(&mut hash, &file);
        }
    }
    hash
}

/// FNV-1a 64 over `version || len || bytes` of both files, in `WRITABLE` order.
pub fn parity_digest() -> u64 {
    digest_versions(encode)
}

/// Same digest as [`parity_digest`] for blocks produced by [`serialize_blocks`]. The metadata is
/// derived from the blocks (one root node over all of them, frame origin zero, bounds = their
/// extent plus 1), so the result depends only on the serialized bytes.
pub fn parity_digest_of_blocks(serialized: &[u8]) -> u64 {
    let blocks = deserialize_blocks(serialized);
    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    let mut total = 0_u64;
    for block in &blocks {
        total += block.positions_local().len() as u64;
        for position in block.positions_local() {
            for axis in 0..3 {
                min[axis] = min[axis].min(position[axis]);
                max[axis] = max[axis].max(position[axis]);
            }
        }
    }
    let bounds = SpatialBounds::new(min.map(|v| v - 1.0), max.map(|v| v + 1.0)).expect("bounds");
    let schema = blocks[0]
        .attributes()
        .iter()
        .map(|column| column.descriptor().clone())
        .collect::<Vec<_>>();
    let ids = blocks.iter().map(|block| block.id()).collect::<Vec<_>>();
    let metadata = metadata_for([0.0; 3], bounds, ids, total, schema);
    digest_versions(|version| {
        encode_with(version, metadata.clone(), deserialize_blocks(serialized))
    })
}

const BLOCKS_MAGIC: &[u8; 4] = b"FPB1";

/// Serialized blocks (all integers and floats little-endian):
/// `"FPB1" | u32 block_count | block*`, with
/// `block = u64 id | u64 points | f64[3*points] positions | u8[3*points] rgb | u32 columns | column*`
/// and `column = u32 name_len | name utf8 | u8 type | u32 components | raw values`.
/// Types: 0 = U8, 1 = U16, 2 = F32, 3 = F64 (the only ones LAS planning produces).
pub fn serialize_blocks(blocks: &[SourcePointBlock]) -> Vec<u8> {
    let mut out = BLOCKS_MAGIC.to_vec();
    out.extend_from_slice(&(blocks.len() as u32).to_le_bytes());
    for block in blocks {
        out.extend_from_slice(&block.id().get().to_le_bytes());
        out.extend_from_slice(&(block.positions_local().len() as u64).to_le_bytes());
        for position in block.positions_local() {
            for value in position {
                out.extend_from_slice(&value.to_le_bytes());
            }
        }
        for color in block.colors() {
            out.extend_from_slice(&[color.r, color.g, color.b]);
        }
        out.extend_from_slice(&(block.attributes().len() as u32).to_le_bytes());
        for column in block.attributes() {
            let descriptor = column.descriptor();
            out.extend_from_slice(&(descriptor.name().len() as u32).to_le_bytes());
            out.extend_from_slice(descriptor.name().as_bytes());
            let code = match column.values() {
                AttributeValues::U8(_) => 0_u8,
                AttributeValues::U16(_) => 1,
                AttributeValues::F32(_) => 2,
                AttributeValues::F64(_) => 3,
                other => panic!("unsupported attribute type {:?}", other.data_type()),
            };
            out.push(code);
            out.extend_from_slice(&descriptor.component_count().to_le_bytes());
            match column.values() {
                AttributeValues::U8(values) => out.extend_from_slice(values),
                AttributeValues::U16(values) => values
                    .iter()
                    .for_each(|v| out.extend_from_slice(&v.to_le_bytes())),
                AttributeValues::F32(values) => values
                    .iter()
                    .for_each(|v| out.extend_from_slice(&v.to_le_bytes())),
                AttributeValues::F64(values) => values
                    .iter()
                    .for_each(|v| out.extend_from_slice(&v.to_le_bytes())),
                _ => unreachable!("rejected above"),
            }
        }
    }
    out
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, len: usize) -> &'a [u8] {
        let (head, tail) = self.0.split_at(len);
        self.0 = tail;
        head
    }

    fn array<const N: usize>(&mut self) -> [u8; N] {
        self.take(N).try_into().expect("exact length")
    }

    fn values<T, const N: usize>(&mut self, count: usize, from: fn([u8; N]) -> T) -> Vec<T> {
        self.take(count * N)
            .as_chunks::<N>()
            .0
            .iter()
            .map(|chunk| from(*chunk))
            .collect()
    }
}

fn deserialize_blocks(bytes: &[u8]) -> Vec<SourcePointBlock> {
    let mut reader = Reader(bytes);
    assert_eq!(
        reader.take(4),
        BLOCKS_MAGIC,
        "not a format_parity block dump"
    );
    let count = u32::from_le_bytes(reader.array());
    let mut blocks = Vec::new();
    for _ in 0..count {
        let id = BlockId::new(u64::from_le_bytes(reader.array()));
        let n = u64::from_le_bytes(reader.array()) as usize;
        let flat = reader.values(n * 3, f64::from_le_bytes);
        let positions = flat.as_chunks::<3>().0.to_vec();
        let colors = reader
            .take(n * 3)
            .as_chunks::<3>()
            .0
            .iter()
            .map(|c| ColorRgb::new(c[0], c[1], c[2]))
            .collect();
        let columns = u32::from_le_bytes(reader.array());
        let mut attributes = Vec::new();
        for _ in 0..columns {
            let name_len = u32::from_le_bytes(reader.array()) as usize;
            let name = std::str::from_utf8(reader.take(name_len)).expect("utf8 name");
            let code = reader.array::<1>()[0];
            let components = u32::from_le_bytes(reader.array());
            let len = n * components as usize;
            let (data_type, values) = match code {
                0 => (
                    AttributeDataType::U8,
                    AttributeValues::U8(reader.take(len).to_vec()),
                ),
                1 => (
                    AttributeDataType::U16,
                    AttributeValues::U16(reader.values(len, u16::from_le_bytes)),
                ),
                2 => (
                    AttributeDataType::F32,
                    AttributeValues::F32(reader.values(len, f32::from_le_bytes)),
                ),
                3 => (
                    AttributeDataType::F64,
                    AttributeValues::F64(reader.values(len, f64::from_le_bytes)),
                ),
                other => panic!("unknown attribute type code {other}"),
            };
            let descriptor =
                AttributeDescriptor::new(name, data_type, components).expect("descriptor");
            attributes.push(AttributeColumn::new(descriptor, values, n).expect("column"));
        }
        blocks.push(SourcePointBlock::new(id, positions, colors, attributes));
    }
    assert!(reader.0.is_empty(), "trailing bytes in block dump");
    blocks
}

/// Raw export read by `run-wasm.mjs`; no wasm-bindgen glue is involved.
#[unsafe(no_mangle)]
pub extern "C" fn format_parity_digest() -> u64 {
    parity_digest()
}

/// Allocates `len` bytes in wasm memory for the host to fill; leaked on purpose (one-shot run).
#[unsafe(no_mangle)]
pub extern "C" fn format_parity_alloc(len: usize) -> *mut u8 {
    Box::leak(vec![0_u8; len].into_boxed_slice()).as_mut_ptr()
}

/// # Safety
/// `ptr..ptr+len` must come from `format_parity_alloc(len)` and be fully written by the host.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn format_parity_digest_blocks(ptr: *const u8, len: usize) -> u64 {
    parity_digest_of_blocks(unsafe { std::slice::from_raw_parts(ptr, len) })
}
