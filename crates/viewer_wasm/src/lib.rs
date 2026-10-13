//! Viewer orchestration and browser bindings for point import, conversion and presentation.
//! `ViewerController` manages in-memory scenes and `LodDataset` manages streamed blocks;
//! browser bindings adapt these contracts to workers, typed arrays and WebGPU surfaces.

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
#[cfg(any(target_arch = "wasm32", test))]
mod lod_session;

pub use controller::{
    RenderInstanceSnapshot, ViewerConfig, ViewerController, ViewerError, ViewerStats,
};
pub use lod_dataset::{LodDataset, LodDatasetError, LodViewStats, LodWorkStats};

#[cfg(target_arch = "wasm32")]
mod bindings;
#[cfg(target_arch = "wasm32")]
mod browser_convert;
#[cfg(target_arch = "wasm32")]
mod browser_io;

#[cfg(target_arch = "wasm32")]
pub use bindings::WasmVoxelViewer;
#[cfg(target_arch = "wasm32")]
pub use browser_convert::WasmLasOctConverter;
