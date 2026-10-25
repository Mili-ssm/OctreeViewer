//! Shared RAII reservations for bounded selective reads.
//!
//! Reservations account for the requested logical read buffers only. They do not include
//! caller-created clones, metadata, allocator/runtime overhead, caches, or process RSS.

use std::sync::{Arc, Mutex, MutexGuard};

use crate::{FormatError, SelectedRead};

#[derive(Debug)]
struct State {
    limit: u64,
    reserved: u64,
    peak: u64,
}

/// Thread-safe budget shared by selective-read reservations.
#[derive(Clone, Debug)]
pub struct ReadBudget {
    state: Arc<Mutex<State>>,
    #[cfg(all(feature = "async-reader", not(target_arch = "wasm32")))]
    notifier: Arc<tokio::sync::Notify>,
}

impl ReadBudget {
    /// Creates a budget. A zero limit still permits zero-byte reservations.
    pub fn new(limit: u64) -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                limit,
                reserved: 0,
                peak: 0,
            })),
            #[cfg(all(feature = "async-reader", not(target_arch = "wasm32")))]
            notifier: Arc::new(tokio::sync::Notify::new()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // The critical sections mutate only counters and invoke no caller code. If a panic
        // nevertheless poisons the mutex, recover its state so Drop can release reservations.
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn limit_bytes(&self) -> u64 {
        self.lock().limit
    }

    pub fn reserved_bytes(&self) -> u64 {
        self.lock().reserved
    }

    pub fn peak_reserved_bytes(&self) -> u64 {
        self.lock().peak
    }

    #[cfg(all(feature = "async-reader", not(target_arch = "wasm32")))]
    pub(crate) fn notifier(&self) -> &tokio::sync::Notify {
        &self.notifier
    }

    /// Reserves bytes, returns `None` when the budget is currently occupied, and errors when
    /// this request can never fit. Zero-byte reservations are valid, including at limit zero.
    pub fn try_reserve(&self, bytes: u64) -> Result<Option<ReadReservation>, FormatError> {
        let mut state = self.lock();
        if bytes > state.limit {
            return Err(FormatError::ReadBudgetExceeded {
                requested: bytes,
                limit: state.limit,
            });
        }
        let available = state.limit - state.reserved;
        if bytes > available {
            return Ok(None);
        }
        state.reserved = state
            .reserved
            .checked_add(bytes)
            .ok_or(FormatError::BlockSizeOverflow)?;
        state.peak = state.peak.max(state.reserved);
        Ok(Some(ReadReservation {
            state: Arc::clone(&self.state),
            bytes,
            #[cfg(all(feature = "async-reader", not(target_arch = "wasm32")))]
            notifier: Arc::clone(&self.notifier),
        }))
    }
}

/// A non-cloneable RAII reservation that may move between threads.
#[derive(Debug)]
pub struct ReadReservation {
    state: Arc<Mutex<State>>,
    bytes: u64,
    #[cfg(all(feature = "async-reader", not(target_arch = "wasm32")))]
    notifier: Arc<tokio::sync::Notify>,
}

impl ReadReservation {
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Releases part of this reservation. Increasing it requires a new budget reservation.
    pub(crate) fn shrink_to(&mut self, bytes: u64) -> Result<(), FormatError> {
        if bytes > self.bytes {
            return Err(FormatError::ReadBudgetExceeded {
                requested: bytes,
                limit: self.bytes,
            });
        }
        let release = self.bytes - bytes;
        if release != 0 {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.reserved = state
                .reserved
                .checked_sub(release)
                .ok_or(FormatError::BlockSizeOverflow)?;
            self.bytes = bytes;
            // Waking tasks can run user-provided wakers: never notify under the budget lock.
            drop(state);
            #[cfg(all(feature = "async-reader", not(target_arch = "wasm32")))]
            self.notifier.notify_waiters();
        }
        Ok(())
    }
}

impl Drop for ReadReservation {
    fn drop(&mut self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Invariant: every live reservation is reflected in `reserved`; Drop runs once because
        // reservations are not Clone, and moving one transfers its sole ownership.
        state.reserved = state
            .reserved
            .checked_sub(self.bytes)
            .expect("live read reservation must be included in budget total");
        drop(state);
        #[cfg(all(feature = "async-reader", not(target_arch = "wasm32")))]
        if self.bytes != 0 {
            self.notifier.notify_waiters();
        }
    }
}

