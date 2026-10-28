use std::collections::{BTreeMap, BTreeSet};

use viewer_core::{
    AttributeValues, BlockId, DatasetMetadata, NodeId, SpatialBounds, SpatialHierarchy,
};

use crate::geometry::{GeometryEncoding, encode_delta};
use crate::layout::{
    PayloadDirectoryEntry, StorageLayout, attribute_payload_size, block_payload_size,
};
use crate::{
    FormatError, FormatMetadata, FormatVersion, SourcePointBlock, compress_block, crc32,
    quantize_local_positions,
};

/// Encodes one block without appending it, allowing independent blocks to be prepared concurrently.
pub fn prepare_point_block(
    block: SourcePointBlock,
    metadata: &FormatMetadata,
    options: FormatWriterOptions,
) -> Result<PreparedPointBlock, FormatError> {
    prepare_point_block_for_version(FormatVersion::V5, block, metadata, options)
}

pub(crate) fn prepare_point_block_for_version(
    version: FormatVersion,
    block: SourcePointBlock,
    metadata: &FormatMetadata,
    options: FormatWriterOptions,
) -> Result<PreparedPointBlock, FormatError> {
    prepare_point_block_for_version_impl(version, block, metadata, options, false)
        .map(|(block, _)| block)
}

pub(crate) fn prepare_point_block_for_version_with_diagnostics(
    version: FormatVersion,
    block: SourcePointBlock,
    metadata: &FormatMetadata,
    options: FormatWriterOptions,
) -> Result<(PreparedPointBlock, crate::GeometryCodecDiagnostics), FormatError> {
    prepare_point_block_for_version_impl(version, block, metadata, options, true)
        .map(|(block, diagnostics)| (block, diagnostics.expect("diagnostics enabled")))
}

