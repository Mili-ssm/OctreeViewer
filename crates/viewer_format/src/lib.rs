//! Versioned `.oct` storage over `Read + Seek` and `Write + Seek` streams.
//! Use `FormatReader` for selective reads and `FormatWriter` for incremental output;
//! both share the binary layout and metadata validation without depending on each other.

#[cfg(all(feature = "async-reader", not(target_arch = "wasm32")))]
mod async_reader;
mod attribute_codec;
mod block_encoder;
mod codec;
mod contracts;
mod crc32;
mod error;
pub mod experimental_tuple_dictionary;
mod geometry;
mod layout;
mod metadata;
mod occupancy_codec;
#[cfg(not(target_arch = "wasm32"))]
mod parallel_reader;
mod quantization;
mod quantized_occupancy;
mod ranges;
mod read_budget;
mod read_plan;
mod reader;
mod storage_reader;
mod version;
mod writer;

pub use codec::{BlockCodec, CodecError, EncodedBlock, compress_block, decompress_block};
pub use contracts::{BlockEncoder, OctreeReader, OctreeWriter, PreparedOctreeWriter};
pub use crc32::{Crc32, crc32};
pub use error::FormatError;
pub use geometry::GeometryEncoding;
pub use geometry::{
    GeometryCandidateDiagnostics, GeometryCandidateKind, GeometryCodecDiagnostics,
    OccupancyBuildDiagnostics,
};
pub use layout::{FORMAT_VERSION, HEADER_ENVELOPE_VERSION, SPLIT_FOOTER_BYTES, SPLIT_HEADER_BYTES};
pub use metadata::{FormatMetadata, MetadataError, NodeAttributeRange, SourcePointBlock};
pub use quantization::{
    QuantizationError, QuantizationTransform, QuantizedCoordinates, quantize_local_positions,
};
pub use quantized_occupancy::{QuantizedOccupancy, QuantizedOccupancyError};
pub use reader::{
    BlockPayloadKind, BlockPayloadStorageStats, BlockStorageStats, FormatReaderLimits,
};
pub use writer::{FormatWriter, FormatWriterOptions, PreparedPointBlock, prepare_point_block};

pub use version::{FileRole, FormatIdentity, FormatVersion, probe_format};

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
