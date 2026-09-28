//! `chain.job`: a job of blocks on a session's chain, and the cuts of the blocks' enqueueing,
//! answered by the job's harness ([`crate::levels::l2_processor::job_case`]).

use super::{Env, Outcome, Run};
use crate::subject::{Case, Verdict};

/// The operations of the job.
pub(super) const OPS: &[(&str, Run)] = &[("chain.job", job)];

fn job(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    crate::levels::l2_processor::job_case(env.devnet()?, case)
}
