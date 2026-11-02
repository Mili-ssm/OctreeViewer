//! Viewer orchestration and browser bindings for point import, conversion and presentation.
//! `ViewerController` manages in-memory scenes and `LodDataset` manages streamed blocks;
//! browser bindings adapt these contracts to workers, typed arrays and WebGPU surfaces.

#[cfg(any(target_arch = "wasm32", test))]
mod block_loading;
mod controller;
mod lod_dataset;
#[cfg(any(target_arch = "wasm32", test))]
mod lod_driver;
#[cfg(target_arch = "wasm32")]
mod lod_metadata;
mod lod_pages;
#[cfg(target_arch = "wasm32")]
mod lod_request_scheduler;
#[cfg(any(target_arch = "wasm32", test))]
mod lod_session;
#[cfg(any(target_arch = "wasm32", test))]
mod scene_builder;

pub use controller::{
    RenderInstanceSnapshot, ViewerConfig, ViewerController, ViewerError, ViewerStats,
};
pub use lod_dataset::{
    LodBuiltScene, LodDataset, LodDatasetError, LodViewStats, LodViewUpdate, LodWorkStats,
};
pub use lod_pages::{DEFAULT_PAGE_BUDGET_BYTES, PageInsert, PageRelease};

#[cfg(target_arch = "wasm32")]
mod async_format_reader;
#[cfg(target_arch = "wasm32")]
mod bindings;
#[cfg(target_arch = "wasm32")]
mod browser_io;
#[cfg(target_arch = "wasm32")]
mod core_convert;
#[cfg(target_arch = "wasm32")]
mod opfs;
#[cfg(target_arch = "wasm32")]
mod spool_host;

#[cfg(target_arch = "wasm32")]
pub use async_format_reader::{
    WasmAsyncFormatReader, WasmAsyncLodReadRequest, WasmAsyncLodReadResult, WasmAsyncReadBudget,
    WasmAsyncReadCancelToken, WasmPageGraft,
};
#[cfg(target_arch = "wasm32")]
pub use bindings::{WasmSceneBuilder, WasmVoxelViewer};
#[cfg(target_arch = "wasm32")]
pub use core_convert::WasmCoreConverter;
#[cfg(target_arch = "wasm32")]
pub use lod_request_scheduler::WasmBlockRequestScheduler;
