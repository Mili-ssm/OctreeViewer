//! Native wall clock; the `Clock` trait lives in the core.

pub(crate) use viewer_convert_core::pipeline::clock::Clock;

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
