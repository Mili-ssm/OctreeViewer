//! Opt-in ready-queue scheduler for one virtual batch of root children
//! ([`SpatialScheduler::ReadyQueue`](crate::SpatialScheduler::ReadyQueue)).
//!
//! The batch has as many threads as root children (`min(W, k)`, as the recursive scheduler) and
//! they take the ready node job with the lowest [`NodePath`]. Only jobs under this batch are
//! dispatched (no lookahead, R16). When a job finishes, the host deletes its `release`, queues its
//! children and keeps the output until a preorder cursor commits it into the branch-local builder
//! of its root child; `append_branch` then merges the branches exactly as it merges the recursive
//! threads, so node and block ids, budget checks and `TooManyBlocks` are the same.
//!
//! Errors are ordered by `(NodePath, Phase)` and the lowest wins (A8, R25): once one is known, no
//! job above it is dispatched and running ones above it abort, while lower jobs and commits go on.
//! `Cancelled` loses to any other error, as with the recursive threads.

use std::{
    collections::BTreeMap,
    io,
    sync::{Condvar, Mutex, MutexGuard, PoisonError, mpsc},
    thread,
    time::Instant,
};

use viewer_core::NodeId;

use super::{BranchOutput, OpenNodeEvent, SpatialBuilder};
use crate::ConvertError;
use crate::diagnostics::{NodeSpoolEvent, SpatialPhaseDiagnostics, SpatialSpoolDiagnostics};
use crate::pipeline::{
    clock::{Clock, SystemClock},
    node_job::{
        ChildOutput, JobContext, JobProgress, NodeJob, NodeJobOutput, NodePath, run_node_job,
    },
    spool_store::PlanStore,
};

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
    result: Result<NodeJobOutput, ConvertError>,
}

/// A successful job waiting for the cursor.
struct Done {
    job: NodeJob,
    start_seq: Option<u64>,
    started: Instant,
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
        open: Option<OpenNodeEvent>,
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

/// Closes the queue if a worker panics, so the others stop waiting and the host's `recv` ends.
struct CloseOnPanic<'a>(&'a ReadyJobs);

impl Drop for CloseOnPanic<'_> {
    fn drop(&mut self) {
        if thread::panicking() {
            self.0.close();
        }
    }
}

fn work<S: PlanStore>(
    ctx: &JobContext,
    store: &S,
    queue: &ReadyJobs,
    sender: mpsc::Sender<Message>,
) {
    let _close = CloseOnPanic(queue);
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

impl<S: PlanStore> SpatialBuilder<'_, S> {
    /// Plans the subtrees of one batch of root children and returns their branches in key order,
    /// ready for `append_branch`.
    pub(super) fn plan_batch_ready_queue(
        &mut self,
        tasks: Vec<ChildOutput>,
        memory_budget: u64,
    ) -> Result<Vec<(u16, BranchOutput)>, ConvertError> {
        let collect = self.spool_diagnostics.is_some();
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
            batch.branches.insert(
                task.key,
                BranchOutput {
                    node_records: Vec::new(),
                    output_blocks: Vec::new(),
                    next_block_id: 0,
                    spool_diagnostics: collect.then(SpatialSpoolDiagnostics::default),
                    spatial_phases: collect.then(SpatialPhaseDiagnostics::default),
                    spool_written_local: 0,
                    node_events: Vec::new(),
                },
            );
            let job = self.node_job(
                NodePath(vec![task.key]),
                1,
                task.bounds,
                task.spool,
                task.point_count,
                true,
            );
            batch.enqueue(job);
        }

        let ctx = self.job_context();
        let store = self.store;
        let (sender, receiver) = mpsc::channel();
        let mut channel_error = None;
        thread::scope(|scope| {
            for _ in 0..threads {
                let (ctx, queue, sender) = (&ctx, &queue, sender.clone());
                scope.spawn(move || work(ctx, store, queue, sender));
            }
            drop(sender);
            while batch.pending > 0 {
                match receiver.recv() {
                    Ok(Message::Progress(progress)) => {
                        if batch.outcome.cancelled.is_none()
                            && let Err(error) = self.emit_job_progress(progress)
                        {
                            batch
                                .outcome
                                .record(&NodePath(Vec::new()), Phase::Job, error);
                        }
                    }
                    Ok(Message::Finished(finished)) => {
                        batch.pending -= 1;
                        self.finish_ready_job(&mut batch, *finished);
                        self.advance_cursor(&mut batch);
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
            batch.queue.close();
        });

        if let Some(error) = batch.outcome.into_error().or(channel_error) {
            return Err(error);
        }
        debug_assert!(batch.cursor.is_empty() && batch.outputs.is_empty());
        Ok(batch.branches.into_iter().collect())
    }

    fn finish_ready_job(&mut self, batch: &mut Batch<'_>, finished: Finished) {
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
            .try_for_each(|spool| self.store.delete(*spool))
        {
            batch.outcome.record(&path, Phase::Job, error);
            return;
        }
        if !batch.outcome.admits(&path) {
            // Its children stay in the store; the failed plan discards them.
            return;
        }
        for child in &output.children {
            let job = self.node_job(
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
                output,
            },
        );
    }

    /// Commits outputs in preorder while the next expected one has arrived.
    fn advance_cursor(&mut self, batch: &mut Batch<'_>) {
        let record_bytes = self.layout.record_bytes() as u64;
        while batch.outcome.cancelled.is_none() {
            match batch.cursor.last() {
                None => return,
                Some(CursorEntry::Exit { .. }) => {
                    let Some(CursorEntry::Exit { key, open }) = batch.cursor.pop() else {
                        unreachable!()
                    };
                    let branch = batch.branches.get_mut(&key).expect("one branch per key");
                    self.with_branch(branch, batch.memory_budget, |builder| {
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
                        output,
                    } = done;
                    let mut open = start_seq.map(|start_seq| OpenNodeEvent {
                        event: NodeSpoolEvent {
                            node_id: branch.node_records.len() as u64,
                            parent_id: parent.map(NodeId::get),
                            depth: job.level,
                            input_spool_bytes: job.point_count.saturating_mul(record_bytes),
                            start_seq,
                            ..NodeSpoolEvent::default()
                        },
                        started,
                    });
                    let committed = self.with_branch(branch, batch.memory_budget, |builder| {
                        builder.commit_job(&job, output, parent, &mut open)
                    });
                    match committed {
                        Ok(children) => {
                            let id = NodeId::new(branch.node_records.len() as u64 - 1);
                            batch.cursor.push(CursorEntry::Exit { key, open });
                            batch.cursor.extend(children.iter().rev().map(|child| {
                                CursorEntry::Node {
                                    path: path.child(child.key),
                                    parent: Some(id),
                                }
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
}
