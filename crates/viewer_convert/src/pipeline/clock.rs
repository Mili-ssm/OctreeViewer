/// Time source for the conversion core; the native one wraps `std::time::Instant`.
pub(crate) trait Clock {
    type Instant: Copy;
    fn now(&self) -> Self::Instant;
    fn elapsed_ns(&self, since: Self::Instant) -> u64;
}

pub(crate) struct SystemClock;

impl Clock for SystemClock {
    type Instant = std::time::Instant;

    fn now(&self) -> Self::Instant {
        std::time::Instant::now()
    }

    fn elapsed_ns(&self, since: Self::Instant) -> u64 {
        u64::try_from(since.elapsed().as_nanos()).unwrap_or(u64::MAX)
    }
}
