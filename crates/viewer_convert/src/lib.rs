//! Native LAS/LAZ and local EPT conversion with bounded staging and validated publication.
//! The `convert_file*` entry points share `ConvertOptions`; progress callbacks can cancel,
//! and checkpoint entry points retain completed LAS/LAZ phases for resumption.

use std::path::Path;

use las_source::convert_file_inner;

mod allocator;
mod blocks;
mod checkpoint;
mod diagnostic_timer;
mod diagnostics;
mod ept;
mod ept_checkpoint;
mod error;
mod executor;
mod file_spool_store;
mod fs_util;
mod las_source;
mod limits;
mod options;
mod pipeline;
mod publish;
mod report;
mod source;
mod spatial;
mod workspace;

pub use checkpoint::CheckpointMode;
pub use viewer_convert_core::{
    CheckpointError, ConversionDiagnostics, ConversionProgress, ConversionReport, ConversionStage,
    ConversionWorkerStats, ConvertError, ConvertOptions, DiagnosticWindow, SpatialPhaseDiagnostics,
    SpatialSpoolDiagnostics, ValidationWorkerDiagnostics,
};

/// Converts LAS/LAZ or a local laszip EPT dataset to the native format.
/// LAS/LAZ output uses replacement hierarchy samples; EPT output preserves its additive hierarchy.
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
    convert_file_inner(
        input_path,
        output_path,
        options,
        None,
        &mut on_progress,
        None,
    )
}

/// Converts LAS/LAZ while collecting bounded aggregate phase timings for diagnostics.
///
/// External RSS and temporary-file samples may be correlated with the returned windows, but
/// remain process-wide observations. EPT reports phase windows but no internal worker or spool
/// attribution because its current reader pipeline is serial per block.
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
        None,
        &mut on_progress,
        Some(&mut diagnostics),
    )?;
    Ok((report, diagnostics))
}

/// Converts a file while retaining completed phase checkpoints for explicit resume.
pub fn convert_file_with_checkpoint<F>(
    input_path: &Path,
    output_path: &Path,
    options: ConvertOptions,
    mode: CheckpointMode,
    mut on_progress: F,
) -> Result<ConversionReport, ConvertError>
where
    F: FnMut(ConversionProgress) -> bool,
{
    convert_file_inner(
        input_path,
        output_path,
        options,
        Some(mode),
        &mut on_progress,
        None,
    )
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "v7_4_tests.rs"]
mod v7_4_tests;
#[cfg(test)]
#[path = "v7_4_viewer_tests.rs"]
mod v7_4_viewer_tests;
