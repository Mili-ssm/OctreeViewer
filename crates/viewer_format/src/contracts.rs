use std::io::{Read, Seek, Write};

use viewer_core::{AttributeColumn, AttributeValues, BlockId, ColorRgb, DatasetSource, PointBatch};

use crate::{
    FormatError, FormatWriter, PreparedPointBlock, SourcePointBlock, reader::IndexedReader,
};

/// Selective access to an immutable octree dataset, independent of its I/O transport.
///
/// All reads retain the same row order within a block. Positions are local to the
/// coordinate frame in `DatasetSource::metadata`. Implementations must not implement
/// selective reads by loading the complete block or unrelated payloads.
/// Opening streams, attaching companions and forking cursors remain implementation details.
pub trait OctreeReader: DatasetSource {
    /// Exact binary identity of this reader; does not describe producer builds.
    fn format_version(&self) -> crate::FormatVersion;

    /// Whether color and attribute payloads are available, including a companion if required.
    fn has_attributes(&self) -> bool;

    /// XYZ only, with neutral gray colors. Does not access color/attribute payloads.
    fn read_positions(&mut self, id: BlockId) -> Result<PointBatch, Self::Error>;

    /// XYZ and available display RGB, without attribute columns. Gray if unavailable.
    fn read_geometry(&mut self, id: BlockId) -> Result<PointBatch, Self::Error>;

    /// Display RGB only, in the same row order as positions.
    fn read_display_colors(&mut self, id: BlockId) -> Result<Vec<ColorRgb>, Self::Error>;

    /// One complete column by index in the dataset schema, without XYZ or other columns.
    fn read_attribute(&mut self, id: BlockId, index: u32) -> Result<AttributeColumn, Self::Error>;

    /// Components of one row. `FormatReader` decodes the whole column, the compression unit.
    fn read_attribute_row(
        &mut self,
        id: BlockId,
        row: u64,
        index: u32,
    ) -> Result<AttributeValues, Self::Error>;

    /// Finite range of one schema component across node payloads; no payload I/O.
    fn attribute_range(&self, index: u32, component: u32) -> Result<Option<[f64; 2]>, Self::Error>;
}

/// Incremental octree output. Blocks retain local f64 positions until encoding.
///
/// Construction chooses metadata/options and output sinks. Finalization validates
/// completeness and returns all sinks, including an optional attribute companion;
/// publication, syncing durable files and checkpoints remain the caller's responsibility.
/// On failure, callers must not publish partially written output.
pub trait OctreeWriter {
    type Error;
    type Output;

    /// Exact binary identity written to all output streams.
    fn format_version(&self) -> crate::FormatVersion;

    fn write_block(&mut self, block: SourcePointBlock) -> Result<(), Self::Error>;

    /// Consumes the writer. Mutable ingestion also supports trait objects; closing
    /// by value requires a sized implementation.
    fn finish_pair(self) -> Result<Self::Output, Self::Error>
    where
        Self: Sized;
}

/// Optional capability for writers that accept independently encoded blocks.
/// The prepared representation must match the writer's metadata and encoding options.
pub trait PreparedOctreeWriter: OctreeWriter {
    type PreparedBlock;

    fn write_prepared_block(&mut self, block: Self::PreparedBlock) -> Result<(), Self::Error>;
}

/// Immutable block preparation, usable by parallel workers without sharing output sinks.
/// The prepared type binds an encoder to a compatible incremental writer.
pub trait BlockEncoder: Sync {
    type PreparedBlock: Send;

    fn prepare(&self, block: SourcePointBlock) -> Result<Self::PreparedBlock, FormatError>;

    /// Diagnostic preparation may add candidate-level instrumentation. Implementations that
    /// do not expose such metrics retain the ordinary preparation path.
    fn prepare_with_diagnostics(
        &self,
        block: SourcePointBlock,
    ) -> Result<(Self::PreparedBlock, Option<crate::GeometryCodecDiagnostics>), FormatError> {
        self.prepare(block).map(|prepared| (prepared, None))
    }

    /// Retained encoded payload capacity, excluding metadata and allocator overhead.
    fn encoded_payload_capacity_bytes(&self, block: &Self::PreparedBlock) -> usize;
}

impl<R: Read + Seek> OctreeReader for IndexedReader<R> {
    fn format_version(&self) -> crate::FormatVersion {
        IndexedReader::format_version(self)
    }

    fn has_attributes(&self) -> bool {
        IndexedReader::has_attributes(self)
    }

    fn read_positions(&mut self, id: BlockId) -> Result<PointBatch, Self::Error> {
        IndexedReader::read_positions(self, id)
    }

    fn read_geometry(&mut self, id: BlockId) -> Result<PointBatch, Self::Error> {
        IndexedReader::read_geometry(self, id)
    }

    fn read_display_colors(&mut self, id: BlockId) -> Result<Vec<ColorRgb>, Self::Error> {
        IndexedReader::read_display_colors(self, id)
    }

    fn read_attribute(&mut self, id: BlockId, index: u32) -> Result<AttributeColumn, Self::Error> {
        IndexedReader::read_attribute(self, id, index)
    }

    fn read_attribute_row(
        &mut self,
        id: BlockId,
        row: u64,
        index: u32,
    ) -> Result<AttributeValues, Self::Error> {
        IndexedReader::read_attribute_row(self, id, row, index)
    }

    fn attribute_range(&self, index: u32, component: u32) -> Result<Option<[f64; 2]>, Self::Error> {
        IndexedReader::attribute_range(self, index, component)
    }
}

impl<W: Write + Seek> OctreeWriter for FormatWriter<W> {
    type Error = FormatError;
    type Output = (W, Option<W>);

    fn format_version(&self) -> crate::FormatVersion {
        FormatWriter::format_version(self)
    }

    fn write_block(&mut self, block: SourcePointBlock) -> Result<(), Self::Error> {
        FormatWriter::write_block(self, block)
    }

    fn finish_pair(self) -> Result<Self::Output, Self::Error> {
        FormatWriter::finish_pair(self)
    }
}

impl<W: Write + Seek> PreparedOctreeWriter for FormatWriter<W> {
    type PreparedBlock = PreparedPointBlock;

    fn write_prepared_block(&mut self, block: Self::PreparedBlock) -> Result<(), Self::Error> {
        FormatWriter::write_prepared_block(self, block)
    }
}
