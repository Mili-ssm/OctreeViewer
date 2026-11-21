//! Encoding digest used to compare `viewer_format` output across targets.
//!
//! The same deterministic blocks are written as V8 in several variants (stored display colour,
//! colour derived from `rgb16`, colour derived from a `u16` intensity-like column, a one-byte
//! page target that spreads the node index over many pages, and an empty LOD column set); every
//! primary/attributes pair is hashed into one digest. Native and `wasm32-unknown-unknown` builds
//! must agree.

use std::io::Cursor;

use viewer_core::{
    AttributeColumn, AttributeDataType, AttributeDescriptor, AttributeValues, BlockId, ColorRgb,
    CoordinateFrame, DatasetMetadata, NodeId, SpatialHierarchy, SpatialNode,
};
use viewer_format::{
    CrsRaw, CrsRecord, DisplayColourSource, FormatMetadata, FormatVersion, FormatWriter,
    FormatWriterOptions, GlobalFrame, NodeCell, OctreeWriter, SourcePointBlock,
};

const SEED: u64 = 0x5eed;
/// Leaf blocks of the synthetic fixture; the LOD block comes in addition.
const LEAVES: u64 = 6;
// Geometry above 64 KiB selects lz4_flex's large hash table, where the 32/64-bit hashes differ.
const POINTS_PER_BLOCK: usize = 16384;
const BLOCK_LIMIT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_QUANTIZATION_ERROR: f64 = 0.01;
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

/// Margin kept between a translated block and the faces of its node cell, in dataset units.
const CELL_MARGIN: f64 = 1.0;
/// Children of the root: one octant each, so at most eight leaves.
const MAX_LEAVES: usize = 8;

fn crs() -> CrsRecord {
    CrsRecord::new(
        "EPSG".to_owned(),
        25830,
        CrsRaw::Wkt("PROJCS[\"ETRS89 / UTM zone 30N\",AUTHORITY[\"EPSG\",\"25830\"]]".into()),
    )
    .expect("valid CRS")
}

/// Rebuilds `blocks` as a two-level V8 dataset: `blocks[0]` is the LOD block of the root node and
/// every later block is the only block of one child octant. Each block is translated (never
/// scaled) so its bounding box sits inside its cell, which keeps its point pattern; the root
/// level is the deepest one whose child cells still hold the largest block. Only the written
/// bytes matter here, so the metadata is whatever the writer accepts.
fn v8_dataset(
    blocks: &[SourcePointBlock],
    variant: &Variant,
) -> (FormatMetadata, Vec<SourcePointBlock>) {
    assert!(
        (2..=MAX_LEAVES + 1).contains(&blocks.len()),
        "need a LOD block and 1..={MAX_LEAVES} leaves"
    );
    let extents = blocks.iter().map(|block| {
        let mut min = [f64::INFINITY; 3];
        let mut max = [f64::NEG_INFINITY; 3];
        for position in block.positions_local() {
            for axis in 0..3 {
                min[axis] = min[axis].min(position[axis]);
                max[axis] = max[axis].max(position[axis]);
            }
        }
        (min, max)
    });
    let extents = extents.collect::<Vec<_>>();
    let widest = extents
        .iter()
        .flat_map(|(min, max)| (0..3).map(|axis| max[axis] - min[axis]))
        .fold(0.0, f64::max);
    // A cell at absolute level `l` is 2^(49 - l) units wide; the children must hold the widest
    // block plus both margins.
    let mut level = 48_u32;
    while level > 3 && 2.0_f64.powi(48 - level as i32) < widest + 2.0 * CELL_MARGIN {
        level -= 1;
    }
    let root_prefix: [u64; 3] = [
        (1 << (level - 1)) + 3,
        (1 << (level - 1)) + 4,
        (1 << (level - 1)) + 5,
    ];
    let origin = root_prefix.map(|p| viewer_format::cell_origin_quanta(level, p));
    let child_prefix = |i: usize| -> [u64; 3] {
        std::array::from_fn(|axis| (root_prefix[axis] << 1) | ((i >> axis) & 1) as u64)
    };
    let cells = (0..blocks.len())
        .map(|i| {
            let (prefix, spacing) = if i == 0 {
                (root_prefix, 2.0_f64.powi(41 - level as i32))
            } else {
                (child_prefix(i - 1), 2.0_f64.powi(-5))
            };
            (NodeId::new(i as u64), NodeCell::new(prefix, spacing))
        })
        .collect::<Vec<_>>();
    let root_side = (1_i64 << (64 - level)) - 1;
    let frame = GlobalFrame::new(
        level,
        root_prefix,
        origin,
        origin.map(|q| q + root_side),
        crs(),
        None,
        cells,
    )
    .expect("valid global frame");

    let schema = blocks[0]
        .attributes()
        .iter()
        .map(|column| column.descriptor().clone())
        .collect::<Vec<_>>();
    let mut nodes = Vec::new();
    let mut translated = Vec::new();
    let mut total = 0_u64;
    for (i, (block, (min, _))) in blocks.iter().zip(&extents).enumerate() {
        let (depth, prefix) = if i == 0 {
            (0, root_prefix)
        } else {
            (1, child_prefix(i - 1))
        };
        let bounds = frame.node_bounds(depth, prefix).expect("node bounds");
        let shift: [f64; 3] =
            std::array::from_fn(|axis| bounds.min()[axis] + CELL_MARGIN - min[axis]);
        let positions = block
            .positions_local()
            .iter()
            .map(|p| std::array::from_fn(|axis| p[axis] + shift[axis]))
            .collect::<Vec<[f64; 3]>>();
        let count = positions.len() as u64;
        total += count;
        let id = BlockId::new(i as u64 + 1);
        nodes.push(
            SpatialNode::new(
                NodeId::new(i as u64),
                (i > 0).then(|| NodeId::new(0)),
                depth,
                bounds,
                count,
                0.0,
                vec![id],
            )
            .expect("valid node"),
        );
        let colors = match variant.colour {
            // Leaves must carry exactly the colour the reader derives: the high byte per channel.
            DisplayColourSource::None => block.colors().to_vec(),
            DisplayColourSource::Rgb(column) => {
                let AttributeValues::U16(values) = block.attributes()[column as usize].values()
                else {
                    panic!("rgb column is u16")
                };
                values
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .map(|c| ColorRgb::new((c[0] >> 8) as u8, (c[1] >> 8) as u8, (c[2] >> 8) as u8))
                    .collect()
            }
            DisplayColourSource::Intensity(column) => {
                let AttributeValues::U16(values) = block.attributes()[column as usize].values()
                else {
                    panic!("intensity column is u16")
                };
                values
                    .iter()
                    .map(|v| ColorRgb::new((v >> 8) as u8, (v >> 8) as u8, (v >> 8) as u8))
                    .collect()
            }
        };
        translated.push(SourcePointBlock::new(
            id,
            positions,
            colors,
            block.attributes().to_vec(),
        ));
    }
    let hierarchy = SpatialHierarchy::new(nodes, schema, Some(total)).expect("valid hierarchy");
    // Whatever the LOD set is, it has to be a strictly increasing list inside the schema.
    let lod_columns = if variant.empty_lod {
        Vec::new()
    } else {
        (0..hierarchy.attributes().len() as u32)
            .step_by(2)
            .collect::<Vec<_>>()
    };
    let metadata = FormatMetadata::new(
        DatasetMetadata::new(
            CoordinateFrame::new(frame.frame_origin()).expect("valid frame"),
            hierarchy,
        ),
        None,
        Some("parity fixture".to_owned()),
        Vec::new(),
    )
    .expect("valid metadata")
    .with_global_frame(frame)
    .expect("global frame matches the hierarchy")
    .with_lod_attribute_columns(lod_columns)
    .with_display_colour_source(variant.colour)
    .with_point_distance_quantiles(
        viewer_format::PointDistanceQuantiles::new(0.25, 0.5, 1.5).expect("valid quantiles"),
    );
    (metadata, translated)
}

