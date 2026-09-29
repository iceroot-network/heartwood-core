//! `heartwood doctor mcp`: a Model Context Protocol server over standard input and output for the
//! operator's own client, which starts it over SSH. Read-only: five tools (`list_checks`,
//! `run_checks`, `get_report`, `read_log_summary`, `explain_check`) that run the same checks and
//! read the same report, redacted log summaries and check documentation. No listener, no tool that
//! writes or runs anything, no key access; untrusted text only in fields named `untrusted`.
//!
//! The protocol revisions it serves are [`REVISIONS`]: the current revision is stateless (every
//! request carries its revision in `_meta`, the server answers `server/discover`, a request of
//! another revision gets error -32022 with the supported list), and the one before it opens with
//! the `initialize` handshake. One JSON-RPC message per line; nothing but protocol on standard
//! output; batches and messages over [`MAX_MESSAGE`] bytes refused; a notification never answered.

use std::io::{BufRead, Write};
use std::time::Duration;

use crate::doctor::DoctorScope;
use crate::doctor::seams::World;

/// The protocol revisions served, newest first: the stateless revision, then the last revision
/// with the `initialize` handshake.
pub const REVISIONS: [&str; 2] = ["2026-07-28", "2025-11-25"];
/// The longest message read; a longer one ends the session.
pub const MAX_MESSAGE: usize = 1024 * 1024;
/// How long a report is served from the cache before `run_checks` runs again.
pub const CACHE: Duration = Duration::from_secs(30);
/// The most `heartwood doctor mcp` servers one user runs at once.
pub const MAX_SERVERS: usize = 2;
/// The server's name in its answers.
pub const SERVER_NAME: &str = "heartwood-doctor";

/// Why the server stopped.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum McpError {
    /// The server is not built yet.
    #[error("the doctor's MCP server is not built in this release")]
    NotBuilt,
    /// Standard input or output failed.
    #[error("the session's input or output failed: {0:?}")]
    Io(std::io::ErrorKind),
    /// A message was longer than [`MAX_MESSAGE`].
    #[error("a message was longer than {MAX_MESSAGE} bytes")]
    TooLarge,
    /// [`MAX_SERVERS`] servers of this user already run.
    #[error("{MAX_SERVERS} doctor servers of this user already run")]
    TooManyServers,
}

/// Serve the session on `input` and `output` until end of input, over `world` for the instance
/// `scope` names.
pub fn serve(
    input: impl BufRead,
    output: impl Write,
    world: &dyn World,
    scope: &DoctorScope,
) -> Result<(), McpError> {
    let _ = (input, output, world, scope);
    Err(McpError::NotBuilt)
}
