//! Versioned `.oct` storage over `Read + Seek` and `Write + Seek` streams.
//! Use `FormatReader` for selective reads and `FormatWriter` for incremental output;
//! both share the binary layout and metadata validation without depending on each other.

mod attribute_codec;
mod block_encoder;
mod codec;
mod contracts;
mod crc32;
mod error;
mod geometry;
mod layout;
mod metadata;
mod occupancy_codec;
mod quantization;
mod quantized_occupancy;
mod ranges;
mod reader;
mod storage_reader;
mod version;
mod writer;

pub use codec::{BlockCodec, CodecError, EncodedBlock, compress_block, decompress_block};
pub use contracts::{BlockEncoder, OctreeReader, OctreeWriter, PreparedOctreeWriter};
pub use crc32::{Crc32, crc32};
pub use error::FormatError;
pub use geometry::GeometryEncoding;
pub use layout::{FORMAT_VERSION, HEADER_ENVELOPE_VERSION};
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

pub use block_encoder::FormatBlockEncoder;
pub use storage_reader::FormatReader;
