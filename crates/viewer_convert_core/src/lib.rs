//! Conversion core shared by the native converter and the web host: options, errors, source
//! inspection, the spatial planner and the block preparation helpers. It has no filesystem,
//! thread or wall-clock access (clippy denies those APIs here); hosts supply them through the
//! `Clock`, `SpoolStore` and `SourceFactory` seams.
//!
//! Only the items re-exported below are stable. The modules are public so the native shell can
//! reach the internals, and are `#[doc(hidden)]`.

#[doc(hidden)]
pub mod blocks;
#[doc(hidden)]
pub mod crs;
#[doc(hidden)]
pub mod diagnostics;
#[doc(hidden)]
pub mod error;
#[doc(hidden)]
pub mod limits;
#[doc(hidden)]
pub mod options;
#[doc(hidden)]
pub mod pipeline;
#[doc(hidden)]
pub mod report;
#[doc(hidden)]
pub mod serial;
#[doc(hidden)]
pub mod source;
#[doc(hidden)]
pub mod spatial;

pub use diagnostics::{
    ConversionDiagnostics, DiagnosticWindow, SpatialPhaseDiagnostics, SpatialSpoolDiagnostics,
    ValidationWorkerDiagnostics,
};
pub use error::{CheckpointError, ConvertError};
pub use options::ConvertOptions;
pub use pipeline::clock::NoClock;
pub use pipeline::hybrid::{
    HYBRID_PAGE_BYTES, HYBRID_ZONE_BYTES, HybridConfig, HybridReader, HybridSpoolStore,
    HybridStats, PageAllocator, PageTier, RamTier, SegmentKey, SpoolEstimate, VecPageTier,
    VecRamTier, estimate_spools, inspect_source,
};
pub use pipeline::point_source::{ByteSource, LasFactory, MemBytes, SourceFactory};
pub use pipeline::spool_store::{MemSpoolStore, SpoolStore};
pub use report::{ConversionProgress, ConversionReport, ConversionStage, ConversionWorkerStats};
pub use serial::{
    PlanSummary, SerialOutput, convert_serial, validate_output_serial, written_block_ids,
};

#[cfg(test)]
mod range_tests;
