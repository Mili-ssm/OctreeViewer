//! Conversion plan and the per-block route shared by every block-writing phase: both the native
//! parallel `stream_blocks` and a serial loop prepare a block with [`prepare_planned_block`]
//! and append it with [`write_prepared`], so their output bytes agree by construction.

use std::io::{Seek, Write};

use viewer_core::{BlockId, CoordinateFrame};
use viewer_format::{
    BlockEncoder, FormatBlockEncoder, FormatError, FormatMetadata, FormatWriter,
    FormatWriterOptions, GeometryCodecDiagnostics, PreparedOctreeWriter,
};

use crate::error::ConvertError;
use crate::options::ConvertOptions;
use crate::pipeline::spool_store::SpoolStore;
use crate::spatial::{PlannedBlock, SpatialPlan};

/// Largest uncompressed block payload the writer and encoder accept.
pub const WRITER_MAX_UNCOMPRESSED_BLOCK_BYTES: usize = 64 * 1024 * 1024;

pub struct ConversionPlan {
    pub metadata: FormatMetadata,
    pub frame: CoordinateFrame,
    pub point_count: u64,
    pub block_count: u64,
    pub blocks: Vec<PlannedBlock>,
    pub source_block_ids: Vec<BlockId>,
    pub omitted_dimensions: Vec<String>,
    pub estimated_peak_memory_bytes: u64,
}

impl ConversionPlan {
    pub fn from_spatial(spatial: SpatialPlan, frame: CoordinateFrame) -> Self {
        let block_count = spatial.blocks.len() as u64;
        let source_block_ids = spatial
            .blocks
            .iter()
            .filter(|block| block.is_source_points)
            .map(|block| block.id)
            .collect();
        Self {
            metadata: spatial.metadata,
            frame,
            point_count: spatial.point_count,
            block_count,
            blocks: spatial.blocks,
            source_block_ids,
            omitted_dimensions: spatial.omitted_dimensions,
            estimated_peak_memory_bytes: spatial.estimated_peak_memory_bytes,
        }
    }
}

/// The writer and its block encoder for `options`, built in the order the converter has always
/// used (writer options, writer, encoder) so error precedence is unchanged. The split pair is
/// written unless the revision is a single file (`attributes` is `None` then).
pub fn output_writer<W: Write + Seek>(
    options: ConvertOptions,
    metadata: &FormatMetadata,
    geometry: W,
    attributes: Option<W>,
) -> Result<(FormatWriter<W>, FormatBlockEncoder<'_>), ConvertError> {
    let writer_options = FormatWriterOptions::new(
        options.max_quantization_error,
        WRITER_MAX_UNCOMPRESSED_BLOCK_BYTES as u64,
    )
    .map_err(ConvertError::Format)?;
    let writer = match attributes {
        Some(attributes) => FormatWriter::new_split_for_version(
            options.format_version(),
            geometry,
            attributes,
            metadata.clone(),
            writer_options,
        )?,
        None => FormatWriter::new_for_version(
            options.format_version(),
            geometry,
            metadata.clone(),
            writer_options,
        )?,
    };
    let encoder = FormatBlockEncoder::new(options.format_version(), metadata, writer_options)?;
    Ok((writer, encoder))
}

/// Reads one planned block from its spool and encodes it.
#[inline]
pub fn prepare_planned_block<E: BlockEncoder, S: SpoolStore>(
    encoder: &E,
    block: &PlannedBlock,
    store: &S,
    frame: CoordinateFrame,
) -> Result<E::PreparedBlock, ConvertError> {
    encoder
        .prepare(block.read(store, frame)?)
        .map_err(ConvertError::Format)
}

/// [`prepare_planned_block`] also returning the encoder's candidate diagnostics.
#[inline]
pub fn prepare_planned_block_with_diagnostics<E: BlockEncoder, S: SpoolStore>(
    encoder: &E,
    block: &PlannedBlock,
    store: &S,
    frame: CoordinateFrame,
) -> Result<(E::PreparedBlock, Option<GeometryCodecDiagnostics>), ConvertError> {
    encoder
        .prepare_with_diagnostics(block.read(store, frame)?)
        .map_err(ConvertError::Format)
}

/// Appends a prepared block, then releases its spool when this was the last block reading it.
/// The caller guarantees that every read of a shared spool has finished.
#[inline]
pub fn write_prepared<W, S>(
    writer: &mut W,
    block: &PlannedBlock,
    prepared: W::PreparedBlock,
    store: &S,
) -> Result<(), ConvertError>
where
    W: PreparedOctreeWriter<Error = FormatError> + ?Sized,
    S: SpoolStore,
{
    writer.write_prepared_block(prepared)?;
    block.release_spool_after_write(store)
}
