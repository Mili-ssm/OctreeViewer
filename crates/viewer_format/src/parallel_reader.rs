//! Bounded parallel execution of selective reads over independent reader cursors.
//!
//! Each invocation reserves a window before starting payload I/O, joins the whole window,
//! then delivers results in request order. Resume after a blocked/cancelled/error prefix with
//! `requests[progress.delivered..]`. Callers own reader forks/snapshots and any clones they make;
//! caller-created copies are outside the quota. The estimate is not RSS, async I/O, or a bound on
//! metadata, allocator/runtime overhead, caches, or the caller's retained copies.

use std::{
    io::{Read, Seek},
    ops::ControlFlow,
    panic::{AssertUnwindSafe, catch_unwind},
    thread,
};

use thiserror::Error;
use viewer_core::BlockId;

use crate::{
    BudgetedRead, FormatError, FormatReader, ReadBudget, ReadReservation, ReadSelection,
    SelectedRead,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParallelReadStatus {
    Complete,
    Blocked,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParallelReadProgress {
    pub delivered: usize,
    pub status: ParallelReadStatus,
}

#[derive(Debug, Error)]
pub enum ParallelReadError {
    #[error("parallel selective read needs at least one reader")]
    NoReaders,
    #[error("reader {reader_index} does not share the validated format index")]
    DifferentIndex { reader_index: usize },
    #[error("request {request_index} failed after {delivered} delivered results: {source}")]
    Read {
        request_index: usize,
        delivered: usize,
        #[source]
        source: FormatError,
    },
    #[error(
        "reader worker panicked on request {request_index} after {delivered} delivered results"
    )]
    WorkerPanicked {
        request_index: usize,
        delivered: usize,
    },
}

enum WorkerOutcome {
    Read(Result<BudgetedRead, FormatError>),
    Panicked,
}

struct Job {
    request_index: usize,
    block_id: BlockId,
    selection: ReadSelection,
    reservation: ReadReservation,
}

/// Reads a prefix of requests in bounded windows and delivers completed results in input order.
///
/// The full reader set must share one validated index. Reservations are acquired before workers
/// start; readers in a window do payload I/O concurrently, then all join before the callback.
/// `Blocked` reports that the next request could not fit alongside existing reservations, while
/// `Cancelled` includes the result passed to the callback that returned `Break`.
pub fn read_selected_parallel<R, F>(
    readers: &mut [FormatReader<R>],
    requests: &[(BlockId, ReadSelection)],
    budget: &ReadBudget,
    mut consume: F,
) -> Result<ParallelReadProgress, ParallelReadError>
where
    R: Read + Seek + Send,
    F: FnMut(usize, &SelectedRead) -> ControlFlow<()>,
{
    if requests.is_empty() {
        return Ok(ParallelReadProgress {
            delivered: 0,
            status: ParallelReadStatus::Complete,
        });
    }
    if readers.is_empty() {
        return Err(ParallelReadError::NoReaders);
    }
    for (reader_index, reader) in readers.iter().enumerate().skip(1) {
        if !readers[0].shares_index_with(reader) {
            return Err(ParallelReadError::DifferentIndex { reader_index });
        }
    }

    let mut delivered = 0;
    while delivered < requests.len() {
        let window_end = delivered.saturating_add(readers.len()).min(requests.len());
        let mut jobs = Vec::with_capacity(window_end - delivered);
        for (request_index, &(block_id, selection)) in
            requests.iter().enumerate().take(window_end).skip(delivered)
        {
            let plan = match readers[0].plan_read(block_id, selection) {
                Ok(plan) => plan,
                Err(source) => {
                    return Err(ParallelReadError::Read {
                        request_index,
                        delivered,
                        source,
                    });
                }
            };
            match budget.try_reserve(plan.memory().peak_bytes()) {
                Ok(Some(reservation)) => jobs.push(Job {
                    request_index,
                    block_id,
                    selection,
                    reservation,
                }),
                Ok(None) => {
                    break;
                }
                Err(source) => {
                    return Err(ParallelReadError::Read {
                        request_index,
                        delivered,
                        source,
                    });
                }
            }
        }

        if jobs.is_empty() {
            return Ok(ParallelReadProgress {
                delivered,
                status: ParallelReadStatus::Blocked,
            });
        }

        let mut outcomes = Vec::with_capacity(jobs.len());
        if jobs.len() == 1 {
            let job = jobs.pop().expect("one job was checked");
            let outcome = catch_unwind(AssertUnwindSafe(|| {
                readers[0].read_reserved(job.block_id, job.selection, job.reservation)
            }))
            .map_or(WorkerOutcome::Panicked, WorkerOutcome::Read);
            outcomes.push((job.request_index, outcome));
        } else {
            thread::scope(|scope| {
                let mut handles = Vec::with_capacity(jobs.len());
                for (reader, job) in readers.iter_mut().zip(jobs) {
                    let request_index = job.request_index;
                    handles.push((
                        request_index,
                        scope.spawn(move || {
                            reader.read_reserved(job.block_id, job.selection, job.reservation)
                        }),
                    ));
                }
                // Join every worker before invoking user code, including after a panic.
                for (request_index, handle) in handles {
                    let outcome = handle
                        .join()
                        .map_or(WorkerOutcome::Panicked, WorkerOutcome::Read);
                    outcomes.push((request_index, outcome));
                }
            });
        }

        for (request_index, outcome) in outcomes {
            let read = match outcome {
                WorkerOutcome::Read(Ok(read)) => read,
                WorkerOutcome::Read(Err(source)) => {
                    return Err(ParallelReadError::Read {
                        request_index,
                        delivered,
                        source,
                    });
                }
                WorkerOutcome::Panicked => {
                    return Err(ParallelReadError::WorkerPanicked {
                        request_index,
                        delivered,
                    });
                }
            };
            if consume(request_index, read.selected()).is_break() {
                delivered += 1;
                drop(read);
                return Ok(ParallelReadProgress {
                    delivered,
                    status: ParallelReadStatus::Cancelled,
                });
            }
            delivered += 1;
            drop(read);
        }
    }

    Ok(ParallelReadProgress {
        delivered,
        status: ParallelReadStatus::Complete,
    })
}
