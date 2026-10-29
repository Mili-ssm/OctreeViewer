/// Time source for the conversion core; the native one wraps `std::time::Instant`.
pub trait Clock {
    type Instant: Copy;
    fn now(&self) -> Self::Instant;
    fn elapsed_ns(&self, since: Self::Instant) -> u64;
}

/// Clock for tests and for hosts without a time source: every instant is `()` and every
/// elapsed time is 0 ns.
pub struct NoClock;

impl Clock for NoClock {
    type Instant = ();

    fn now(&self) {}

    fn elapsed_ns(&self, (): ()) -> u64 {
        0
    }
}