/// A materialized read that keeps its budget reservation alive for the result's lifetime.
#[derive(Debug)]
pub struct BudgetedRead {
    // Field order intentionally destroys the result before releasing its reservation.
    result: SelectedRead,
    reservation: ReadReservation,
}

impl BudgetedRead {
    /// Transfers the completed result into a budgeted owner and releases unused estimate bytes.
    pub(crate) fn new(
        result: SelectedRead,
        reservation: ReadReservation,
        output_bytes: u64,
    ) -> Result<Self, FormatError> {
        let mut completed = Self {
            result,
            reservation,
        };
        completed.reservation.shrink_to(output_bytes)?;
        Ok(completed)
    }

    pub fn selected(&self) -> &SelectedRead {
        &self.result
    }

    pub fn reserved_bytes(&self) -> u64 {
        self.reservation.bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Barrier, mpsc};
    use viewer_core::PointBatch;

    #[test]
    fn zero_exact_near_max_and_unavailable_reservations() {
        let zero = ReadBudget::new(0);
        let zero_reservation = zero.try_reserve(0).unwrap().unwrap();
        assert!(zero.try_reserve(1).is_err());
        drop(zero_reservation);

        let budget = ReadBudget::new(10);
        let first = budget.try_reserve(6).unwrap().unwrap();
        assert!(budget.try_reserve(5).unwrap().is_none());
        let exact = budget.try_reserve(4).unwrap().unwrap();
        assert_eq!(budget.reserved_bytes(), 10);
        assert_eq!(budget.peak_reserved_bytes(), 10);
        drop((first, exact));
        assert_eq!(budget.reserved_bytes(), 0);

        let near_max = ReadBudget::new(u64::MAX);
        let all = near_max.try_reserve(u64::MAX).unwrap().unwrap();
        assert!(near_max.try_reserve(1).unwrap().is_none());
        assert_eq!(near_max.reserved_bytes(), u64::MAX);
        drop(all);
        assert_eq!(near_max.reserved_bytes(), 0);
    }

    #[test]
    fn shrink_releases_only_the_difference_and_rejects_growth() {
        let budget = ReadBudget::new(20);
        let mut reservation = budget.try_reserve(12).unwrap().unwrap();
        reservation.shrink_to(5).unwrap();
        assert_eq!(reservation.bytes(), 5);
        assert_eq!(budget.reserved_bytes(), 5);
        assert!(reservation.shrink_to(6).is_err());
        assert_eq!(budget.reserved_bytes(), 5);
        drop(reservation);
        assert_eq!(budget.reserved_bytes(), 0);
    }

    #[test]
    fn reservation_drop_and_budgeted_result_release_quota() {
        let budget = ReadBudget::new(32);
        let reservation = budget.try_reserve(16).unwrap().unwrap();
        let result = SelectedRead::Positions(PointBatch::new(vec![]));
        let read = BudgetedRead::new(result, reservation, 4).unwrap();
        assert_eq!(read.reserved_bytes(), 4);
        assert_eq!(budget.reserved_bytes(), 4);
        assert!(matches!(read.selected(), SelectedRead::Positions(points) if points.is_empty()));
        drop(read);
        assert_eq!(budget.reserved_bytes(), 0);

        let reservation = budget.try_reserve(8).unwrap().unwrap();
        assert!(
            BudgetedRead::new(
                SelectedRead::Positions(PointBatch::new(vec![])),
                reservation,
                9
            )
            .is_err()
        );
        assert_eq!(budget.reserved_bytes(), 0);
    }

    #[test]
    fn overlapping_threads_cannot_overbook_budget() {
        let budget = ReadBudget::new(10);
        let ready = Arc::new(Barrier::new(3));
        let (tx, rx) = mpsc::channel();
        let mut joins = Vec::new();
        for _ in 0..2 {
            let budget = budget.clone();
            let ready = Arc::clone(&ready);
            let tx = tx.clone();
            joins.push(std::thread::spawn(move || {
                let reservation = budget.try_reserve(7).unwrap();
                ready.wait();
                tx.send(reservation.is_some()).unwrap();
                drop(reservation);
            }));
        }
        drop(tx);
        ready.wait();
        let outcomes: Vec<_> = rx.iter().collect();
        for join in joins {
            join.join().unwrap();
        }
        assert_eq!(outcomes.iter().filter(|reserved| **reserved).count(), 1);
        assert_eq!(budget.reserved_bytes(), 0);
        assert_eq!(budget.peak_reserved_bytes(), 7);
    }
}
