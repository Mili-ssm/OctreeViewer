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
