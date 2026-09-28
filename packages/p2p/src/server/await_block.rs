//! AwaitBlock: a public route waits while the machine is in `newBlock` and the job queue runs, so
//! the answer sees the block being processed. The reference waits without limit; the wait here
//! ends after a bound, and the request is then served from the published chain.

use std::time::Duration;

use tokio::sync::watch;

use crate::node_services::QueueState;

/// Wait while `queue` shows the machine in `newBlock` with the queue running, at most `bound`;
/// whether the wait ended before the bound.
pub async fn await_block(queue: watch::Receiver<QueueState>, bound: Duration) -> bool {
    let _ = (queue, bound);
    todo!("the bounded wait")
}
