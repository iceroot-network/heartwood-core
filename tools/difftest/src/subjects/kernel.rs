//! The kernel and the logger: rounds, forging slots, block chaining, the status canonical JSON,
//! `.env` text, epoch timestamps, IP lists, node paths and log lines.

use super::{Env, Outcome};
use crate::subject::{Case, SubjectError, Verdict};

/// `round.calculate`: the round of a height.
pub(super) fn round_calculate(_env: &Env<'_>, _case: &Case<'_>) -> Outcome<Verdict> {
    Err(SubjectError::Unsupported("not implemented yet".into()))
}

/// `round.isNew`: whether a new round starts at a height.
pub(super) fn round_is_new(_env: &Env<'_>, _case: &Case<'_>) -> Outcome<Verdict> {
    Err(SubjectError::Unsupported("not implemented yet".into()))
}

/// `forging.info`: the forging slot of a time.
pub(super) fn forging_info(_env: &Env<'_>, _case: &Case<'_>) -> Outcome<Verdict> {
    Err(SubjectError::Unsupported("not implemented yet".into()))
}

/// `block.isChained`: whether a block chains onto the previous one.
pub(super) fn block_is_chained(_env: &Env<'_>, _case: &Case<'_>) -> Outcome<Verdict> {
    Err(SubjectError::Unsupported("not implemented yet".into()))
}

/// `kernel.stringify`: the status canonical JSON of a value.
pub(super) fn stringify(_env: &Env<'_>, _case: &Case<'_>) -> Outcome<Verdict> {
    Err(SubjectError::Unsupported("not implemented yet".into()))
}

/// `kernel.dotenv`: the pairs of `.env` text.
pub(super) fn dotenv(_env: &Env<'_>, _case: &Case<'_>) -> Outcome<Verdict> {
    Err(SubjectError::Unsupported("not implemented yet".into()))
}

/// `kernel.formatTimestamp`: the three forms of an epoch timestamp.
pub(super) fn format_timestamp(_env: &Env<'_>, _case: &Case<'_>) -> Outcome<Verdict> {
    Err(SubjectError::Unsupported("not implemented yet".into()))
}

/// `kernel.ip`: whether an address is whitelisted and blacklisted by a list.
pub(super) fn ip(_env: &Env<'_>, _case: &Case<'_>) -> Outcome<Verdict> {
    Err(SubjectError::Unsupported("not implemented yet".into()))
}

/// `kernel.paths`: the directories of a node.
pub(super) fn paths(_env: &Env<'_>, _case: &Case<'_>) -> Outcome<Verdict> {
    Err(SubjectError::Unsupported("not implemented yet".into()))
}

/// `logger.format`: a log line.
pub(super) fn logger_format(_env: &Env<'_>, _case: &Case<'_>) -> Outcome<Verdict> {
    Err(SubjectError::Unsupported("not implemented yet".into()))
}
