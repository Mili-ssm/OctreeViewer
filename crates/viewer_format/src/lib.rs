#![forbid(unsafe_code)]
//! Versioned `.oct` storage over `Read + Seek` and `Write + Seek` streams.
//! Use `FormatReader` for selective reads and `FormatWriter` for incremental output;
//! both share the binary layout and metadata validation without depending on each other.

#[cfg(all(feature = "async-reader", not(target_arch = "wasm32")))]
mod async_reader;
mod attribute_codec;
mod block_encoder;
mod checkpoint_record;
mod codec;
mod contracts;
mod crc32;
mod crs_unit;
mod error;
#[cfg(feature = "experimental")]
pub mod experimental_tuple_dictionary;
mod geometry;
mod global_frame;
mod index_codec;
mod layout;
mod lazy_pages;
mod metadata;
mod occupancy_codec;
mod page_codec;
mod paged_index;
#[cfg(not(target_arch = "wasm32"))]
mod parallel_reader;
mod prepare;
mod quantization;
mod quantized_occupancy;
mod ranges;
mod read_budget;
mod read_plan;
mod reader;
mod reader_limits;
mod slice_cursor;
mod storage_reader;
mod version;
mod writer;

/// Largest attribute schema the writer emits and the default reader limit accepts (RF-27).
pub(crate) const MAX_SCHEMA_ATTRIBUTES: usize = 256;

pub use codec::{BlockCodec, CodecError, EncodedBlock, compress_block, decompress_block};
pub use contracts::{BlockEncoder, OctreeReader, OctreeWriter, PreparedOctreeWriter};
pub use crc32::{Crc32, crc32};
pub use crs_unit::{CrsLinearUnit, CrsUnit};
pub use error::FormatError;
pub use geometry::GeometryEncoding;
pub use geometry::{
    GeometryCandidateDiagnostics, GeometryCandidateKind, GeometryCodecDiagnostics,
    OccupancyBuildDiagnostics,
};
pub use global_frame::{
    CrsRaw, CrsRecord, GLOBAL_LATTICE_BASE, GlobalFrame, NodeCell, SourceScaleOffset,
    cell_origin_from_base, cell_origin_quanta, quanta_to_units, quantize_coordinate,
    round_to_quanta,
};
pub use layout::{FORMAT_VERSION, HEADER_ENVELOPE_VERSION, SPLIT_FOOTER_BYTES, SPLIT_HEADER_BYTES};
pub use lazy_pages::{PageGraft, PageRange, PageSource};
pub use metadata::{
    FormatMetadata, MetadataError, NodeAttributeRange, PointDistanceQuantiles, SourcePointBlock,
};
pub use page_codec::{
    MAX_PAGE_LEVELS, Page, PageAnchor, PageBlock, PageNode, PagePointer, TARGET_PAGE_BYTES,
    page_node_id, split_node_id,
};
pub use paged_index::{
    IndexLocation, PagedCore, PagedHeader, PagedReader, PagedStats, TAIL_READ_BYTES, verify_paged,
};
pub use quantization::{
    QuantizationError, QuantizationTransform, QuantizedCoordinates, norm3, quantize_local_positions,
};
pub use quantized_occupancy::{QuantizedOccupancy, QuantizedOccupancyError};
pub use reader::{
    BlockPayloadKind, BlockPayloadStorageStats, BlockStorageStats, FormatReaderLimits,
};
pub use writer::{
    FormatWriter, FormatWriterOptions, PreparedPointBlock, RowOrder, prepare_point_block,
};

pub use version::{
    FileRole, FormatIdentity, FormatVersion, block_checkpoint_versions, probe_format,
};

#[cfg(all(feature = "async-reader", not(target_arch = "wasm32")))]
pub use async_reader::{AsyncFormatReader, AsyncReadError};
pub use block_encoder::FormatBlockEncoder;
#[cfg(not(target_arch = "wasm32"))]
pub use parallel_reader::{
    ParallelReadError, ParallelReadProgress, ParallelReadStatus, read_selected_parallel,
};
pub use read_budget::{BudgetedRead, ReadBudget, ReadReservation};
pub use read_plan::{
    PayloadReadRange, ReadMemoryEstimate, ReadPlan, ReadSelection, ReadSource, SelectedRead,
};
pub use storage_reader::FormatReader;
