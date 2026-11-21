use std::collections::{BTreeMap, BTreeSet};

use viewer_core::{AttributeColumn, AttributeValues, BlockId, NodeId, SpatialHierarchy};

use crate::geometry::{GeometryEncoding, encode_delta, encode_delta_iter};
use crate::layout::{
    PayloadDirectoryEntry, StorageLayout, StoredColumn, attribute_payload_size, block_payload_size,
};
use crate::{FormatError, FormatMetadata, FormatVersion, SourcePointBlock, compress_block, crc32};

/// Encodes one block without appending it, allowing independent blocks to be prepared concurrently.
pub fn prepare_point_block(
    block: SourcePointBlock,
    metadata: &FormatMetadata,
    options: FormatWriterOptions,
) -> Result<PreparedPointBlock, FormatError> {
    prepare_point_block_for_version(FormatVersion::V8, block, metadata, options)
}

/// Whether `block` belongs to a node with children, found by scanning the hierarchy. Callers that
/// prepare many blocks build [`lod_block_ids`] once instead. Only the single-block entry points
/// (tests and [`prepare_point_block`]) use it; the writer's `write_block` path takes the set
/// built once in its constructor.
fn block_is_lod_by_scan(block: BlockId, metadata: &FormatMetadata) -> Result<bool, FormatError> {
    Ok(lod_block_ids(metadata.dataset().hierarchy())?.contains(&block))
}

