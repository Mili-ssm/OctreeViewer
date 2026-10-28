use crate::{ConversionDiagnostics, ConvertError};

pub(crate) fn aggregate_geometry_codec_diagnostics(
    diagnostics: &mut ConversionDiagnostics,
    metric: viewer_format::GeometryCodecDiagnostics,
) {
    let candidates = diagnostics.geometry_candidates.get_or_insert_with(Vec::new);
    for candidate in metric.candidates {
        if let Some(total) = candidates
            .iter_mut()
            .find(|total| total.kind == candidate.kind)
        {
            total.attempts += candidate.attempts;
            total.encoding_elapsed_ns += candidate.encoding_elapsed_ns;
            total.compression_elapsed_ns += candidate.compression_elapsed_ns;
            total.raw_bytes += candidate.raw_bytes;
            total.compressed_bytes += candidate.compressed_bytes;
            total.selected_winners += candidate.selected_winners;
            total.selected_raw_bytes += candidate.selected_raw_bytes;
            total.selected_compressed_bytes += candidate.selected_compressed_bytes;
        } else {
            candidates.push(candidate);
        }
    }
    let builds = diagnostics.occupancy_builds.get_or_insert_with(Vec::new);
    for build in metric.occupancy_builds {
        if let Some(total) = builds
            .iter_mut()
            .find(|total| total.residual_bits == build.residual_bits)
        {
            total.attempts += build.attempts;
            total.elapsed_ns += build.elapsed_ns;
        } else {
            builds.push(build);
        }
    }
}

/// Concurrency ceiling selected from the requested count, block count and memory budget.
/// For preparation this is the retained batch width; Rayon may use fewer threads.
/// The byte estimate is a planning model, not observed RSS or retained payload size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConversionWorkerStats {
    pub worker_limit: usize,
    pub estimated_bytes_per_worker: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversionReport {
    pub point_count: u64,
    pub block_count: u64,
    pub output_bytes: u64,
    pub estimated_peak_memory_bytes: u64,
    pub omitted_dimensions: Vec<String>,
    /// None when this invocation skips batched preparation (resume or serial EPT ingestion).
    pub block_preparation: Option<ConversionWorkerStats>,
    pub output_validation: ConversionWorkerStats,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversionStage {
    InspectingInput,
    PlanningMetadata,
    PlanningSpatialIndex,
    BuildingSpatialHierarchy,
    ReadingAndWritingBlocks,
    ValidatingOutput,
    PublishingOutput,
    Complete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConversionProgress {
    pub stage: ConversionStage,
    pub points_processed: u64,
    pub total_points: u64,
    pub bytes_written: u64,
}

pub(crate) fn emit_progress(
    on_progress: &mut dyn FnMut(ConversionProgress) -> bool,
    progress: ConversionProgress,
    points_processed: u64,
) -> Result<(), ConvertError> {
    if on_progress(progress) {
        Ok(())
    } else {
        Err(ConvertError::Cancelled { points_processed })
    }
}
