//! AwaitBlock: a public route waits while the machine is in `newBlock` and the job queue runs, so
//! the answer sees the block being processed. The reference waits for the queue's drain without
//! limit; the wait here ends after a bound, and the request is then served from the published
//! chain.
//!
//! The wait reads the queue's state through a `watch`: a drain followed at once by a new job,
//! both between two readings, is seen as a queue that still runs, and the bound then ends the wait.

use std::time::Duration;

use tokio::sync::watch;

use crate::node_services::QueueState;

/// Wait while `queue` shows the machine in `newBlock` with the queue running, at most `bound`;
/// whether the wait ended before the bound. The machine's state is read once, as the reference
/// reads it before it waits for the drain.
pub async fn await_block(mut queue: watch::Receiver<QueueState>, bound: Duration) -> bool {
    let state = *queue.borrow_and_update();
    if !(state.new_block && state.running) {
        return true;
    }
    tokio::time::timeout(bound, queue.wait_for(|state| !state.running))
        .await
        .is_ok()
}
