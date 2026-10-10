//! Versioned `.oct` storage over `Read + Seek` and `Write + Seek` streams.
//! Use `FormatReader` for selective reads and `FormatWriter` for incremental output;
//! both share the binary layout and metadata validation without depending on each other.

mod codec;
mod crc32;
mod error;
mod layout;
mod metadata;
mod quantization;
mod ranges;
mod reader;
mod writer;

pub use codec::{BlockCodec, CodecError, EncodedBlock, compress_block, decompress_block};
pub use crc32::{Crc32, crc32};
pub use error::FormatError;
pub use layout::FORMAT_VERSION;
pub use metadata::{FormatMetadata, MetadataError, NodeAttributeRange, SourcePointBlock};
pub use quantization::{
    QuantizationError, QuantizationTransform, QuantizedCoordinates, quantize_local_positions,
};
pub use reader::{
    BlockPayloadKind, BlockPayloadStorageStats, BlockStorageStats, FormatReader, FormatReaderLimits,
};
pub use writer::{FormatWriter, FormatWriterOptions, PreparedPointBlock, prepare_point_block};