pub(crate) fn prepare_point_block_for_version(
    version: FormatVersion,
    block: SourcePointBlock,
    metadata: &FormatMetadata,
    options: FormatWriterOptions,
) -> Result<PreparedPointBlock, FormatError> {
    let block_is_lod = block_is_lod_by_scan(block.id(), metadata)?;
    prepare_point_block_for_version_impl(version, block, metadata, options, block_is_lod, false)
        .map(|(block, _)| block)
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "takes ownership of the block, which the caller no longer needs"
)]
pub(crate) fn prepare_point_block_for_version_impl(
    version: FormatVersion,
    block: SourcePointBlock,
    metadata: &FormatMetadata,
    options: FormatWriterOptions,
    block_is_lod: bool,
    collect_codec_diagnostics: bool,
) -> Result<(PreparedPointBlock, Option<crate::GeometryCodecDiagnostics>), FormatError> {
    let layout = StorageLayout::from_version(version)?;
    let rules = metadata.column_rules();
    let column_rule = |column: usize| layout.stored_column(block_is_lod, column, rules);
    let row_order = options.resolved_row_order(version)?;
    let point_count = block.positions_local().len();
    if block.colors().len() != point_count {
        return Err(FormatError::ColorCountMismatch {
            points: point_count,
            colors: block.colors().len(),
        });
    }
    let schema = metadata.dataset().hierarchy().attributes();
    let attributes = order_attributes(block.attributes(), schema, point_count)?;
    if let StoredColumn::Derived { from } = column_rule(1) {
        ensure_display_is_derivable(block.id(), block.colors(), attributes[from - 2])?;
    }
    let ranges = attributes
        .iter()
        .enumerate()
        .filter(|(index, _)| column_rule(index + 2).is_present())
        .filter_map(|(index, column)| crate::ranges::column_range(index as u32, column))
        .collect();
    let uncompressed_size = block_payload_size(point_count, schema, |column| {
        column_rule(column).is_present()
    })?;
    if uncompressed_size > options.max_uncompressed_block_bytes {
        return Err(FormatError::BlockExceedsLimit {
            requested: uncompressed_size,
            limit: options.max_uncompressed_block_bytes,
        });
    }
    let frame = global_frame_of(metadata)?;
    let (quantized, origin_quanta) = {
        let cell = frame
            .block_box(block.id())
            .ok_or(FormatError::UnreferencedBlock(block.id()))?;
        crate::quantization::quantize_global_positions(
            block.positions_local(),
            metadata.dataset().coordinate_frame().origin(),
            frame.frame_origin_quanta(),
            cell,
            options.max_quantization_error,
        )?
    };
    let quantization_error = quantized.max_error();
    let geometry_length = usize::try_from(
        (point_count as u64)
            .checked_mul(12)
            .ok_or(FormatError::BlockSizeOverflow)?,
    )
    .map_err(|_| FormatError::BlockSizeOverflow)?;
    let raw_encoding_started = collect_codec_diagnostics.then(std::time::Instant::now);
    let raw_geometry = encode_geometry(&quantized, geometry_length)?;
    let raw_encoding_ns = raw_encoding_started.map_or(0, |started| started.elapsed().as_nanos());
    let raw_geometry_length = raw_geometry.len();
    let mut codec_diagnostics =
        collect_codec_diagnostics.then(crate::GeometryCodecDiagnostics::default);
    let mut candidates = Vec::new();
    let (mut geometry_encoding, mut encoded_geometry, mut geometry_raw_length) = {
        let delta_started = collect_codec_diagnostics.then(std::time::Instant::now);
        let delta = encode_delta(quantized.coordinates())?;
        let delta_encoding_ns = delta_started.map_or(0, |started| started.elapsed().as_nanos());
        let delta_length = delta.len();
        let other_raw_size = uncompressed_size
            .checked_sub(geometry_length as u64)
            .ok_or(FormatError::BlockSizeOverflow)?;
        let delta_total = other_raw_size
            .checked_add(delta.len() as u64)
            .ok_or(FormatError::BlockSizeOverflow)?;
        let raw_started = collect_codec_diagnostics.then(std::time::Instant::now);
        let raw_encoded = compress_block(&raw_geometry);
        let raw_compression_ns = raw_started.map_or(0, |started| started.elapsed().as_nanos());
        if codec_diagnostics.is_some() {
            candidates.push(candidate_metric(
                crate::GeometryCandidateKind::Raw,
                raw_encoding_ns,
                raw_compression_ns,
                raw_geometry_length,
                raw_encoded.bytes().len(),
            ));
        }
        if delta_total <= options.max_uncompressed_block_bytes {
            let delta_started = collect_codec_diagnostics.then(std::time::Instant::now);
            let delta_encoded = compress_block(&delta);
            let delta_compression_ns =
                delta_started.map_or(0, |started| started.elapsed().as_nanos());
            if codec_diagnostics.is_some() {
                candidates.push(candidate_metric(
                    crate::GeometryCandidateKind::Delta,
                    delta_encoding_ns,
                    delta_compression_ns,
                    delta_length,
                    delta_encoded.bytes().len(),
                ));
            }
            if delta_encoded.bytes().len() < raw_encoded.bytes().len() {
                (GeometryEncoding::DeltaVarint, delta_encoded, delta_length)
            } else {
                (GeometryEncoding::RawU32, raw_encoded, raw_geometry_length)
            }
        } else {
            if codec_diagnostics.is_some() {
                candidates.push(candidate_metric(
                    crate::GeometryCandidateKind::Delta,
                    delta_encoding_ns,
                    0,
                    delta_length,
                    0,
                ));
            }
            (GeometryEncoding::RawU32, raw_encoded, raw_geometry_length)
        }
    };
    let mut selected_candidate = match geometry_encoding {
        GeometryEncoding::RawU32 => crate::GeometryCandidateKind::Raw,
        GeometryEncoding::DeltaVarint => crate::GeometryCandidateKind::Delta,
        GeometryEncoding::Occupied => crate::GeometryCandidateKind::Occupancy {
            residual_bits: 0,
            own_octant: false,
        },
        GeometryEncoding::CompactOccupied => {
            crate::GeometryCandidateKind::CompactOccupancy { residual_bits: 0 }
        }
    };
    if point_count != 0 && u32::try_from(point_count).is_ok() {
        let max_geometry_bytes =
            options.max_uncompressed_block_bytes - (uncompressed_size - geometry_length as u64);
        {
            let build_started = collect_codec_diagnostics.then(std::time::Instant::now);
            let candidate = crate::QuantizedOccupancy::new(&quantized, 24)
                .map_err(|_| FormatError::BlockSizeOverflow)?;
            record_occupancy_build(&mut codec_diagnostics, 24, build_started);
            let encoding_started = collect_codec_diagnostics.then(std::time::Instant::now);
            if let Some(raw) =
                crate::occupancy_codec::encode_compact(&candidate, max_geometry_bytes)?
            {
                let encoding_ns =
                    encoding_started.map_or(0, |started| started.elapsed().as_nanos());
                let compression_started = collect_codec_diagnostics.then(std::time::Instant::now);
                let encoded = compress_block(&raw);
                let compression_ns =
                    compression_started.map_or(0, |started| started.elapsed().as_nanos());
                if codec_diagnostics.is_some() {
                    candidates.push(candidate_metric(
                        crate::GeometryCandidateKind::CompactOccupancy { residual_bits: 24 },
                        encoding_ns,
                        compression_ns,
                        raw.len(),
                        encoded.bytes().len(),
                    ));
                }
                if encoded.bytes().len() < encoded_geometry.bytes().len() {
                    geometry_encoding = GeometryEncoding::CompactOccupied;
                    selected_candidate =
                        crate::GeometryCandidateKind::CompactOccupancy { residual_bits: 24 };
                    geometry_raw_length = raw.len();
                    encoded_geometry = encoded;
                }
            } else if codec_diagnostics.is_some() {
                candidates.push(candidate_metric(
                    crate::GeometryCandidateKind::CompactOccupancy { residual_bits: 24 },
                    encoding_started.map_or(0, |started| started.elapsed().as_nanos()),
                    0,
                    0,
                    0,
                ));
            }
        }
    }
    // Hybrid writer (ADR 0041/0042): a block whose source-order winner is CompactOccupied is
    // retried with its rows sorted by Morton key and DeltaVarint; ties keep source order.
    let mut row_permutation: Option<Vec<u32>> = None;
    if row_order == RowOrder::Hybrid
        && geometry_encoding == GeometryEncoding::CompactOccupied
        && u32::try_from(point_count).is_ok()
    {
        let morton_started = collect_codec_diagnostics.then(std::time::Instant::now);
        let permutation = morton_permutation(quantized.coordinates());
        let coordinates = quantized.coordinates();
        let delta = encode_delta_iter(
            permutation.iter().map(|&row| coordinates[row as usize]),
            coordinates.len(),
        )?;
        let other_raw_size = uncompressed_size
            .checked_sub(geometry_length as u64)
            .ok_or(FormatError::BlockSizeOverflow)?;
        let delta_total = other_raw_size
            .checked_add(delta.len() as u64)
            .ok_or(FormatError::BlockSizeOverflow)?;
        if delta_total <= options.max_uncompressed_block_bytes {
            let encoding_ns = morton_started.map_or(0, |started| started.elapsed().as_nanos());
            let compression_started = collect_codec_diagnostics.then(std::time::Instant::now);
            let encoded = compress_block(&delta);
            if codec_diagnostics.is_some() {
                candidates.push(candidate_metric(
                    crate::GeometryCandidateKind::DeltaMorton,
                    encoding_ns,
                    compression_started.map_or(0, |started| started.elapsed().as_nanos()),
                    delta.len(),
                    encoded.bytes().len(),
                ));
            }
            if encoded.bytes().len() < encoded_geometry.bytes().len() {
                geometry_encoding = GeometryEncoding::DeltaVarint;
                selected_candidate = crate::GeometryCandidateKind::DeltaMorton;
                geometry_raw_length = delta.len();
                encoded_geometry = encoded;
                row_permutation = Some(permutation);
            }
        }
    }
    if let Some(diagnostics) = codec_diagnostics.as_mut() {
        for candidate in &mut candidates {
            if candidate.kind == selected_candidate {
                candidate.selected_winners = 1;
                candidate.selected_raw_bytes = geometry_raw_length as u64;
                candidate.selected_compressed_bytes = encoded_geometry.bytes().len() as u64;
            }
        }
        diagnostics.candidates = candidates;
    }
    // The u32 coordinates have been encoded; retain only their scalar transform/error
    // while compressing payloads, rather than another 12 bytes per point.
    drop(quantized);
    let mut bytes = Vec::new();
    let mut payloads = Vec::new();
    payloads
        .try_reserve_exact(schema.len() + 2)
        .map_err(|_| FormatError::AllocationFailed(schema.len() + 2))?;
    let mut raw = raw_geometry;
    let mut scratch = Vec::new();
    append_encoded_payload(
        encoded_geometry,
        geometry_raw_length,
        &mut bytes,
        &mut payloads,
    )?;
    let display_rule = column_rule(1);
    if display_rule.is_present() {
        raw.clear();
        for color in block.colors() {
            raw.extend_from_slice(&[color.r, color.g, color.b]);
        }
        if let Some(permutation) = &row_permutation {
            permute_rows(&raw, permutation, 3, &mut scratch);
            std::mem::swap(&mut raw, &mut scratch);
        }
    }
    append_stored_payload(display_rule, &raw, &mut bytes, &mut payloads, 3)?;
    for (schema_index, column) in attributes.into_iter().enumerate() {
        let stored = column_rule(schema_index + 2);
        if !stored.is_present() {
            payloads.push(PayloadDirectoryEntry::not_stored(
                stored,
                bytes.len() as u64,
            ));
            continue;
        }
        let length = attribute_payload_size(point_count as u64, column.descriptor())?;
        let length = usize::try_from(length).map_err(|_| FormatError::SizeOutOfRange(length))?;
        raw.clear();
        raw.try_reserve_exact(length)
            .map_err(|_| FormatError::AllocationFailed(length))?;
        encode_values(column.values(), &mut raw);
        let width = crate::layout::attribute_tuple_width(column.descriptor())?;
        if let Some(permutation) = &row_permutation {
            permute_rows(&raw, permutation, width, &mut scratch);
            std::mem::swap(&mut raw, &mut scratch);
        }
        append_payload(&raw, &mut bytes, &mut payloads, width)?;
    }
    Ok((
        PreparedPointBlock {
            geometry_encoding,
            id: block.id(),
            point_count: u64::try_from(point_count)
                .map_err(|_| FormatError::PointCountOutOfRange)?,
            quantization_error,
            origin_quanta,
            bytes,
            payloads,
            ranges,
        },
        codec_diagnostics,
    ))
}

