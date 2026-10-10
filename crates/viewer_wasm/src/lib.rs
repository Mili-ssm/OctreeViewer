//! Viewer orchestration and browser bindings for point import, conversion and presentation.
//! `ViewerController` manages in-memory scenes and `LodDataset` manages streamed blocks;
//! browser bindings adapt these contracts to workers, typed arrays and WebGPU surfaces.

mod controller;
mod las_import;
mod lod_dataset;
#[cfg(target_arch = "wasm32")]
mod lod_metadata;
#[cfg(any(target_arch = "wasm32", test))]
mod lod_partition;
#[cfg(any(target_arch = "wasm32", test))]
mod lod_representatives;

pub use controller::{
    RenderInstanceSnapshot, RenderSceneSnapshot, ViewerConfig, ViewerController, ViewerError,
    ViewerStats,
};
pub use las_import::{LasImportError, LasPointChunk, LasPointStream};
pub use lod_dataset::{LodDataset, LodDatasetError, LodViewStats};

#[cfg(target_arch = "wasm32")]
mod bindings;
#[cfg(target_arch = "wasm32")]
mod browser_convert;
#[cfg(target_arch = "wasm32")]
mod browser_io;

#[cfg(target_arch = "wasm32")]
pub use bindings::{WasmLasImportSession, WasmVoxelViewer};
#[cfg(target_arch = "wasm32")]
pub use browser_convert::WasmLasOctConverter;
