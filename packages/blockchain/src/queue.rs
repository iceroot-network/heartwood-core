//! The serial job queue: the reference's in-memory queue, which runs one job at a time on the node
//! thread, in push order.
//!
//! It keeps the reference's semantics. A push, or a resume, starts the processing unless it runs
//! already; it runs the jobs in order while the queue is not paused, reporting each outcome to
//! [`QueueEvents::finished`] before it takes the next job, so a job's clear of the queue removes
//! the jobs after it. When the processing ends with no job waiting (every job ran, or the queue
//! was cleared), the queue drains: the node saves the online record, then re-adds the pool
//! ([`crate::work::Work::Drain`]), and [`QueueEvents::drained`] runs once the save is done (the
//! service's part, the machine's `PROCESSFINISHED`). So a resume of an idle, empty queue drains at once, and a pause
//! leaves the waiting jobs without a drain. `stop` pauses, drops the waiting jobs, and waits for
//! the running one to finish (committed or rolled back).

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tokio::sync::{Notify, oneshot};

use crate::work::{Job, JobOutcome, NodeHandle, Work};

/// Who hears of the queue's jobs.
pub trait QueueEvents: Send + Sync {
    /// A job finished with `outcome`; the next job starts after this returns.
    fn finished(&self, outcome: JobOutcome);

    /// The queue ran every job, and the node's drain is done.
    fn drained(&self);
}

/// The serial job queue.
pub struct JobQueue {
    shared: Arc<Shared>,
    /// The node that runs the jobs.
    node: NodeHandle,
    /// Who hears of the jobs.
    events: Arc<dyn QueueEvents>,
    /// The runtime the processing runs on.
    runtime: tokio::runtime::Handle,
}

/// What the queue and its processing task share: the queue, and a notice for `stop`, given when
/// the processing stops running jobs.
#[derive(Default)]
struct Shared(Mutex<Queue>, Notify);

#[derive(Default)]
struct Queue {
    jobs: VecDeque<Job>,
    /// Whether no job may start (the reference's `started`, inverted).
    paused: bool,
    /// Whether the processing runs jobs (the reference's `running`).
    running: bool,
    /// Whether a processing task exists, until it has sent its drain.
    processing: bool,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Queue> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl JobQueue {
    /// A running, empty queue whose jobs `node` runs, reporting to `events`, with its task on
    /// `runtime`.
    pub fn new(
        node: NodeHandle,
        events: Arc<dyn QueueEvents>,
        runtime: tokio::runtime::Handle,
    ) -> JobQueue {
        let shared = Arc::default();
        JobQueue {
            shared,
            node,
            events,
            runtime,
        }
    }

    /// Push `job`; it runs after the jobs before it, once the queue is resumed.
    pub fn push(&self, job: Job) {
        self.shared.lock().jobs.push_back(job);
        self.process();
    }

    /// Hold the jobs not started; the running job finishes.
    pub fn pause(&self) {
        self.shared.lock().paused = true;
    }

    /// Run the held jobs again; an idle queue without jobs drains.
    pub fn resume(&self) {
        self.shared.lock().paused = false;
        self.process();
    }

    /// Drop the jobs not started.
    pub fn clear(&self) {
        self.shared.lock().jobs.clear();
    }

    /// Hold every job, drop the waiting ones, and wait for the running one to finish.
    pub async fn stop(&self) {
        let idle = {
            let mut queue = self.shared.lock();
            (queue.paused, queue.jobs) = (true, VecDeque::new());
            // Registered before the lock is released, so the task's notice cannot be missed.
            queue.running.then(|| self.shared.1.notified())
        };
        if let Some(idle) = idle {
            idle.await;
        }
    }

    /// How many jobs wait.
    pub fn size(&self) -> usize {
        self.shared.lock().jobs.len()
    }

    /// Whether a job runs now.
    pub fn is_running(&self) -> bool {
        self.shared.lock().running
    }

    /// Whether the queue is paused or stopped.
    pub fn is_paused(&self) -> bool {
        self.shared.lock().paused
    }

    /// Start the processing, as the reference's `processJobs` is entered: nothing while it runs
    /// or while the queue is paused.
    fn process(&self) {
        let mut queue = self.shared.lock();
        if !queue.processing && !queue.paused {
            (queue.processing, queue.running) = (true, !queue.jobs.is_empty());
            let shared = Arc::clone(&self.shared);
            let (node, events) = (self.node.clone(), Arc::clone(&self.events));
            self.runtime.spawn(run(shared, node, events));
        }
    }
}

/// The processing: the jobs in order while the queue is not paused, then the drain when none
/// waits. When the node has stopped, it ends without a drain.
async fn run(shared: Arc<Shared>, node: NodeHandle, events: Arc<dyn QueueEvents>) {
    let mut stopped = false;
    while let Some(job) = next(&shared).filter(|_| !stopped) {
        match node.job(job).await {
            Ok(outcome) => events.finished(outcome),
            Err(_) => stopped = true,
        }
    }
    let drained = {
        let mut queue = shared.lock();
        (queue.running, queue.processing) = (false, false);
        // Sent before another processing can start, so the node drains before its next jobs.
        let (answer, drained) = oneshot::channel();
        let drain = !stopped && queue.jobs.is_empty();
        let sent = drain && node.submit(Work::Drain(answer, false)).is_ok();
        sent.then_some(drained)
    };
    shared.1.notify_waiters();
    if let Some(drained) = drained
        && let Ok(Ok(())) = drained.await
    {
        events.drained();
    }
}

/// The next job, marked running, unless the queue is paused. (After the node has stopped, the
/// processing drops the job it takes.)
fn next(shared: &Shared) -> Option<Job> {
    let mut queue = shared.lock();
    let job = (!queue.paused).then(|| queue.jobs.pop_front()).flatten();
    queue.running |= job.is_some();
    job
}
