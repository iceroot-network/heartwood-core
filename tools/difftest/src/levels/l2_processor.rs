//! L2 through the processor: a corpus replayed as a follower imports it, and the job's cases.
//!
//! Each block goes through the download path (a getBlocks-shaped listing and
//! `heartwood_p2p::to_block`), then through the enqueueing's cuts and the job, committed per job
//! to a new store, with a manual clock at each block's slot time. Every result must be accepted,
//! every line must equal the judge's (`ts-digests.jsonl`), the store anchors must hold, and the
//! reference-chunked and one-block modes must give equal lines. [`job_case`] answers the
//! `chain.job` records.

use crate::corpus::Corpus;
use crate::errors::Result;
use crate::oracle::OracleServer;
use crate::report::Report;
use crate::subject::{Case, SubjectError, Verdict};
use crate::subjects::Devnet;

/// How the replay cuts the chain into jobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chunks {
    /// As the reference's enqueueing cuts them (150 transactions, 100 blocks, milestone heights).
    Reference,
    /// One block per job.
    One,
}

/// The options of a processor replay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct L2ProcessorOptions {
    /// How the chain is cut into jobs; both modes when `None`.
    pub chunks: Option<Chunks>,
    /// Replay the chain cut at this height.
    pub to: Option<u32>,
    /// Also run the synthetic processor and fork cases (they need the oracle).
    pub synthetic: bool,
}

/// Replay `corpus` through the processor with `options`, and the synthetic cases with `oracle`.
pub fn run(
    corpus: &Corpus,
    options: &L2ProcessorOptions,
    oracle: Option<&mut OracleServer>,
) -> Result<Report> {
    let _ = (corpus, options, oracle);
    todo!()
}

/// `chain.job`: the job of a record's blocks on its session, and the enqueueing's cuts.
pub fn job_case(devnet: &Devnet, case: &Case<'_>) -> std::result::Result<Verdict, SubjectError> {
    let _ = (devnet, case);
    Err(SubjectError::NotImplemented)
}
