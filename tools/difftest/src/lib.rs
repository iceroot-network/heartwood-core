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
//! - [`replay`]: a corpus of inputs (a fuzz corpus) through the oracle and Heartwood;
//!   [`replay_codec`]: the codec fuzz targets' corpora, as each target reads its input.
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
//! - [`lockstep`]: `difftest lockstep make`, chains forged by Heartwood while the reference
//!   applies every block and judges every transaction beside it.
//! - [`scenario`]: the scenario format, its compilation through the oracle and its runner with
//!   its backends (`difftest scenario compile|run|compare`).
//! - [`gate`]: the gate records, the cumulative ledger and the baseline record (`difftest gate`).
//! - [`genesis`]: the devnet chain-definition driver (`difftest genesis`).
//! - [`forger_kill`]: the forger's crash test (`difftest forger crash`).
//! - [`network_generate`]: `heartwood network generate` against the genesis vectors.
//! - [`p2p_compare`]: `difftest p2p compare`, the reference-client probe's records of two relays.
//! - [`store_summary`]: `difftest store summary`, a relay's store summarised.
//! - [`synthetic_rollback`] and [`synthetic_clear`]: the synthetic recovery cases on a corpus
//!   chain's tip.

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
pub mod gate;
pub mod genesis;
pub mod levels;
pub mod live;
pub mod lockstep;
pub mod network_generate;
pub mod oracle;
pub mod p2p_compare;
pub mod replay;
pub mod replay_codec;
pub mod report;
pub mod restart;
pub mod runtime_kill;
pub mod scenario;
pub mod service_rig;
pub mod store;
pub mod store_summary;
pub mod strategies;
pub mod subject;
pub mod subjects;
pub mod synthetic;
pub mod synthetic_clear;
pub mod synthetic_forger;
pub mod synthetic_pool;
pub mod synthetic_processor;
pub mod synthetic_rollback;
pub mod vectors;

pub use errors::{Error, Result};
