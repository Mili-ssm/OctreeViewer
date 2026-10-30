//! Hybrid spool store: RAM segments for the hot spools, fixed-size pages in spill zones for the
//! rest. The tiers are dumb (keyed byte stores); the store owns lifecycle, accounting and (in a
//! later task) the spill policy.

pub mod estimate;
pub mod pages;
pub mod store;
pub mod tiers;

pub use estimate::{SpoolEstimate, estimate_spools, inspect_source};
pub use pages::PageAllocator;
pub use store::{
    HYBRID_PAGE_BYTES, HYBRID_ZONE_BYTES, HybridConfig, HybridReader, HybridSpoolStore,
    HybridStats, SEGMENT_MAX_BYTES, SEGMENT_MIN_BYTES,
};
pub use tiers::{PageTier, RamTier, SegmentKey, VecPageTier, VecRamTier, storage_full};