/// Row order chosen by the writer. V7 rows of a block may be in any order (ADR 0041).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RowOrder {
    /// Rows stay in the order of the source block.
    #[default]
    Source,
    /// Blocks whose best source-order geometry is `CompactOccupied` are re-encoded sorted by
    /// Morton key with `DeltaVarint` when that is strictly smaller (ADR 0042).
    Hybrid,
}

impl RowOrder {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Hybrid => "hybrid",
        }
    }
}

impl std::str::FromStr for RowOrder {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "source" => Ok(Self::Source),
            "hybrid" => Ok(Self::Hybrid),
            other => Err(format!(
                "unknown row order {other:?} (expected source or hybrid)"
            )),
        }
    }
}

/// Interleaves the 32 bits of each coordinate into a 96-bit Morton key (no truncation), x in the
/// high bit of each triple like the cell builder (`viewer_render_wgpu` `node_cells::morton`).
fn morton_key(coordinate: [u32; 3]) -> u128 {
    // Bit i of axis a goes to 3*i + (2 - a): the low 21 bits fill bits 0..63, the high 11 bits
    // fill bits 63..96. Magic-mask spreading instead of a 96-step loop (the key is computed for
    // every point of every dense block).
    let interleave = |part: fn(u32) -> u64| {
        (spread_by_3(part(coordinate[0])) << 2)
            | (spread_by_3(part(coordinate[1])) << 1)
            | spread_by_3(part(coordinate[2]))
    };
    let low = interleave(|value| u64::from(value & 0x1f_ffff));
    let high = interleave(|value| u64::from(value >> 21));
    u128::from(low) | (u128::from(high) << 63)
}

