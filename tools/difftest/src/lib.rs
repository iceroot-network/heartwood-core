//! `heartwood-difftest`: the differential-test harness of Heartwood Core.
//!
//! It runs the Solar Core TypeScript reference (the oracle, `tools/oracle/`) in its container or
//! locally, loads the golden vectors the oracle writes, and compares them with Heartwood's
//! implementations. This crate is a test tool: it is never a dependency of a production crate.
//!
//! - [`oracle`]: start `gen-vectors.js` (L0) and `oracle-server.js` (L1), in the container or locally.
//! - [`vectors`]: the JSONL vector files, their meta records and the SHA-256 manifest.
//! - [`subject`]: Heartwood's implementation of each oracle operation, shared by L0 and L1.
//! - [`compare`]: the structural diff, with byte offsets for hex values.
//! - [`report`]: outcomes and the text report.
//! - [`levels`]: L0 to L4; L0 runs today, the others plug in as the node's crates arrive.
//! - [`genesis`]: the devnet chain-definition driver (`difftest genesis`).
//!
//! No Heartwood subject exists yet, so an L0 run reports "0 compared".

pub mod cli;
pub mod compare;
pub mod errors;
pub mod genesis;
pub mod levels;
pub mod oracle;
pub mod report;
pub mod subject;
pub mod vectors;

pub use errors::{Error, Result};
