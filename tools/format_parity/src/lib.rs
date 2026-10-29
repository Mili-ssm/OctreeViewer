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

fn metadata() -> FormatMetadata {
    let bounds = SpatialBounds::new(
        ORIGIN,
        [ORIGIN[0] + EXTENT, ORIGIN[1] + EXTENT, ORIGIN[2] + EXTENT],
    )
    .expect("valid bounds");
    let blocks = (0..BLOCKS).map(BlockId::new).collect();
    let total = BLOCKS * POINTS_PER_BLOCK as u64;
    let root =
        SpatialNode::new(NodeId::new(0), None, 0, bounds, total, 0.25, blocks).expect("valid root");
    let hierarchy =
        SpatialHierarchy::new(vec![root], descriptors(), Some(total)).expect("valid hierarchy");
    FormatMetadata::new(
        DatasetMetadata::new(
            CoordinateFrame::new(ORIGIN).expect("valid frame"),
            hierarchy,
        ),
        Some("EPSG:25830".to_owned()),
        Some("parity fixture".to_owned()),
        Vec::new(),
    )
    .expect("valid metadata")
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

fn encode(version: FormatVersion) -> (Vec<u8>, Vec<u8>) {
    let options =
        FormatWriterOptions::new(MAX_QUANTIZATION_ERROR, BLOCK_LIMIT_BYTES).expect("valid options");
    let mut writer = FormatWriter::new_split_for_version(
        version,
        Cursor::new(Vec::new()),
        Cursor::new(Vec::new()),
        metadata(),
        options,
    )
    .expect("writer");
    let mut rng = Lcg(SEED);
    for index in 0..BLOCKS {
        OctreeWriter::write_block(&mut writer, block(index, &mut rng)).expect("write block");
    }
    let (primary, attributes) = OctreeWriter::finish_pair(writer).expect("finish");
    (
        primary.into_inner(),
        attributes.expect("split companion").into_inner(),
    )
}

fn fnv1a(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

/// FNV-1a 64 over `version || len || bytes` of both files, in `WRITABLE` order.
pub fn parity_digest() -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325;
    for version in FormatVersion::WRITABLE {
        let (primary, attributes) = encode(version);
        for file in [primary, attributes] {
            fnv1a(&mut hash, format!("{version:?}").as_bytes());
            fnv1a(&mut hash, &(file.len() as u64).to_le_bytes());
            fnv1a(&mut hash, &file);
        }
    }
    hash
}

/// Raw export read by `run-wasm.mjs`; no wasm-bindgen glue is involved.
#[unsafe(no_mangle)]
pub extern "C" fn format_parity_digest() -> u64 {
    parity_digest()
}
