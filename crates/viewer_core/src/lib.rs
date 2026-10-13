//! Platform-independent point data, spatial selection and bounded block state.
//! `DatasetSource` supplies blocks; scheduling and LOD do not perform I/O or render.

mod block_cache;
mod dataset;
mod geometry;
mod lod_transition;
mod scheduler;
mod spatial;
mod visibility;
mod voxel_store;

pub use block_cache::{
    BlockCacheStats, CacheInsertRejection, CacheInsertResult, DecodedBlockCache,
};
pub use dataset::{
    AttributeColumn, AttributeValues, DatasetError, DatasetMetadata, DatasetSource, PointBlock,
    ResidentBlockSource,
};
pub use geometry::{
    Aabb, ColorRgb, CoordinateFrame, CoordinateFrameError, Point, PointBatch, VoxelCoord,
};
pub use lod_transition::{LodTransitionPlan, LodTransitionPlanner};
pub use scheduler::{
    BlockRequestScheduler, RequestCompletion, RequestState, RequestTicket, RequestUpdate,
    ScheduledBlockRequest, SchedulerError, ViewUpdate,
};
pub use spatial::{
    AttributeDataType, AttributeDescriptor, BlockId, DEFAULT_LOD_PRIORITY_BYTES, FrustumPlane,
    HierarchySemantics, LodPriorityFallback, LodPriorityStats, LodSelection, LodSelectionConfig,
    LodSelectionError, NodeId, SpatialBounds, SpatialError, SpatialFrustum, SpatialHierarchy,
    SpatialNode, SpatialQuery, SpatialStats, SpatialSubdivision,
};
pub use visibility::{VisibilityQuery, VisibleVoxel, VisibleVoxelBatch};
pub use voxel_store::{
    SparseVoxelStore, Voxel, VoxelStoreStats, VoxelizationConfig, VoxelizationError,
};
