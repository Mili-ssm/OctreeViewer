//! Native implementations of the seams the conversion core defines (the traits live in
//! `viewer_convert_core`).

use viewer_convert_core::pipeline::clock::Clock;

pub struct SystemClock;

impl Clock for SystemClock {
    type Instant = std::time::Instant;

    fn now(&self) -> Self::Instant {
        std::time::Instant::now()
    }

    fn elapsed_ns(&self, since: Self::Instant) -> u64 {
        u64::try_from(since.elapsed().as_nanos()).unwrap_or(u64::MAX)
    }
}

#[cfg(test)]
mod node_job_tests;
#[cfg(test)]
mod point_source_tests;
