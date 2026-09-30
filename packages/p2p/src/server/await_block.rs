//! AwaitBlock: a public route waits while the machine is in `newBlock` and the job queue runs, so
//! the answer sees the block being processed. The reference waits for the queue's drain without
//! limit; the wait here ends after a bound, and the request is then served from the published
//! chain.
//!
//! The wait reads the queue's state through a `watch`: a drain followed at once by a new job,
//! both between two readings, is seen as a queue that still runs, and the bound then ends the wait.
//! The node refreshes that state every 20 ms, so a drain is seen up to 20 ms after it happened.
//!
//! A socket that closes during the wait does not end it: the reference's server runs a request's
//! steps and its handler after its socket has closed, and drops the answer. The outcome says that
//! the socket closed first.

use std::time::Duration;

use tokio::sync::watch;
use tokio::time::Instant;

use super::seams::SocketClosed;
use crate::node_services::{AwaitEnd, QueueState};

/// How a wait ended, and how long it took.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AwaitOutcome {
    /// How the wait ended.
    pub end: AwaitEnd,
    /// The whole wait; zero when there was nothing to wait for.
    pub waited: Duration,
}

/// Wait while `queue` shows the machine in `newBlock` with the queue running, at most `bound`;
/// how the wait ended. The machine's state is read once, as the reference reads it before it
/// waits for the drain. When `closed` fires before the drain or the bound, the wait still runs to
/// one of them and ends [`AwaitEnd::SocketClosed`].
pub async fn await_block(
    mut queue: watch::Receiver<QueueState>,
    bound: Duration,
    mut closed: SocketClosed,
) -> AwaitOutcome {
    let started = Instant::now();
    let state = *queue.borrow_and_update();
    if !(state.new_block && state.running) {
        return AwaitOutcome {
            end: AwaitEnd::NoWait,
            waited: Duration::ZERO,
        };
    }
    let wait = tokio::time::timeout(bound, queue.wait_for(|state| !state.running));
    tokio::pin!(wait);
    let mut closed_first = false;
    let drained = loop {
        tokio::select! {
            biased;
            waited = &mut wait => break waited.is_ok(),
            () = closed.closed(), if !closed_first => closed_first = true,
        }
    };
    let end = match (closed_first, drained) {
        (true, _) => AwaitEnd::SocketClosed,
        (false, true) => AwaitEnd::Drained,
        (false, false) => AwaitEnd::Bound,
    };
    AwaitOutcome {
        end,
        waited: started.elapsed(),
    }
}
