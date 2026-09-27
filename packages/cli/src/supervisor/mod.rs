//! `heartwood supervise`: the built-in supervisor of one role of one instance, with pm2's
//! semantics.
//!
//! - It holds its lock (`<role>.supervisor.lock`) while it runs and writes its state file.
//! - It runs `heartwood <role> run` with an explicit argv and the caller's environment, its output
//!   captured to the `-out.log` and `-error.log` files ([`logs`]).
//! - A node that exits without a stop request after running 1 s or more is restarted at once and
//!   the unstable count reset; a run under 1 s is unstable, and after 5 unstable runs in a row the
//!   status is `errored` and the supervisor exits ([`restart`]).
//! - SIGTERM asks it to stop: it sends SIGINT to the node, SIGKILL after 30 s, and exits when the
//!   node has ([`signals`]). A pid is signalled only while its lock is held.

pub mod logs;
pub mod restart;
pub mod signals;
pub mod state;

use std::ffi::OsString;

use crate::seams::Instance;

/// Supervise `instance`'s node, run with `argv` (the program first), until a stop; never returns.
pub fn supervise(instance: Instance, argv: Vec<OsString>) -> ! {
    let _ = (instance, argv);
    todo!("the supervisor")
}
