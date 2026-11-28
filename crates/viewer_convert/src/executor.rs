//! `ThreadedExecutor`: ready-queue scheduler (the only native one) for one virtual batch of root children.
//!
//! The batch has as many threads as root children (`min(W, k)`, as the old per-branch threads did) and
//! they take the ready node job with the lowest [`NodePath`]. Only jobs under this batch are
//! dispatched (no lookahead, R16). When a job finishes, the host deletes its `release`, queues its
//! children and keeps the output until a preorder cursor commits it into the branch-local builder
//! of its root child; `append_branch` then merges the branches in root-child order, so node and
//! block ids, budget checks and `TooManyBlocks` match the serial plan.
//!
//! Errors are ordered by `(NodePath, Phase)` and the lowest wins (A8, R25): once one is known, no
//! job above it is dispatched and running ones above it abort, while lower jobs and commits go on.
//! `Cancelled` loses to any other error.

use std::{
    collections::BTreeMap,
    io,
    sync::{
        Condvar, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::Instant,
};

use viewer_convert_core::{
    spatial::executor::{BatchPlanner, RangeRunner},
    spatial::{BranchOutput, OpenNodeEvent, SpatialBuilder},
};
use viewer_core::NodeId;

use crate::ConvertError;
use crate::pipeline::{
    clock::{Clock, SystemClock},
    node_job::{
        ChildOutput, JobContext, JobProgress, NodeJob, NodeJobOutput, NodePath, RangeJob,
        RangePartition, run_node_job, run_root_partition_range,
    },
    point_source::{PointRange, SourceFactory},
    spool_store::{PlanStore, SpoolId},
};

/// Plans batches on one thread per root child with a ready queue of node jobs.
pub(crate) struct ThreadedExecutor;

type Builder<'a, 'b, S> = &'a mut SpatialBuilder<'b, S, SystemClock, ThreadedExecutor>;

/// Error phase at one path; the batch budget is checked before any job is dispatched.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Phase {
    Job,
    Commit,
}

/// A finished job as the host receives it. The event fields are host diagnostics, not job I/O.
struct Finished {
    job: NodeJob,
    start_seq: Option<u64>,
    started: Instant,
    /// Job work only, so a node event does not count the wait for the cursor.
    elapsed_ns: u64,
    result: Result<NodeJobOutput, ConvertError>,
}

/// A successful job waiting for the cursor.
struct Done {
    job: NodeJob,
    start_seq: Option<u64>,
    started: Instant,
    elapsed_ns: u64,
    output: NodeJobOutput,
}

enum Message {
    Progress(JobProgress),
    Finished(Box<Finished>),
}

enum CursorEntry {
    /// The next node to commit, with its branch-local parent id.
    Node {
        path: NodePath,
        parent: Option<NodeId>,
    },
    /// Leaves the subtree of a committed node of branch `key`: closes its event.
    Exit {
        key: u16,
        open: Option<OpenNodeEvent<Instant>>,
    },
}

#[derive(Default)]
struct Outcome {
    first: Option<(NodePath, Phase, ConvertError)>,
    cancelled: Option<ConvertError>,
}

impl Outcome {
    fn record(&mut self, path: &NodePath, phase: Phase, error: ConvertError) {
        if matches!(error, ConvertError::Cancelled { .. }) {
            self.cancelled.get_or_insert(error);
        } else if self
            .first
            .as_ref()
            .is_none_or(|(first, first_phase, _)| (path, phase) < (first, *first_phase))
        {
            self.first = Some((path.clone(), phase, error));
        }
    }

    /// Whether work at `path` can still change the result.
    fn admits(&self, path: &NodePath) -> bool {
        self.cancelled.is_none() && self.first.as_ref().is_none_or(|(first, ..)| path < first)
    }

    fn into_error(self) -> Option<ConvertError> {
        self.first.map(|(.., error)| error).or(self.cancelled)
    }
}

#[derive(Default)]
struct QueueState {
    jobs: BTreeMap<NodePath, NodeJob>,
    closed: bool,
    /// Running jobs above this path abort at their next heartbeat.
    abort_above: Option<NodePath>,
    abort_all: bool,
}

#[derive(Default)]
struct ReadyJobs {
    state: Mutex<QueueState>,
    ready: Condvar,
}

