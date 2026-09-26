//! The component lifecycle.
//!
//! A node process is a fixed list of components, booted in rank order and disposed in reverse.
//! Each component crate implements [`Component`]; the binary matches on a closed list of them.
//! There is no discovery, no registration at run time and no per-block re-evaluation.

use std::future::Future;

use crate::errors::BootError;

/// A component of a node process.
pub trait Component {
    /// The package name that `app.json` lists for the component.
    const PACKAGE: &'static str;
    /// Whether a boot failure stops the process (otherwise it is logged and the process goes on).
    const REQUIRED: bool;

    /// Whether the component runs in this process, decided once at start.
    fn enabled(&self) -> bool {
        true
    }

    /// Start the component: open sockets, start tasks, load state.
    fn boot(&mut self) -> impl Future<Output = Result<(), BootError>> + Send;

    /// Stop the component, after the process has stopped accepting work.
    fn dispose(&mut self) -> impl Future<Output = ()> + Send;
}
