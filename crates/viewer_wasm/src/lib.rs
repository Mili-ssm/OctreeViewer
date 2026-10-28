//! Viewer orchestration and browser bindings for point import, conversion and presentation.
//! `ViewerController` manages in-memory scenes and `LodDataset` manages streamed blocks;
//! browser bindings adapt these contracts to workers, typed arrays and WebGPU surfaces.

// viewer_format is only used by wasm32/test modules; this marks it used for plain host builds.
#[cfg(not(any(target_arch = "wasm32", test)))]
use viewer_format as _;

#[cfg(any(target_arch = "wasm32", test))]
mod block_loading;
mod controller;
mod lod_dataset;
#[cfg(target_arch = "wasm32")]
mod lod_metadata;
#[cfg(any(target_arch = "wasm32", test))]
mod lod_partition;
#[cfg(any(target_arch = "wasm32", test))]
mod lod_representatives;
#[cfg(target_arch = "wasm32")]
mod lod_request_scheduler;
#[cfg(any(target_arch = "wasm32", test))]
mod lod_session;

pub use controller::{
    RenderInstanceSnapshot, ViewerConfig, ViewerController, ViewerError, ViewerStats,
};
pub use lod_dataset::{LodDataset, LodDatasetError, LodViewStats, LodWorkStats};

#[cfg(target_arch = "wasm32")]
mod async_format_reader;
#[cfg(target_arch = "wasm32")]
mod bindings;
#[cfg(target_arch = "wasm32")]
mod browser_convert;
#[cfg(target_arch = "wasm32")]
mod browser_io;

#[cfg(target_arch = "wasm32")]
pub use async_format_reader::{
    WasmAsyncFormatReader, WasmAsyncLodReadRequest, WasmAsyncLodReadResult, WasmAsyncReadBudget,
    WasmAsyncReadCancelToken,
};
#[cfg(target_arch = "wasm32")]
pub use bindings::WasmVoxelViewer;
#[cfg(target_arch = "wasm32")]
pub use browser_convert::WasmLasOctConverter;
#[cfg(target_arch = "wasm32")]
pub use lod_request_scheduler::WasmBlockRequestScheduler;