/// Spreads the low 21 bits of `value` so that bit i lands on bit 3*i.
fn spread_by_3(value: u64) -> u64 {
    let mut x = value & 0x1f_ffff;
    x = (x | (x << 32)) & 0x001f_0000_0000_ffff;
    x = (x | (x << 16)) & 0x001f_0000_ff00_00ff;
    x = (x | (x << 8)) & 0x100f_00f0_0f00_f00f;
    x = (x | (x << 4)) & 0x10c3_0c30_c30c_30c3;
    x = (x | (x << 2)) & 0x1249_2492_4924_9249;
    x
}

/// Source row indices ordered by (Morton key, source row): a deterministic total order.
fn morton_permutation(coordinates: &[[u32; 3]]) -> Vec<u32> {
    // Stable LSD radix sort of the rows by key: equal keys keep source row order, so the result
    // is the (key, row) order. Only the bits where the block's keys differ are visited (at most
    // nine 11-bit passes). The caller guarantees the row count fits in u32.
    const BITS: u32 = 11;
    const MASK: u128 = (1 << BITS) - 1;
    let keys: Vec<u128> = coordinates.iter().map(|c| morton_key(*c)).collect();
    let mut order: Vec<u32> = (0..keys.len() as u32).collect();
    let Some(&first) = keys.first() else {
        return order;
    };
    let varying = keys.iter().fold(0, |acc, key| acc | (key ^ first));
    if varying == 0 {
        return order;
    }
    let end = 128 - varying.leading_zeros();
    let mut shift = varying.trailing_zeros();
    let mut scratch = vec![0_u32; order.len()];
    while shift < end {
        let mut counts = [0_usize; 1 << BITS];
        for &row in &order {
            counts[((keys[row as usize] >> shift) & MASK) as usize] += 1;
        }
        let mut start = 0;
        for count in &mut counts {
            let n = *count;
            *count = start;
            start += n;
        }
        for &row in &order {
            let digit = ((keys[row as usize] >> shift) & MASK) as usize;
            scratch[counts[digit]] = row;
            counts[digit] += 1;
        }
        std::mem::swap(&mut order, &mut scratch);
        shift += BITS;
    }
    order
}

