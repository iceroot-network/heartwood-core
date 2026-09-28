//! Signals: SIGINT and SIGTERM start the role's graceful shutdown (a relay stops accepting
//! connections and closes its forger's sessions, stops the queue after the current job, flushes
//! the pool and writes the online record, checkpoints and closes the store; a forger stops
//! scheduling, lets a `postBlock` in flight finish and closes its relay sessions; then the logs
//! are flushed and the process exits 0); a second signal exits at once, with the status of a
//! process ended by that signal (128 plus its number).
//!
//! A role listens before it takes its process lock and writes its pid file ([`Signals::listen`]),
//! so a stop that finds the pid file always starts the graceful shutdown, however early it comes.

use tokio::runtime::Runtime;
use tokio::signal::unix::{Signal, SignalKind, signal};
use tokio::sync::mpsc;

/// The role's SIGINT and SIGTERM, caught from the moment they are listened to: a signal that
/// comes before [`Signals::requested`] is first polled is kept for it.
#[derive(Debug)]
pub struct Signals {
    role: &'static str,
    streams: Vec<(Signal, i32)>,
    /// The signals that cannot be handled, logged once the role's logger runs.
    failed: Vec<String>,
}

impl Signals {
    /// Catch SIGINT and SIGTERM for the process of `role` from now on. Called inside a Tokio
    /// runtime's context (in it, or under [`tokio::runtime::Runtime::enter`]), whose I/O driver
    /// is enabled.
    pub fn listen(role: &'static str) -> Signals {
        let (mut streams, mut failed) = (Vec::new(), Vec::new());
        for (kind, number) in [(SignalKind::interrupt(), 2), (SignalKind::terminate(), 15)] {
            match signal(kind) {
                Ok(stream) => streams.push((stream, number)),
                Err(error) => failed.push(format!("signal {number} cannot be handled: {error}")),
            }
        }
        Signals {
            role,
            streams,
            failed,
        }
    }

    /// [`Signals::listen`] in the context of `runtime`, which need not run yet.
    pub fn listen_in(runtime: &Runtime, role: &'static str) -> Signals {
        let _context = runtime.enter();
        Signals::listen(role)
    }

    /// Wait for the first SIGINT or SIGTERM; after it, a second one exits the process at once.
    pub async fn requested(self) {
        let (sender, mut received) = mpsc::unbounded_channel();
        for message in &self.failed {
            heartwood_kernel::error!("{message}");
        }
        for (mut stream, number) in self.streams {
            let sender = sender.clone();
            tokio::spawn(async move {
                while stream.recv().await.is_some() && sender.send(number).is_ok() {}
            });
        }
        drop(sender);
        let Some(number) = received.recv().await else {
            return std::future::pending().await;
        };
        heartwood_kernel::info!("Received signal {number}, stopping the {}", self.role);
        tokio::spawn(async move {
            if let Some(number) = received.recv().await {
                std::process::exit(128 + number);
            }
        });
    }
}

/// Wait for the first SIGINT or SIGTERM to the process of `role`; after it, a second one exits
/// the process at once. The handlers are in place when this is first polled.
pub async fn shutdown_requested(role: &'static str) {
    Signals::listen(role).requested().await;
}
