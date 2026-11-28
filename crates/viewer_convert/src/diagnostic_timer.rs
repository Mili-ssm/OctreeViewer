//! Native wall-clock timer behind `DiagnosticWindow`; stays in the shell, not in the core.

use std::time::{Instant, SystemTime, UNIX_EPOCH};

use viewer_convert_core::diagnostics::DiagnosticWindow;

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