/// Row `i` of the result is row `permutation[i]` of `raw`, whose rows are `width` bytes.
fn permute_rows(raw: &[u8], permutation: &[u32], width: usize, out: &mut Vec<u8>) {
    out.clear();
    out.reserve(raw.len());
    for &row in permutation {
        let start = row as usize * width;
        out.extend_from_slice(&raw[start..start + width]);
    }
}

pub(crate) fn candidate_metric(
    kind: crate::GeometryCandidateKind,
    encoding_ns: u128,
    compression_ns: u128,
    raw_bytes: usize,
    compressed_bytes: usize,
) -> crate::GeometryCandidateDiagnostics {
    crate::GeometryCandidateDiagnostics {
        kind,
        attempts: 1,
        encoding_elapsed_ns: encoding_ns,
        compression_elapsed_ns: compression_ns,
        raw_bytes: raw_bytes as u64,
        compressed_bytes: compressed_bytes as u64,
        selected_winners: 0,
        selected_raw_bytes: 0,
        selected_compressed_bytes: 0,
    }
}

pub(crate) fn record_occupancy_build(
    diagnostics: &mut Option<crate::GeometryCodecDiagnostics>,
    residual_bits: u8,
    started: Option<std::time::Instant>,
) {
    if let (Some(diagnostics), Some(started)) = (diagnostics, started) {
        diagnostics
            .occupancy_builds
            .push(crate::OccupancyBuildDiagnostics {
                residual_bits,
                attempts: 1,
                elapsed_ns: started.elapsed().as_nanos(),
            });
    }
}