impl ReadyJobs {
    fn lock(&self) -> MutexGuard<'_, QueueState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn push(&self, job: NodeJob) {
        self.lock().jobs.insert(job.path.clone(), job);
        self.ready.notify_one();
    }

    /// The ready job with the lowest path; `None` once the queue is closed.
    fn next(&self) -> Option<NodeJob> {
        let mut state = self.lock();
        loop {
            if state.closed {
                return None;
            }
            if let Some((_, job)) = state.jobs.pop_first() {
                return Some(job);
            }
            state = self
                .ready
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    fn aborts(&self, path: &NodePath) -> bool {
        let state = self.lock();
        state.abort_all || state.abort_above.as_ref().is_some_and(|limit| path > limit)
    }

    /// Drops the queued jobs `outcome` no longer admits and returns how many.
    fn restrict(&self, outcome: &Outcome) -> usize {
        let mut state = self.lock();
        let before = state.jobs.len();
        if outcome.cancelled.is_some() {
            state.abort_all = true;
            state.jobs.clear();
        } else if let Some((first, ..)) = &outcome.first {
            state.abort_above = Some(first.clone());
            drop(state.jobs.split_off(first));
        }
        before - state.jobs.len()
    }

    fn close(&self) {
        self.lock().closed = true;
        self.ready.notify_all();
    }
}

/// Closes the queue on every exit of its owner, unwinding included: a worker panic lets the
/// others stop waiting and the host's `recv` end; a host panic lets the parked workers exit so
/// `thread::scope` can join them and resume the panic.
struct CloseOnDrop<'a>(&'a ReadyJobs);

impl Drop for CloseOnDrop<'_> {
    fn drop(&mut self) {
        self.0.close();
    }
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "the sender is owned by the worker and dropped when it ends, which closes the channel"
)]
fn work<S: PlanStore>(
    ctx: &JobContext,
    store: &S,
    queue: &ReadyJobs,
    sender: mpsc::Sender<Message>,
) {
    let _close = CloseOnDrop(queue);
    while let Some(job) = queue.next() {
        let start_seq = store.next_event_seq();
        let started = SystemClock.now();
        let result = run_node_job(ctx, &job, store, &SystemClock, &mut |progress| {
            if queue.aborts(&job.path) {
                return Err(ConvertError::Cancelled {
                    points_processed: 0,
                });
            }
            sender
                .send(Message::Progress(progress))
                .map_err(|_| ConvertError::Cancelled {
                    points_processed: 0,
                })
        });
        let finished = Finished {
            job,
            start_seq,
            started,
            elapsed_ns: SystemClock.elapsed_ns(started),
            result,
        };
        if sender.send(Message::Finished(Box::new(finished))).is_err() {
            return;
        }
    }
}

/// Host-side state of one batch.
struct Batch<'q> {
    queue: &'q ReadyJobs,
    /// Jobs queued or running.
    pending: usize,
    outcome: Outcome,
    outputs: BTreeMap<NodePath, Done>,
    cursor: Vec<CursorEntry>,
    branches: BTreeMap<u16, BranchOutput>,
    memory_budget: u64,
}

impl Batch<'_> {
    fn enqueue(&mut self, job: NodeJob) {
        if self.outcome.admits(&job.path) {
            self.pending += 1;
            self.queue.push(job);
        }
    }
}

