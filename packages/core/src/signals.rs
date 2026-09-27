//! Signals: SIGINT and SIGTERM start the relay's graceful shutdown (stop accepting API
//! connections; stop the queue after the current job; flush the pool and write the online record;
//! checkpoint and close the store; flush the logs; exit 0); a second signal exits at once.

/// Wait for the first SIGINT or SIGTERM; after it, a second one exits the process at once.
pub async fn shutdown_requested() {
    todo!()
}