/// One way of writing the same blocks.
struct Variant {
    colour: DisplayColourSource,
    /// `Some(bytes)` sets the page target; a tiny one gives every index level its own page.
    page_target: Option<usize>,
    empty_lod: bool,
}

/// The variants a schema supports: the derived-colour ones need a `u16` x 3 column (`rgb16`) or
/// a `u16` column (intensity-like), so a schema without them skips those variants.
fn variants(blocks: &[SourcePointBlock]) -> Vec<Variant> {
    let column = |components| {
        blocks[0].attributes().iter().position(|column| {
            let descriptor = column.descriptor();
            descriptor.data_type() == AttributeDataType::U16
                && descriptor.component_count() == components
        })
    };
    let mut colours = vec![DisplayColourSource::None];
    colours.extend(column(3).map(|i| DisplayColourSource::Rgb(i as u32)));
    colours.extend(column(1).map(|i| DisplayColourSource::Intensity(i as u32)));
    let mut variants = colours
        .into_iter()
        .map(|colour| Variant {
            colour,
            page_target: None,
            empty_lod: false,
        })
        .collect::<Vec<_>>();
    variants.push(Variant {
        colour: DisplayColourSource::None,
        page_target: Some(1),
        empty_lod: false,
    });
    variants.push(Variant {
        colour: DisplayColourSource::None,
        page_target: None,
        empty_lod: true,
    });
    variants
}

fn encode(blocks: &[SourcePointBlock], variant: &Variant) -> (Vec<u8>, Vec<u8>) {
    let (metadata, blocks) = v8_dataset(blocks, variant);
    let mut options =
        FormatWriterOptions::new(MAX_QUANTIZATION_ERROR, BLOCK_LIMIT_BYTES).expect("valid options");
    if let Some(bytes) = variant.page_target {
        options = options.with_page_target_bytes(bytes);
    }
    let version: FormatVersion = "v8".parse().expect("known format version");
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

fn fnv1a(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

/// FNV-1a 64 over `len || bytes` of the V8 primary file and its attributes companion, for every
/// [`variants`] entry in order.
fn digest(blocks: &[SourcePointBlock]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325;
    for variant in &variants(blocks) {
        let (primary, attributes) = encode(blocks, variant);
        for file in [primary, attributes] {
            fnv1a(&mut hash, &(file.len() as u64).to_le_bytes());
            fnv1a(&mut hash, &file);
        }
    }
    hash
}

/// Digest of the synthetic fixture: a LOD block and six leaf blocks of three point patterns.
pub fn parity_digest() -> u64 {
    let mut rng = Lcg(SEED);
    let blocks = (0..=LEAVES)
        .map(|index| block(index, &mut rng))
        .collect::<Vec<_>>();
    digest(&blocks)
}

/// Same digest for blocks produced by [`serialize_blocks`] (LOD block first, then the leaves).
/// The metadata is derived from the blocks, so the result depends only on the serialized bytes.
pub fn parity_digest_of_blocks(serialized: &[u8]) -> u64 {
    digest(&deserialize_blocks(serialized))
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
    // SAFETY: the function contract (see `# Safety`) says `ptr..ptr+len` comes from
    // `format_parity_alloc(len)` and is fully written, so it is a valid, initialized `[u8]`.
    parity_digest_of_blocks(unsafe { std::slice::from_raw_parts(ptr, len) })
}
