//! Native LAS/LAZ and local EPT conversion with bounded staging and validated publication.
//! The `convert_file*` entry points share `ConvertOptions`; progress callbacks can cancel,
//! and checkpoint entry points retain completed LAS/LAZ phases for resumption.

use std::path::Path;

use viewer_core::SpatialBounds;

use las::convert_file_inner;
use options::MAX_INPUT_BYTES_PER_BATCH;

mod checkpoint;
mod diagnostics;
mod ept;
mod ept_checkpoint;
mod error;
mod fs_util;
mod las;
mod options;
mod publish;
mod report;
mod spatial;

pub use checkpoint::{CheckpointError, CheckpointMode};
pub use diagnostics::{
    ConversionDiagnostics, DiagnosticWindow, SpatialPhaseDiagnostics, SpatialSpoolDiagnostics,
    ValidationWorkerDiagnostics,
};
pub use error::ConvertError;
pub use options::ConvertOptions;
pub use report::{ConversionProgress, ConversionReport, ConversionStage, ConversionWorkerStats};

/// Applies the same input staging limit to planning, spooling and EPT reads.
pub(crate) fn input_batch_point_limit(points_per_block: u64, record_length: usize) -> u64 {
    points_per_block.min((MAX_INPUT_BYTES_PER_BATCH / record_length.max(1)).max(1) as u64)
}

pub(crate) fn visual_scale_for_bounds(bounds: SpatialBounds) -> Result<f64, ConvertError> {
    viewer_core::las::visual_scale_for_bounds(bounds).ok_or_else(|| {
        let min = bounds.min();
        let max = bounds.max();
        let extent = (0..3)
            .map(|axis| max[axis] - min[axis])
            .fold(0.0_f64, f64::max);
        ConvertError::InvalidVisualScale(viewer_core::las::TARGET_RENDER_EXTENT / extent)
    })
}

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