#[derive(Debug, Clone)]
pub(crate) struct BlockRecord {
    pub(crate) offset: u64,
    pub(crate) payload_length: u64,
    pub(crate) point_count: u64,
    pub(crate) geometry_encoding: GeometryEncoding,
    pub(crate) quantization_error: f64,
    /// Block origin in quanta.
    pub(crate) origin_quanta: [i64; 3],
    pub(crate) payloads: Vec<PayloadDirectoryEntry>,
    pub(crate) ranges: Vec<crate::NodeAttributeRange>,
}

/// The global frame every `v8` file is written from.
pub(crate) fn global_frame_of(
    metadata: &FormatMetadata,
) -> Result<&crate::GlobalFrame, FormatError> {
    metadata.global_frame().ok_or(FormatError::InvalidMetadata(
        "v8 needs a global frame in the metadata",
    ))
}

pub(crate) fn build_block_nodes(
    hierarchy: &SpatialHierarchy,
) -> Result<BTreeMap<BlockId, Vec<NodeId>>, FormatError> {
    let root = hierarchy
        .node(hierarchy.root_id())
        .ok_or(FormatError::InvalidMetadata("root node is missing"))?;
    let mut block_nodes = BTreeMap::<BlockId, Vec<NodeId>>::new();
    for node in hierarchy.query(root.bounds()) {
        let mut local_ids = BTreeSet::new();
        for block_id in node.blocks() {
            if !local_ids.insert(*block_id) {
                return Err(FormatError::DuplicateNodeBlock {
                    node: node.id(),
                    block: *block_id,
                });
            }
            block_nodes.entry(*block_id).or_default().push(node.id());
        }
    }
    Ok(block_nodes)
}

/// Ids of the blocks of nodes with children, the blocks whose columns a revision may omit.
/// Built once per writer or encoder; `finish` checks it against the written `has_children`.
pub(crate) fn lod_block_ids(
    hierarchy: &SpatialHierarchy,
) -> Result<BTreeSet<BlockId>, FormatError> {
    let root = hierarchy
        .node(hierarchy.root_id())
        .ok_or(FormatError::InvalidMetadata("root node is missing"))?;
    let nodes = hierarchy.query(root.bounds()).collect::<Vec<_>>();
    let parents = nodes
        .iter()
        .filter_map(|node| node.parent())
        .collect::<BTreeSet<_>>();
    Ok(nodes
        .iter()
        .filter(|node| parents.contains(&node.id()))
        .flat_map(|node| node.blocks().iter().copied())
        .collect())
}

pub(crate) fn order_attributes<'a>(
    attributes: &'a [viewer_core::AttributeColumn],
    schema: &[viewer_core::AttributeDescriptor],
    point_count: usize,
) -> Result<Vec<&'a viewer_core::AttributeColumn>, FormatError> {
    if attributes.len() != schema.len() {
        return Err(FormatError::AttributeCountMismatch {
            expected: schema.len(),
            actual: attributes.len(),
        });
    }
    let mut ordered = Vec::with_capacity(schema.len());
    let mut seen = BTreeSet::new();
    for descriptor in schema {
        let Some(column) = attributes
            .iter()
            .find(|column| column.descriptor().name() == descriptor.name())
        else {
            return Err(FormatError::MissingAttribute(descriptor.name().to_owned()));
        };
        if !seen.insert(column.descriptor().name()) {
            return Err(FormatError::DuplicateAttribute(
                column.descriptor().name().to_owned(),
            ));
        }
        if column.descriptor().data_type() != descriptor.data_type()
            || column.descriptor().component_count() != descriptor.component_count()
            || column.values().data_type() != descriptor.data_type()
        {
            return Err(FormatError::AttributeSchemaMismatch(
                descriptor.name().to_owned(),
            ));
        }
        let expected = point_count
            .checked_mul(descriptor.component_count() as usize)
            .ok_or_else(|| FormatError::AttributeLengthOverflow(descriptor.name().to_owned()))?;
        if column.values().len() != expected {
            return Err(FormatError::AttributeLengthMismatch {
                name: descriptor.name().to_owned(),
                expected,
                actual: column.values().len(),
            });
        }
        ordered.push(column);
    }
    Ok(ordered)
}