impl<S: PlanStore> BatchPlanner<S, SystemClock> for ThreadedExecutor {
    fn plan_batch(
        &self,
        builder: &mut SpatialBuilder<'_, S, SystemClock, Self>,
        tasks: Vec<ChildOutput>,
        memory_budget: u64,
    ) -> Result<Vec<(u16, BranchOutput)>, ConvertError> {
        let collect = builder.collects_diagnostics();
        let threads = tasks.len();
        let queue = ReadyJobs::default();
        let mut batch = Batch {
            queue: &queue,
            pending: 0,
            outcome: Outcome::default(),
            outputs: BTreeMap::new(),
            cursor: Vec::new(),
            branches: BTreeMap::new(),
            memory_budget,
        };
        for task in tasks.iter().rev() {
            batch.cursor.push(CursorEntry::Node {
                path: NodePath(vec![task.key]),
                parent: None,
            });
        }
        for task in tasks {
            batch.branches.insert(task.key, BranchOutput::new(collect));
            let job = builder.node_job(
                NodePath(vec![task.key]),
                1,
                task.bounds,
                task.spool,
                task.point_count,
                true,
            );
            batch.enqueue(job);
        }

        let ctx = builder.job_context();
        let store = builder.store();
        let (sender, receiver) = mpsc::channel();
        let mut channel_error = None;
        thread::scope(|scope| {
            // Close before spawning: a failed spawn must not leave earlier workers parked forever.
            let _close = CloseOnDrop(&queue);
            for _ in 0..threads {
                let (ctx, queue, sender) = (&ctx, &queue, sender.clone());
                scope.spawn(move || work(ctx, store, queue, sender));
            }
            drop(sender);
            while batch.pending > 0 {
                match receiver.recv() {
                    Ok(Message::Progress(progress)) => {
                        if batch.outcome.cancelled.is_none()
                            && let Err(error) = builder.emit_job_progress(progress)
                        {
                            batch
                                .outcome
                                .record(&NodePath(Vec::new()), Phase::Job, error);
                        }
                    }
                    Ok(Message::Finished(finished)) => {
                        batch.pending -= 1;
                        finish_ready_job(builder, &mut batch, *finished);
                        advance_cursor(builder, &mut batch);
                    }
                    Err(_) => {
                        channel_error = Some(ConvertError::Io(io::Error::other(
                            "spatial worker channel closed unexpectedly",
                        )));
                        break;
                    }
                }
                batch.pending -= batch.queue.restrict(&batch.outcome);
            }
        });

        if let Some(error) = batch.outcome.into_error().or(channel_error) {
            return Err(error);
        }
        debug_assert!(batch.cursor.is_empty() && batch.outputs.is_empty());
        Ok(batch.branches.into_iter().collect())
    }
}

fn finish_ready_job<S: PlanStore>(
    builder: Builder<'_, '_, S>,
    batch: &mut Batch<'_>,
    finished: Finished,
) {
    let path = finished.job.path.clone();
    let output = match finished.result {
        Ok(output) => output,
        // Only a job aborted by `restrict` is cancelled; a user cancel is already recorded.
        Err(ConvertError::Cancelled { .. }) => return,
        Err(error) => {
            batch.outcome.record(&path, Phase::Job, error);
            return;
        }
    };
    if let Err(error) = output
        .release
        .iter()
        .try_for_each(|spool| builder.store().delete(*spool))
    {
        batch.outcome.record(&path, Phase::Job, error);
        return;
    }
    if !batch.outcome.admits(&path) {
        // Its children stay in the store; the failed plan discards them.
        return;
    }
    for child in &output.children {
        let job = builder.node_job(
            path.child(child.key),
            finished.job.level + 1,
            child.bounds,
            child.spool,
            child.point_count,
            true,
        );
        batch.enqueue(job);
    }
    batch.outputs.insert(
        path,
        Done {
            job: finished.job,
            start_seq: finished.start_seq,
            started: finished.started,
            elapsed_ns: finished.elapsed_ns,
            output,
        },
    );
}

/// Commits outputs in preorder while the next expected one has arrived.
fn advance_cursor<S: PlanStore>(builder: Builder<'_, '_, S>, batch: &mut Batch<'_>) {
    let record_bytes = builder.layout().record_bytes() as u64;
    while batch.outcome.cancelled.is_none() {
        match batch.cursor.last() {
            None => return,
            Some(CursorEntry::Exit { .. }) => {
                let Some(CursorEntry::Exit { key, open }) = batch.cursor.pop() else {
                    unreachable!()
                };
                let branch = batch.branches.get_mut(&key).expect("one branch per key");
                builder.with_branch(branch, batch.memory_budget, |builder| {
                    builder.finish_node_event(open);
                });
            }
            Some(CursorEntry::Node { path, .. }) => {
                if !batch.outcome.admits(path) || !batch.outputs.contains_key(path) {
                    return;
                }
                let Some(CursorEntry::Node { path, parent }) = batch.cursor.pop() else {
                    unreachable!()
                };
                let done = batch.outputs.remove(&path).expect("checked above");
                let key = path.0[0];
                let branch = batch.branches.get_mut(&key).expect("one branch per key");
                let Done {
                    job,
                    start_seq,
                    started,
                    elapsed_ns,
                    output,
                } = done;
                let mut open = start_seq.map(|start_seq| {
                    OpenNodeEvent::new(
                        NodeId::new(branch.node_count()),
                        parent,
                        job.level,
                        job.point_count.saturating_mul(record_bytes),
                        start_seq,
                        started,
                    )
                });
                let committed = builder.with_branch(branch, batch.memory_budget, |builder| {
                    builder.commit_job(&job, output, parent, &mut open)
                });
                if let Some(open) = open.as_mut() {
                    open.event.elapsed_ns = elapsed_ns.max(1);
                }
                match committed {
                    Ok(children) => {
                        let id = NodeId::new(branch.node_count() - 1);
                        batch.cursor.push(CursorEntry::Exit { key, open });
                        batch
                            .cursor
                            .extend(children.iter().rev().map(|child| CursorEntry::Node {
                                path: path.child(child.key),
                                parent: Some(id),
                            }));
                    }
                    Err(error) => {
                        batch.outcome.record(&path, Phase::Commit, error);
                        return;
                    }
                }
            }
        }
    }
}

