//! `heartwood-cli`: the commands of Heartwood Core, without the node.
//!
//! The one `heartwood` binary (`heartwood-core`) parses its command tree with clap and calls this
//! crate for everything that is not a node process:
//!
//! - [`argv`]: the colon aliases of the reference's commands (`relay:start` is `relay start`).
//! - [`instance`]: an instance (token, network and role), its five directories, and its name
//!   `<token>-<network>-<role>`; explicit flags always win over `config.json`.
//! - [`supervisor`]: `heartwood supervise`, the built-in supervisor with pm2's restart rules, its
//!   lock, state file and output capture.
//! - [`process`] and [`process_table`]: the role commands `start`, `stop`, `restart`, `status` and
//!   `log`, and the process table that `status` prints.
//! - [`network`]: the file-writing side of `network generate`, with the devnet profile built in.
//! - [`doctor`]: `heartwood doctor`, read-only diagnostics of an instance, and its MCP server.
//! - [`recovery`]: `pool clear` and `state clear`, the reference's markers, and the trust rule the
//!   relay applies to them at its start.
//! - `devnet` (feature `devnet`) and [`http`]: the local devnet library, mode B (one loopback
//!   address per node), and the loopback HTTP reads of a relay's API.
//! - [`seams`]: the instance, the traits between the parts (the process operations, the API reads
//!   and the chain generator) and the types they share; [`errors`]: the typed errors.
//!
//! The crate depends on the crypto crate and the kernel only, so it never links a node component.
//! Every child process is started with an explicit argv, never through a shell.
//!
//! Features: `devnet` (on by default) builds the devnet library; `test-child` builds the
//! supervisor tests' child binary, a test seam that a release build never has.

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

pub mod argv;
#[cfg(feature = "devnet")]
pub mod devnet;
pub mod doctor;
pub mod errors;
pub mod http;
pub mod instance;
pub mod network;
pub mod process;
pub mod process_table;
pub mod recovery;
pub mod seams;
pub mod supervisor;

pub use crate::errors::{DevnetError, HttpError, InstanceError, NetworkError, ProcessError};
pub use crate::seams::{
    ApiBlock, ApiNodeStatus, ApiPeer, ChainGenerator, GenerateOptions, Generated,
    GeneratedDelegate, Instance, NodeApi, ProcessOps, Profile, Role, StatusRow, SupervisedStatus,
};