pub(crate) fn encode_geometry(
    quantized: &crate::QuantizedCoordinates,
    expected_size: usize,
) -> Result<Vec<u8>, FormatError> {
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(expected_size)
        .map_err(|_| FormatError::AllocationFailed(expected_size))?;
    for coordinate in quantized.coordinates() {
        for value in coordinate {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    if bytes.len() != expected_size {
        return Err(FormatError::InternalPayloadSize {
            expected: expected_size,
            actual: bytes.len(),
        });
    }
    Ok(bytes)
}

/// A leaf that does not store its display colour must hold exactly what the reader derives from
/// the source column: each channel is the high byte of the 16-bit value (r = g = b for a single
/// value), so any other colour would read back changed.
fn ensure_display_is_derivable(
    id: BlockId,
    colors: &[viewer_core::ColorRgb],
    source: &AttributeColumn,
) -> Result<(), FormatError> {
    let AttributeValues::U16(values) = source.values() else {
        return Err(FormatError::InvalidMetadata(
            "display colour source is not a 16-bit column",
        ));
    };
    let components = source.descriptor().component_count();
    let derived = crate::reader::derived_display_colors(values, components);
    if derived.len() != colors.len()
        || derived
            .zip(colors)
            .any(|(derived, color)| derived != *color)
    {
        return Err(FormatError::DisplayColourNotDerivable(id));
    }
    Ok(())
}

/// Appends a column according to the storage rule: a stored column is compressed, any other keeps
/// only its slot in the directory.
fn append_stored_payload(
    stored: StoredColumn,
    raw: &[u8],
    bytes: &mut Vec<u8>,
    payloads: &mut Vec<PayloadDirectoryEntry>,
    width: usize,
) -> Result<(), FormatError> {
    if stored.is_present() {
        append_payload(raw, bytes, payloads, width)
    } else {
        payloads.push(PayloadDirectoryEntry::not_stored(
            stored,
            bytes.len() as u64,
        ));
        Ok(())
    }
}

/// Compresses one independent payload without retaining a combined raw block.
pub(crate) fn append_payload(
    raw: &[u8],
    bytes: &mut Vec<u8>,
    payloads: &mut Vec<PayloadDirectoryEntry>,
    width: usize,
) -> Result<(), FormatError> {
    let encoded = crate::attribute_codec::encode_v7_pre3(raw, width)?;
    append_encoded_payload(encoded, raw.len(), bytes, payloads)
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "the encoded block is consumed by value so it cannot be reused after its payload is appended"
)]
pub(crate) fn append_encoded_payload(
    encoded: crate::EncodedBlock,
    raw_length: usize,
    bytes: &mut Vec<u8>,
    payloads: &mut Vec<PayloadDirectoryEntry>,
) -> Result<(), FormatError> {
    bytes
        .try_reserve(encoded.bytes().len())
        .map_err(|_| FormatError::AllocationFailed(encoded.bytes().len()))?;
    payloads.push(PayloadDirectoryEntry {
        offset: bytes.len() as u64,
        stored_length: encoded.bytes().len() as u64,
        raw_length: raw_length as u64,
        codec: encoded.codec(),
        checksum: crc32(encoded.bytes()),
        stored: StoredColumn::Present,
    });
    bytes.extend_from_slice(encoded.bytes());
    Ok(())
}

pub(crate) fn encode_values(values: &AttributeValues, output: &mut Vec<u8>) {
    match values {
        AttributeValues::U8(values) => output.extend_from_slice(values),
        AttributeValues::I8(values) => output.extend(values.iter().map(|value| *value as u8)),
        AttributeValues::U16(values) => values
            .iter()
            .for_each(|v| output.extend_from_slice(&v.to_le_bytes())),
        AttributeValues::I16(values) => values
            .iter()
            .for_each(|v| output.extend_from_slice(&v.to_le_bytes())),
        AttributeValues::U32(values) => values
            .iter()
            .for_each(|v| output.extend_from_slice(&v.to_le_bytes())),
        AttributeValues::I32(values) => values
            .iter()
            .for_each(|v| output.extend_from_slice(&v.to_le_bytes())),
        AttributeValues::U64(values) => values
            .iter()
            .for_each(|v| output.extend_from_slice(&v.to_le_bytes())),
        AttributeValues::I64(values) => values
            .iter()
            .for_each(|v| output.extend_from_slice(&v.to_le_bytes())),
        AttributeValues::F32(values) => values
            .iter()
            .for_each(|v| output.extend_from_slice(&v.to_bits().to_le_bytes())),
        AttributeValues::F64(values) => values
            .iter()
            .for_each(|v| output.extend_from_slice(&v.to_bits().to_le_bytes())),
    }
}

/// Bounds per-block allocations made by the writer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FormatWriterOptions {
    max_quantization_error: f64,
    max_uncompressed_block_bytes: u64,
    page_target_bytes: usize,
    /// `None` takes the format's fixed order or `Source`.
    row_order: Option<RowOrder>,
}