/// Progress or result of one root range thread.
enum RangeMessage {
    Progress(usize, JobProgress),
    Finished(usize, Box<Result<RangePartition, ConvertError>>),
}

impl<S: PlanStore, F: SourceFactory + Sync> RangeRunner<S, SystemClock, F> for ThreadedExecutor {
    /// One thread per range, each opening its own source (`las::Reader` is not `Send`).
    /// `abort_above` holds the lowest failed range, an open error included: ranges above it stop
    /// at their next progress report, the ones below finish. A cancellation reported by
    /// `progress` stops every range.
    fn run_ranges(
        &self,
        job: &RangeJob<'_>,
        factory: &F,
        ranges: Vec<(PointRange, SpoolId)>,
        store: &S,
        clock: &SystemClock,
        progress: &mut dyn FnMut(usize, JobProgress) -> Result<(), ConvertError>,
    ) -> Vec<Result<RangePartition, ConvertError>> {
        let count = ranges.len();
        let abort_above = AtomicUsize::new(usize::MAX);
        let abort_all = AtomicBool::new(false);
        let (sender, receiver) = mpsc::channel();
        let mut results = (0..count).map(|_| None).collect::<Vec<_>>();
        let mut host_error = None;
        thread::scope(|scope| {
            for (range, (points, first_segment)) in ranges.into_iter().enumerate() {
                let sender = sender.clone();
                let (abort_above, abort_all) = (&abort_above, &abort_all);
                scope.spawn(move || {
                    let cancelled = || ConvertError::Cancelled {
                        points_processed: 0,
                    };
                    let aborted = || {
                        abort_all.load(Ordering::Relaxed)
                            || range > abort_above.load(Ordering::Relaxed)
                    };
                    let result = if aborted() {
                        Err(cancelled())
                    } else {
                        factory.open(points).and_then(|mut source| {
                            run_root_partition_range(
                                job,
                                first_segment,
                                &mut source,
                                store,
                                clock,
                                &mut |update| {
                                    if aborted() {
                                        return Err(cancelled());
                                    }
                                    sender
                                        .send(RangeMessage::Progress(range, update))
                                        .map_err(|_| cancelled())
                                },
                            )
                        })
                    };
                    if matches!(&result, Err(error) if !matches!(error, ConvertError::Cancelled { .. }))
                    {
                        abort_above.fetch_min(range, Ordering::Relaxed);
                    }
                    let _ = sender.send(RangeMessage::Finished(range, Box::new(result)));
                });
            }
            drop(sender);
            while let Ok(message) = receiver.recv() {
                match message {
                    RangeMessage::Progress(range, update) => {
                        if let Err(error) = progress(range, update) {
                            abort_all.store(true, Ordering::Relaxed);
                            host_error.get_or_insert(error);
                        }
                    }
                    RangeMessage::Finished(range, result) => results[range] = Some(*result),
                }
            }
        });
        let mut results = results
            .into_iter()
            .map(|result| {
                result.unwrap_or(Err(ConvertError::Cancelled {
                    points_processed: 0,
                }))
            })
            .collect::<Vec<_>>();
        // The caller's own cancellation replaces the first placeholder one.
        if let Some(error) = host_error
            && let Some(slot) = results
                .iter_mut()
                .find(|result| matches!(result, Err(ConvertError::Cancelled { .. })))
        {
            *slot = Err(error);
        }
        results
    }
}
