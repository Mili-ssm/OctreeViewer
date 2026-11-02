use thiserror::Error;
use viewer_core::{BlockId, NodeId};

use crate::{CodecError, MetadataError, QuantizationError};

#[derive(Debug, Error)]
pub enum FormatError {
    #[error("attribute companion is not attached")]
    AttributesUnavailable,
    #[error("I/O error while accessing the format: {0}")]
    Io(#[from] std::io::Error),
    #[error("format file is shorter than its header and footer")]
    FileTooShort,
    #[error("format header is malformed or has an invalid checksum")]
    InvalidHeader,
    #[error("provisional format is no longer supported; regenerate from LAS/LAZ/EPT")]
    ProvisionalFormat,
    #[error("format version {0} is not supported")]
    UnsupportedVersion(u16),
    #[error("format version {0} is recognized but has no supported decoder")]
    UnsupportedFormatVersion(crate::FormatVersion),
    #[error("prepared block version {block} does not match writer version {writer}")]
    PreparedVersionMismatch {
        writer: crate::FormatVersion,
        block: crate::FormatVersion,
    },
    #[error("format footer is malformed, truncated or inconsistent")]
    InvalidFooter,
    #[error("{0} checksum does not match")]
    InvalidChecksum(&'static str),
    #[error("block {0:?} checksum does not match")]
    InvalidBlockChecksum(BlockId),
    #[error("format reader limits must be greater than zero")]
    InvalidReaderLimits,
    #[error("format index needs {requested} bytes, exceeding limit {limit}")]
    IndexExceedsLimit { requested: u64, limit: u64 },
    #[error("format page has {requested} bytes, exceeding limit {limit}")]
    PageExceedsLimit { requested: u64, limit: u64 },
    #[error("format count for {kind} is {requested}, exceeding limit {limit}")]
    CountLimit {
        kind: &'static str,
        requested: u64,
        limit: u64,
    },
    #[error("format string has {requested} bytes, exceeding limit {limit}")]
    StringExceedsLimit { requested: u32, limit: u32 },
    #[error("format contains invalid UTF-8")]
    InvalidUtf8,
    #[error("format index is malformed: {0}")]
    MalformedIndex(&'static str),
    #[error("format block is malformed: {0}")]
    MalformedBlock(&'static str),
    #[error("read plan was created by a different reader")]
    PlanReaderMismatch,
    #[error("payload buffer count is {actual}, expected {expected}")]
    PayloadBufferCount { expected: usize, actual: usize },
    #[error("payload buffer length is {actual}, expected {expected}")]
    PayloadBufferLength { expected: usize, actual: usize },
    #[error("this metadata-only reader requires externally fetched payload buffers")]
    ExternalPayloadBuffersRequired,
    #[error("format index ended before a field was complete")]
    TruncatedIndex,
    #[error("format schema is invalid")]
    InvalidSchema,
    #[error("format uses unsupported tag {tag} for {field}")]
    InvalidTag { field: &'static str, tag: u8 },
    #[error("requested block {0:?} does not exist")]
    UnknownBlock(BlockId),
    #[error("requested attribute schema index {0} does not exist")]
    UnknownAttribute(u32),
    #[error("row {row_id} is outside block {block:?}")]
    UnknownRow { block: BlockId, row_id: u64 },
    #[error("invalid quantization error limit {0}")]
    InvalidQuantizationLimit(f64),
    #[error("maximum uncompressed block size must be greater than zero")]
    ZeroBlockLimit,
    #[error("output stream must be empty and positioned at offset zero")]
    OutputMustBeEmpty,
    #[error("writer cannot continue after a partial I/O failure")]
    WriterPoisoned,
    #[error("format metadata is invalid: {0}")]
    InvalidMetadata(&'static str),
    #[error("visual scale must be finite and greater than zero")]
    InvalidVisualScale,
    #[error("metadata string has {0} bytes, above the format limit")]
    StringTooLong(usize),
    #[error("attribute schema has too many entries")]
    SchemaTooLarge,
    #[error("hierarchy has too many nodes")]
    TooManyNodes,
    #[error("format contains too many blocks")]
    TooManyBlocks,
    #[error("node {0:?} contains too many block references")]
    TooManyNodeBlocks(NodeId),
    #[error("node {0:?} contains too many attribute ranges")]
    TooManyAttributeRanges(NodeId),
    #[error("node {0:?} has attribute range metadata with an invalid schema index or values")]
    InvalidAttributeRange(NodeId),
    #[error("attribute ranges are repeated for node {0:?}")]
    DuplicateNodeRanges(NodeId),
    #[error("metadata contains unknown node {0:?}")]
    UnknownNode(NodeId),
    #[error("block {block:?} is referenced more than once in node {node:?}")]
    DuplicateNodeBlock { node: NodeId, block: BlockId },
    #[error("block {0:?} is referenced by the hierarchy zero times")]
    UnreferencedBlock(BlockId),
    #[error("block {0:?} was written more than once")]
    DuplicateBlock(BlockId),
    #[error("block {0:?} was referenced but not written")]
    MissingBlock(BlockId),
    #[error("block has {points} positions but {colors} colors")]
    ColorCountMismatch { points: usize, colors: usize },
    #[error("block contains {actual} attributes, expected {expected}")]
    AttributeCountMismatch { expected: usize, actual: usize },
    #[error("block is missing attribute {0:?}")]
    MissingAttribute(String),
    #[error("block repeats attribute {0:?}")]
    DuplicateAttribute(String),
    #[error("block attribute {0:?} does not match the dataset schema")]
    AttributeSchemaMismatch(String),
    #[error("attribute {name:?} has {actual} values, expected {expected}")]
    AttributeLengthMismatch {
        name: String,
        expected: usize,
        actual: usize,
    },
    #[error("attribute {0:?} component count overflows the platform length")]
    AttributeLengthOverflow(String),
    #[error("block point count cannot be represented as u64")]
    PointCountOutOfRange,
    #[error("block payload size overflows u64")]
    BlockSizeOverflow,
    #[error("block needs {requested} raw bytes, exceeding limit {limit}")]
    BlockExceedsLimit { requested: u64, limit: u64 },
    #[error("read needs an estimated {requested} payload bytes, exceeding budget {limit}")]
    ReadBudgetExceeded { requested: u64, limit: u64 },
    #[error("size {0} cannot be represented by this platform")]
    SizeOutOfRange(u64),
    #[error("could not reserve {0} bytes for an encoded block")]
    AllocationFailed(usize),
    #[error("encoded payload has {actual} bytes, expected {expected}")]
    InternalPayloadSize { expected: usize, actual: usize },
    #[error("point count overflows for node {0:?}")]
    PointCountOverflow(NodeId),
    #[error("node {node:?} declares {declared} points but its blocks contain {blocks}")]
    NodePointCountMismatch {
        node: NodeId,
        declared: u64,
        blocks: u64,
    },
    #[error("quantization error overflow for node {0:?}")]
    GeometricErrorOverflow(NodeId),
    #[error("file size overflows u64")]
    FileSizeOverflow,
    #[error(transparent)]
    Quantization(#[from] QuantizationError),
    #[error(transparent)]
    Codec(#[from] CodecError),
    #[error(transparent)]
    Spatial(#[from] viewer_core::SpatialError),
    #[error(transparent)]
    CoordinateFrame(#[from] viewer_core::CoordinateFrameError),
    #[error(transparent)]
    Metadata(#[from] MetadataError),
    #[error(transparent)]
    Dataset(#[from] viewer_core::DatasetError),
}