pub(crate) fn prepare_point_block_for_version_impl(
    version: FormatVersion,
    block: SourcePointBlock,
    metadata: &FormatMetadata,
    options: FormatWriterOptions,
    collect_codec_diagnostics: bool,
) -> Result<(PreparedPointBlock, Option<crate::GeometryCodecDiagnostics>), FormatError> {
    let layout = StorageLayout::from_version(version)?;
    let point_count = block.positions_local().len();
    if block.colors().len() != point_count {
        return Err(FormatError::ColorCountMismatch {
            points: point_count,
            colors: block.colors().len(),
        });
    }
    let schema = metadata.dataset().hierarchy().attributes();
    let attributes = order_attributes(block.attributes(), schema, point_count)?;
    let ranges = attributes
        .iter()
        .enumerate()
        .filter_map(|(index, column)| crate::ranges::column_range(index as u32, column))
        .collect();
    let uncompressed_size = block_payload_size(point_count, schema)?;
    if uncompressed_size > options.max_uncompressed_block_bytes {
        return Err(FormatError::BlockExceedsLimit {
            requested: uncompressed_size,
            limit: options.max_uncompressed_block_bytes,
        });
    }
    let quantized =
        quantize_local_positions(block.positions_local(), options.max_quantization_error)?;
    let world_bounds = block_world_bounds(block.positions_local(), &quantized, metadata.dataset())?;
    let transform = quantized.transform();
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
    let (mut geometry_encoding, mut encoded_geometry, mut geometry_raw_length) =
        if layout == StorageLayout::V5 {
            let started = collect_codec_diagnostics.then(std::time::Instant::now);
            let encoded = compress_block(&raw_geometry);
            let elapsed = started.map_or(0, |started| started.elapsed().as_nanos());
            if codec_diagnostics.is_some() {
                candidates.push(candidate_metric(
                    crate::GeometryCandidateKind::Raw,
                    raw_encoding_ns,
                    elapsed,
                    raw_geometry_length,
                    encoded.bytes().len(),
                ));
            }
            (GeometryEncoding::RawU32, encoded, raw_geometry_length)
        } else {
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
    if layout.supports_occupancy() && point_count != 0 && u32::try_from(point_count).is_ok() {
        let max_geometry_bytes =
            options.max_uncompressed_block_bytes - (uncompressed_size - geometry_length as u64);
        if layout.restricts_occupancy_to_compact24() {
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
        } else {
            for bits in [24, 28] {
                let build_started = collect_codec_diagnostics.then(std::time::Instant::now);
                let candidate = crate::QuantizedOccupancy::new(&quantized, bits)
                    .map_err(|_| FormatError::BlockSizeOverflow)?;
                record_occupancy_build(&mut codec_diagnostics, bits, build_started);
                for own_octant in [false, true] {
                    let encoding_started = collect_codec_diagnostics.then(std::time::Instant::now);
                    if let Some(raw) =
                        crate::occupancy_codec::encode(&candidate, own_octant, max_geometry_bytes)?
                    {
                        let encoding_ns =
                            encoding_started.map_or(0, |started| started.elapsed().as_nanos());
                        let compression_started =
                            collect_codec_diagnostics.then(std::time::Instant::now);
                        let encoded = compress_block(&raw);
                        let compression_ns =
                            compression_started.map_or(0, |started| started.elapsed().as_nanos());
                        if codec_diagnostics.is_some() {
                            candidates.push(candidate_metric(
                                crate::GeometryCandidateKind::Occupancy {
                                    residual_bits: bits,
                                    own_octant,
                                },
                                encoding_ns,
                                compression_ns,
                                raw.len(),
                                encoded.bytes().len(),
                            ));
                        }
                        if encoded.bytes().len() < encoded_geometry.bytes().len() {
                            geometry_encoding = GeometryEncoding::Occupied;
                            selected_candidate = crate::GeometryCandidateKind::Occupancy {
                                residual_bits: bits,
                                own_octant,
                            };
                            geometry_raw_length = raw.len();
                            encoded_geometry = encoded;
                        }
                    } else if codec_diagnostics.is_some() {
                        candidates.push(candidate_metric(
                            crate::GeometryCandidateKind::Occupancy {
                                residual_bits: bits,
                                own_octant,
                            },
                            encoding_started.map_or(0, |started| started.elapsed().as_nanos()),
                            0,
                            0,
                            0,
                        ));
                    }
                }
                if layout.supports_compact_occupancy() {
                    let encoding_started = collect_codec_diagnostics.then(std::time::Instant::now);
                    if let Some(raw) =
                        crate::occupancy_codec::encode_compact(&candidate, max_geometry_bytes)?
                    {
                        let encoding_ns =
                            encoding_started.map_or(0, |started| started.elapsed().as_nanos());
                        let compression_started =
                            collect_codec_diagnostics.then(std::time::Instant::now);
                        let encoded = compress_block(&raw);
                        let compression_ns =
                            compression_started.map_or(0, |started| started.elapsed().as_nanos());
                        if codec_diagnostics.is_some() {
                            candidates.push(candidate_metric(
                                crate::GeometryCandidateKind::CompactOccupancy {
                                    residual_bits: bits,
                                },
                                encoding_ns,
                                compression_ns,
                                raw.len(),
                                encoded.bytes().len(),
                            ));
                        }
                        if encoded.bytes().len() < encoded_geometry.bytes().len() {
                            geometry_encoding = GeometryEncoding::CompactOccupied;
                            selected_candidate = crate::GeometryCandidateKind::CompactOccupancy {
                                residual_bits: bits,
                            };
                            geometry_raw_length = raw.len();
                            encoded_geometry = encoded;
                        }
                    } else if codec_diagnostics.is_some() {
                        candidates.push(candidate_metric(
                            crate::GeometryCandidateKind::CompactOccupancy {
                                residual_bits: bits,
                            },
                            encoding_started.map_or(0, |started| started.elapsed().as_nanos()),
                            0,
                            0,
                            0,
                        ));
                    }
                }
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
    append_encoded_payload(
        encoded_geometry,
        geometry_raw_length,
        &mut bytes,
        &mut payloads,
    )?;
    raw.clear();
    for color in block.colors() {
        raw.extend_from_slice(&[color.r, color.g, color.b]);
    }
    append_payload(&raw, &mut bytes, &mut payloads, layout, 3)?;
    for column in attributes {
        let length = attribute_payload_size(point_count as u64, column.descriptor())?;
        let length = usize::try_from(length).map_err(|_| FormatError::SizeOutOfRange(length))?;
        raw.clear();
        raw.try_reserve_exact(length)
            .map_err(|_| FormatError::AllocationFailed(length))?;
        encode_values(column.values(), &mut raw);
        let width = crate::layout::attribute_tuple_width(column.descriptor())?;
        append_payload(&raw, &mut bytes, &mut payloads, layout, width)?;
    }
    Ok((
        PreparedPointBlock {
            layout,
            geometry_encoding,
            id: block.id(),
            point_count: u64::try_from(point_count)
                .map_err(|_| FormatError::PointCountOutOfRange)?,
            quantization_error,
            origin: transform.origin(),
            scale: transform.scale(),
            world_bounds,
            bytes,
            payloads,
            ranges,
        },
        codec_diagnostics,
    ))
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
    pub(crate) origin: [f64; 3],
    pub(crate) scale: [f64; 3],
    pub(crate) world_bounds: Option<SpatialBounds>,
    pub(crate) payloads: Vec<PayloadDirectoryEntry>,
    pub(crate) ranges: Vec<crate::NodeAttributeRange>,
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

/// Compresses one independent payload without retaining a combined raw block.
pub(crate) fn append_payload(
    raw: &[u8],
    bytes: &mut Vec<u8>,
    payloads: &mut Vec<PayloadDirectoryEntry>,
    layout: StorageLayout,
    width: usize,
) -> Result<(), FormatError> {
    let encoded = if layout.restricts_dictionary_tuples() {
        crate::attribute_codec::encode_v7_pre3(raw, width)?
    } else if layout.supports_byte_planes() {
        crate::attribute_codec::encode_pre4(raw, width)?
    } else if layout.supports_dictionary() {
        crate::attribute_codec::encode(raw, width)?
    } else {
        compress_block(raw)
    };
    append_encoded_payload(encoded, raw.len(), bytes, payloads)
}

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

pub(crate) fn block_world_bounds(
    source_positions: &[[f64; 3]],
    quantized: &crate::QuantizedCoordinates,
    metadata: &DatasetMetadata,
) -> Result<Option<SpatialBounds>, FormatError> {
    let origin = metadata.coordinate_frame().origin();
    let mut bounds = BoundsAccumulator::empty();
    for local in source_positions {
        bounds.include_point(std::array::from_fn(|axis| origin[axis] + local[axis]));
    }
    for local in quantized.decoded_local_positions() {
        let local = local?;
        bounds.include_point(std::array::from_fn(|axis| {
            origin[axis] + f64::from(local[axis])
        }));
    }
    if bounds.is_empty() {
        Ok(None)
    } else {
        Ok(Some(bounds.to_bounds()?))
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct BoundsAccumulator {
    min: [f64; 3],
    max: [f64; 3],
    empty: bool,
}

impl BoundsAccumulator {
    pub(crate) fn empty() -> Self {
        Self {
            min: [f64::INFINITY; 3],
            max: [f64::NEG_INFINITY; 3],
            empty: true,
        }
    }

    pub(crate) fn from_bounds(bounds: SpatialBounds) -> Self {
        Self {
            min: bounds.min(),
            max: bounds.max(),
            empty: false,
        }
    }

    pub(crate) fn include_point(&mut self, point: [f64; 3]) {
        for (axis, coordinate) in point.into_iter().enumerate() {
            self.min[axis] = self.min[axis].min(coordinate);
            self.max[axis] = self.max[axis].max(coordinate);
        }
        self.empty = false;
    }

    pub(crate) fn include_bounds(&mut self, bounds: SpatialBounds) {
        self.include_point(bounds.min());
        self.include_point(bounds.max());
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.empty
    }

    pub(crate) fn to_bounds(self) -> Result<SpatialBounds, FormatError> {
        if self.empty {
            return Err(FormatError::InvalidMetadata("empty spatial bounds"));
        }
        Ok(SpatialBounds::new(self.min, self.max)?)
    }
}

/// Bounds per-block allocations made by the writer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FormatWriterOptions {
    max_quantization_error: f64,
    max_uncompressed_block_bytes: u64,
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
        })
    }
}

/// A fully encoded block ready for ordered append to a format writer.
pub struct PreparedPointBlock {
    pub(crate) layout: StorageLayout,
    pub(crate) geometry_encoding: GeometryEncoding,
    pub(crate) id: BlockId,
    pub(crate) point_count: u64,
    pub(crate) quantization_error: f64,
    pub(crate) origin: [f64; 3],
    pub(crate) scale: [f64; 3],
    pub(crate) world_bounds: Option<SpatialBounds>,
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
