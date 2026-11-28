//! Native LAS/LAZ conversion with bounded staging and validated publication.
//! The `convert_file*` entry points share `ConvertOptions`; progress callbacks can cancel.
//! EPT input and checkpoints were removed with V8 (ADR 0061).

use std::path::Path;

use las_source::convert_file_inner;

mod allocator;
mod diagnostic_timer;
mod executor;
mod file_spool_store;
mod fs_util;
mod las_source;
mod pipeline;
mod publish;
#[cfg(test)]
mod test_options;
mod verify;
mod workspace;

pub use verify::{VerifyReport, verify_file};
pub use viewer_convert_core::{
    ConversionDiagnostics, ConversionProgress, ConversionReport, ConversionStage,
    ConversionWorkerStats, ConvertError, ConvertOptions, DiagnosticWindow, SpatialPhaseDiagnostics,
    SpatialSpoolDiagnostics, ValidationWorkerDiagnostics,
};

/// Converts LAS/LAZ to the native format with replacement hierarchy samples.
/// Point payload memory is bounded by the configured block size and spatial-index workspace.
pub fn convert_file(
    input_path: &Path,
    output_path: &Path,
    options: ConvertOptions,
) -> Result<ConversionReport, ConvertError> {
    convert_file_with_progress(input_path, output_path, options, |_| true)
}

/// Test support for `tools/format_parity`: the `leaf_count` largest source-point blocks followed
/// by the first `lod_count` LOD blocks of a LAS/LAZ plan, exactly as the converter would feed them
/// to the encoder. Not part of the stable API.
#[doc(hidden)]
pub fn plan_sample_blocks(
    input_path: &Path,
    workspace_anchor: &Path,
    options: ConvertOptions,
    leaf_count: usize,
    lod_count: usize,
) -> Result<Vec<viewer_format::SourcePointBlock>, ConvertError> {
    las_source::plan_sample_blocks(input_path, workspace_anchor, options, leaf_count, lod_count)
}

/// Converts a file and reports progress after each phase and completed block.
/// Return `false` from the callback to cancel before the output is published.
pub fn convert_file_with_progress<F>(
    input_path: &Path,
    output_path: &Path,
    options: ConvertOptions,
    mut on_progress: F,
) -> Result<ConversionReport, ConvertError>
where
    F: FnMut(ConversionProgress) -> bool,
{
    convert_file_inner(input_path, output_path, options, &mut on_progress, None)
}

/// Converts LAS/LAZ while collecting bounded aggregate phase timings for diagnostics.
///
/// External RSS and temporary-file samples may be correlated with the returned windows, but
/// remain process-wide observations.
pub fn convert_file_with_diagnostics<F>(
    input_path: &Path,
    output_path: &Path,
    options: ConvertOptions,
    mut on_progress: F,
) -> Result<(ConversionReport, ConversionDiagnostics), ConvertError>
where
    F: FnMut(ConversionProgress) -> bool,
{
    let mut diagnostics = ConversionDiagnostics::default();
    let report = convert_file_inner(
        input_path,
        output_path,
        options,
        &mut on_progress,
        Some(&mut diagnostics),
    )?;
    Ok((report, diagnostics))
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "global_frame_tests.rs"]
mod global_frame_tests;
#[cfg(test)]
#[path = "las_extra_tests.rs"]
mod las_extra_tests;
#[cfg(test)]
#[path = "paged_tests.rs"]
mod paged_tests;
#[cfg(test)]
#[path = "viewer_scene_tests.rs"]
mod viewer_scene_tests;
