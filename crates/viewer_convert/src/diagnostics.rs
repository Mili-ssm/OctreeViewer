use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Optional aggregate timings for one conversion invocation.
///
/// Elapsed values use a monotonic clock. Unix bounds let external process samplers correlate
/// observations with the whole phase; they do not attribute RSS or temporary bytes to internals.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConversionDiagnostics {
    /// Includes input spooling when used and construction/finalization of the spatial plan.
    pub spatial_planning: Option<DiagnosticWindow>,
    /// Whole block-stream scope; excludes final index/footer emission and file sync.
    pub block_stream: Option<DiagnosticWindow>,
    /// Sum of wall time spent preparing each parallel batch; not summed worker CPU time.
    pub block_preparation_elapsed_ns: Option<u128>,
    /// Ordered writes, spool release, and progress reporting across batches.
    pub ordered_write_and_release_elapsed_ns: Option<u128>,
    /// Validation setup and worker execution/join; does not attribute RSS to readers.
    pub output_validation: Option<DiagnosticWindow>,
    /// Per-worker read aggregates. The block memory estimate excludes transient decoder buffers.
    pub validation_workers: Option<Vec<ValidationWorkerDiagnostics>>,
    /// Logical spool record payloads processed during spatial planning; excludes physical I/O.
    pub spatial_spools: Option<SpatialSpoolDiagnostics>,
    /// Payload bytes decoded from full-resolution leaf ranges during block preparation.
    pub block_stream_source_payload_read_bytes: Option<u64>,
    /// Payload bytes decoded from LOD sample ranges during block preparation.
    pub block_stream_lod_payload_read_bytes: Option<u64>,
    /// Largest number of prepared blocks retained by one completed batch.
    pub block_stream_max_prepared_blocks: Option<u64>,
    /// Largest retained compressed payload buffer capacity for one prepared block.
    pub block_stream_max_encoded_payload_capacity_per_block_bytes: Option<u64>,
    /// Sum of retained compressed payload buffer capacities at the completed-batch boundary.
    pub block_stream_max_encoded_payload_capacity_per_batch_bytes: Option<u64>,
    /// Summed wall durations for selected spatial-planning operations; worker scopes may overlap.
    pub spatial_phases: Option<SpatialPhaseDiagnostics>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpatialPhaseDiagnostics {
    /// Time in root direct-LAS `fill_points`; source-spool generation and checkpoints are excluded.
    pub root_las_fill_elapsed_ns: u128,
    /// Time in recursive `read_records` calls for source/child spool scans.
    pub spool_scan_read_elapsed_ns: u128,
    /// Time decoding/classifying scanned records and partitioning into child spools/samples.
    pub spool_partition_and_sample_elapsed_ns: u128,
    /// Time creating and writing spatial LOD sample spools.
    pub lod_sample_write_elapsed_ns: u128,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpatialSpoolDiagnostics {
    pub source_payload_written_bytes: u64,
    pub source_payload_scanned_bytes: u64,
    pub child_payload_written_bytes: u64,
    pub child_payload_scanned_bytes: u64,
    pub lod_payload_written_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationWorkerDiagnostics {
    pub worker_index: usize,
    pub blocks_read: u64,
    pub points_read: u64,
    /// Sum of time inside successful and failed `read_block` calls; worker sums can overlap.
    pub read_elapsed_ns: u128,
    /// Largest decoded block allocation estimate observed by this worker.
    pub max_decoded_block_estimated_bytes: u64,
}

/// Monotonic elapsed time and best-effort wall-clock bounds for an observed phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiagnosticWindow {
    pub elapsed_ns: u128,
    pub start_unix_ns: Option<u128>,
    pub end_unix_ns: Option<u128>,
}

pub(crate) struct DiagnosticTimer {
    started: Instant,
    start_unix_ns: Option<u128>,
}

impl DiagnosticTimer {
    pub(crate) fn start() -> Self {
        Self {
            started: Instant::now(),
            start_unix_ns: unix_ns(),
        }
    }

    pub(crate) fn finish(self) -> DiagnosticWindow {
        let end_unix_ns = unix_ns();
        let (start_unix_ns, end_unix_ns) = match (self.start_unix_ns, end_unix_ns) {
            (Some(start), Some(end)) if end >= start => (Some(start), Some(end)),
            _ => (None, None),
        };
        DiagnosticWindow {
            elapsed_ns: self.started.elapsed().as_nanos(),
            start_unix_ns,
            end_unix_ns,
        }
    }
}

fn unix_ns() -> Option<u128> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_nanos())
}
