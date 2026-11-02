use std::{io, path::PathBuf};

use thiserror::Error;

use crate::options::{MAX_BLOCKS, MAX_POINTS_PER_BLOCK, MAX_SPATIAL_WORKERS};

#[derive(Debug, Error)]
pub enum ConvertError {
    #[error(
        "coordinate output and attribute companion must have different paths; use .oct for coordinates"
    )]
    ConflictingOutputPaths,
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("LAS/LAZ error: {0}")]
    Las(#[from] las::Error),
    #[error("LAZ decoder panicked on corrupt point data: {message}")]
    LasDecoderPanicked {
        message: String,
        /// Global index of the first point of the failing call; `None` outside source reads.
        point_index: Option<u64>,
    },
    #[error("unsafe LAS/LAZ header: {0}")]
    LasPreflight(#[from] viewer_core::las::LasPreflightError),
    #[error("unsafe LAZ chunk table: {field} = {value} is outside the bound {bound}")]
    LazChunkTable {
        field: &'static str,
        value: u64,
        bound: u64,
    },
    #[error(transparent)]
    Format(#[from] viewer_format::FormatError),
    #[error(transparent)]
    Spatial(#[from] viewer_core::SpatialError),
    #[error(transparent)]
    CoordinateFrame(#[from] viewer_core::CoordinateFrameError),
    #[error(transparent)]
    Dataset(#[from] viewer_core::DatasetError),
    #[error(transparent)]
    Metadata(#[from] viewer_format::MetadataError),
    #[error("points per block must be between 1 and {MAX_POINTS_PER_BLOCK}, got {0}")]
    InvalidBlockPointLimit(u64),
    #[error("maximum quantization error must be finite and non-negative, got {0}")]
    InvalidQuantizationError(f64),
    #[error("visual scale is outside the supported range: {0}")]
    InvalidVisualScale(f64),
    #[error(
        "checkpoint candidate has visual scale {actual}, expected {expected}; restart conversion with --checkpoint"
    )]
    StaleVisualScaleCheckpoint { expected: f64, actual: f64 },
    #[error("memory budget must be greater than zero")]
    InvalidMemoryBudget,
    #[error("spatial worker count must be between 1 and {MAX_SPATIAL_WORKERS}, got {0}")]
    InvalidSpatialWorkerCount(usize),
    #[error("spatial divisions per axis must be 2, 4, or 8, got {0}")]
    InvalidSpatialDivisions(u8),
    #[error("root partition ranges must be between 0 and 8, got {0}")]
    InvalidRootRanges(usize),
    #[error("sample grid per axis must be between 1 and 256, got {0}")]
    InvalidSampleGrid(usize),
    #[error("LAS dimensions do not match the planned output attribute schema")]
    InvalidAttributeSchema,
    #[error("estimated controlled memory is {requested} bytes, exceeding budget {limit} bytes")]
    MemoryBudgetExceeded { requested: u64, limit: u64 },
    #[error("conversion cancelled after processing {points_processed} points")]
    Cancelled { points_processed: u64 },
    #[error("LAS/LAZ header bounds are invalid")]
    InvalidHeaderBounds,
    #[error("LAS/LAZ point count requires {0} blocks, exceeding the limit of {MAX_BLOCKS}")]
    TooManyBlocks(u64),
    #[error(
        "output would hold up to {ranges} attribute ranges ({nodes} nodes x {attributes} attributes), more than the {limit} the default reader accepts"
    )]
    TooManyAttributeRanges {
        nodes: u64,
        attributes: u64,
        ranges: u64,
        limit: u64,
    },
    #[error("LAS/LAZ point data ended after {actual} points; header declared {expected}")]
    PointCountMismatch { expected: u64, actual: u64 },
    #[error("unsupported EPT dataset: {0}")]
    UnsupportedEpt(String),
    #[error("invalid EPT dataset: {0}")]
    InvalidEpt(String),
    #[error("{0} is fixed by the output format version and cannot be overridden")]
    FixedByFormat(&'static str),
    #[error("{0} cannot be written as v7.4-pre")]
    UnsupportedForGlobalFormat(&'static str),
    #[error("coordinate magnitude {0} exceeds the converter limit of 2^35 CRS units for v7.4-pre")]
    GlobalCoordinateRange(f64),
    #[error("conversion size or identifier count overflowed")]
    SizeOverflow,
    #[error("point {point_index} at {position:?} is outside the LAS/LAZ header bounds")]
    PointOutsideHeaderBounds {
        point_index: u64,
        position: [f64; 3],
    },
    #[error("output path already exists: {0}")]
    OutputExists(PathBuf),
    #[error("could not allocate a temporary output path beside {0}")]
    TemporaryPathUnavailable(PathBuf),
    #[error(transparent)]
    Checkpoint(#[from] CheckpointError),
}

#[derive(Debug, Error)]
pub enum CheckpointError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("checkpoint already exists at {0}; use --resume to continue it")]
    AlreadyExists(PathBuf),
    #[error("no checkpoint exists at {0}; use --checkpoint to start a resumable conversion")]
    Missing(PathBuf),
    #[error("checkpoint is invalid: {0}")]
    Invalid(String),
    #[error("checkpoint does not match the source or conversion options")]
    Mismatch,
    #[error("checkpoint is already being used by another converter")]
    Locked,
    #[error("checkpoint cleanup failed at {path}: {source}")]
    Cleanup { path: PathBuf, source: io::Error },
}
