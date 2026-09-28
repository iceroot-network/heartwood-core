//! `heartwood-difftest`: the differential-test harness of Heartwood Core.
//!
//! It runs the reference implementation (the oracle, `tools/oracle/`) in its container or
//! locally, loads the golden vectors the oracle writes, and compares them with Heartwood's
//! implementations. This crate is a test tool: it is never a dependency of a production crate.
//!
//! - [`oracle`]: start `gen-vectors.js` (L0) and `oracle-server.js` (L1), in the container or locally.
//! - [`vectors`]: the JSONL vector files, their meta records and the SHA-256 manifest.
//! - [`subject`]: a Rust implementation of one oracle operation, shared by L0 and L1, and the
//!   registry.
//! - [`subjects`]: Heartwood's subjects, which answer the oracle's operations with Heartwood's
//!   crates.
//! - [`fixture`]: the wallet fixtures of the state operations, as the oracle reads them and as a
//!   Heartwood wallet repository.
//! - [`strategies`]: the `proptest` strategies of L1.
//! - [`compare`]: the structural diff, with byte offsets for hex values.
//! - [`report`]: outcomes and the text report.
//! - [`replay`]: a corpus of inputs (a fuzz corpus) through the oracle and Heartwood.
//! - [`corpus`]: the chains that L2 replays: import from devnet archives, the corpus directory,
//!   and revert plans; [`corpus_from_blocks`]: a corpus from a Heartwood relay's block files.
//! - [`digests`]: the state digest files of L2 replays, and their comparison.
//! - [`synthetic`]: blocks built on top of a corpus chain for the synthetic L2 cases.
//! - [`synthetic_pool`]: the synthetic pool cases on a corpus chain's tip.
//! - [`restart`]: restart plans, and restarts of the reference and of Heartwood during a replay.
//! - [`store`]: a corpus replayed through Heartwood's store, its store anchors, and the kill
//!   test.
//! - [`synthetic_processor`]: the synthetic processor and fork cases on a corpus chain.
//! - [`synthetic_forger`]: the synthetic cases of a node's own forger's blocks on a corpus chain.
//! - [`runtime_kill`]: the kill test through the full runtime of a relay.
//! - [`api`]: the route corpus, its projections and `difftest api compare`.
//! - [`live`]: `difftest live check`, the comparisons of a live run on a devnet.
//! - [`levels`]: L0 to L4; L0, L1 and L2 run, L3 and L4 plug in with the node's processes.
//! - [`genesis`]: the devnet chain-definition driver (`difftest genesis`).
//! - [`forger_kill`]: the forger's crash test (`difftest forger crash`).
//! - [`network_generate`]: `heartwood network generate` against the genesis vectors.

#![warn(missing_docs)]

pub mod api;
pub mod cli;
pub mod compare;
pub mod corpus;
pub mod corpus_from_blocks;
pub mod digests;
pub mod errors;
pub mod fixture;
pub mod forger_kill;
pub mod genesis;
pub mod levels;
pub mod live;
pub mod network_generate;
pub mod oracle;
pub mod replay;
pub mod report;
pub mod restart;
pub mod runtime_kill;
pub mod store;
pub mod strategies;
pub mod subject;
pub mod subjects;
pub mod synthetic;
pub mod synthetic_forger;
pub mod synthetic_pool;
pub mod synthetic_processor;
pub mod vectors;

pub use errors::{Error, Result};
