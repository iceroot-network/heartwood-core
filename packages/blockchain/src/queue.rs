//! The serial job queue: the reference's block queue, which runs one job at a time on the node
//! thread, in push order.
//!
//! A pushed job waits while the queue is paused or stopped; `resume` starts it again. `clear`
//! drops the jobs not started; `stop` also waits for the running job to finish (committed or
//! rolled back). Each finished job's outcome goes to [`QueueEvents::finished`]; when the queue has
//! run every job and is idle, [`QueueEvents::drained`] runs: the node's re-add of the pool and the
//! save of the online record, then the machine's `PROCESSFINISHED` (the service's part).

use std::sync::Arc;

use crate::work::{Job, JobOutcome, NodeHandle};

/// Who hears of the queue's jobs.
pub trait QueueEvents: Send + Sync {
    /// A job finished with `outcome`.
    fn finished(&self, outcome: JobOutcome);

    /// The queue ran every job and is idle.
    fn drained(&self);
}

/// The serial job queue.
pub struct JobQueue {
    /// The node that runs the jobs.
    node: NodeHandle,
    /// Who hears of the jobs.
    events: Arc<dyn QueueEvents>,
    /// The runtime the queue's task runs on.
    runtime: tokio::runtime::Handle,
}

impl JobQueue {
    /// A running, empty queue whose jobs `node` runs, reporting to `events`, with its task on
    /// `runtime`.
    pub fn new(
        node: NodeHandle,
        events: Arc<dyn QueueEvents>,
        runtime: tokio::runtime::Handle,
    ) -> JobQueue {
        JobQueue {
            node,
            events,
            runtime,
        }
    }

    /// Push `job`; it runs after the jobs before it, once the queue is resumed.
    pub fn push(&self, job: Job) {
        let _ = (&self.node, &self.events, &self.runtime, job);
        todo!()
    }

    /// Hold the jobs not started; the running job finishes.
    pub fn pause(&self) {
        todo!()
    }

    /// Run the held jobs again.
    pub fn resume(&self) {
        todo!()
    }

    /// Drop the jobs not started.
    pub fn clear(&self) {
        todo!()
    }

    /// Hold every job, and wait for the running one to finish.
    pub async fn stop(&self) {
        todo!()
    }

    /// How many jobs wait.
    pub fn size(&self) -> usize {
        todo!()
    }

    /// Whether a job runs now.
    pub fn is_running(&self) -> bool {
        todo!()
    }

    /// Whether the queue is paused or stopped.
    pub fn is_paused(&self) -> bool {
        todo!()
    }
}
