//! Signals: SIGINT and SIGTERM start the relay's graceful shutdown (stop accepting API
//! connections; stop the queue after the current job; flush the pool and write the online record;
//! checkpoint and close the store; flush the logs; exit 0); a second signal exits at once, with
//! the status of a process ended by that signal (128 plus its number).

use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::mpsc;

/// Wait for the first SIGINT or SIGTERM; after it, a second one exits the process at once. The
/// handlers are in place when this is first polled.
pub async fn shutdown_requested() {
    let (sender, mut received) = mpsc::unbounded_channel();
    for (kind, number) in [(SignalKind::interrupt(), 2), (SignalKind::terminate(), 15)] {
        let mut stream = match signal(kind) {
            Ok(stream) => stream,
            Err(error) => {
                heartwood_kernel::error!("signal {number} cannot be handled: {error}");
                continue;
            }
        };
        let sender = sender.clone();
        tokio::spawn(async move {
            while stream.recv().await.is_some() && sender.send(number).is_ok() {}
        });
    }
    drop(sender);
    let Some(number) = received.recv().await else {
        return std::future::pending().await;
    };
    heartwood_kernel::info!("Received signal {number}, stopping the relay");
    tokio::spawn(async move {
        if let Some(number) = received.recv().await {
            std::process::exit(128 + number);
        }
    });
}