impl FormatWriterOptions {
    pub fn new(
        max_quantization_error: f64,
        max_uncompressed_block_bytes: u64,
    ) -> Result<Self, FormatError> {
        if !max_quantization_error.is_finite() || max_quantization_error < 0.0 {
            return Err(FormatError::InvalidQuantizationLimit(
                max_quantization_error,
            ));
        }
        if max_uncompressed_block_bytes == 0 {
            return Err(FormatError::ZeroBlockLimit);
        }
        Ok(Self {
            max_quantization_error,
            max_uncompressed_block_bytes,
            page_target_bytes: crate::TARGET_PAGE_BYTES,
            row_order: None,
        })
    }

    /// `v7.5-pre` only: the size a page may reach before the writer lowers its level count `k`
    /// (default [`crate::TARGET_PAGE_BYTES`]). A page never has fewer than one level.
    #[must_use]
    pub fn with_page_target_bytes(mut self, bytes: usize) -> Self {
        self.page_target_bytes = bytes;
        self
    }

    /// Requests a row order of each block. Unset, the format's fixed order applies (`v7` is
    /// always hybrid) or else [`RowOrder::Source`]; a different explicit order for a format
    /// that fixes one fails when blocks are prepared.
    #[must_use]
    pub fn with_row_order(mut self, row_order: RowOrder) -> Self {
        self.row_order = Some(row_order);
        self
    }

    /// Row order that blocks of `version` are written with.
    pub fn resolved_row_order(&self, version: FormatVersion) -> Result<RowOrder, FormatError> {
        match (version.fixed_row_order(), self.row_order) {
            (Some(fixed), Some(requested)) if fixed != requested => {
                Err(FormatError::RowOrderFixedByFormat(version))
            }
            (Some(fixed), _) => Ok(fixed),
            (None, requested) => Ok(requested.unwrap_or(RowOrder::Source)),
        }
    }

    pub(crate) fn page_target_bytes(&self) -> usize {
        self.page_target_bytes
    }
}

/// A fully encoded block ready for ordered append to a format writer.
pub struct PreparedPointBlock {
    pub(crate) geometry_encoding: GeometryEncoding,
    pub(crate) id: BlockId,
    pub(crate) point_count: u64,
    pub(crate) quantization_error: f64,
    pub(crate) origin_quanta: [i64; 3],
    pub(crate) bytes: Vec<u8>,
    pub(crate) payloads: Vec<PayloadDirectoryEntry>,
    pub(crate) ranges: Vec<crate::NodeAttributeRange>,
}

impl PreparedPointBlock {
    /// Capacity of the retained compressed payload buffer, excluding metadata and allocator overhead.
    pub fn encoded_payload_capacity_bytes(&self) -> usize {
        self.bytes.capacity()
    }
}

#[cfg(test)]
#[path = "row_order_tests.rs"]
mod row_order_tests;
