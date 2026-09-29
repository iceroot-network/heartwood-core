//! The wallet search: `api.walletsLike` (the sorted list of a search) and `js.localeCompare` (the
//! numeric collation of two strings).

use super::{Env, Outcome, Run, failed};
use crate::subject::{Case, Verdict};

/// The operations of this module.
pub(super) const OPS: &[(&str, Run)] = &[
    ("api.walletsLike", wallets_like),
    ("js.localeCompare", locale_compare),
];

fn wallets_like(_env: &Env<'_>, _case: &Case<'_>) -> Outcome<Verdict> {
    Err(failed("api.walletsLike has no Rust implementation yet"))
}

fn locale_compare(_env: &Env<'_>, _case: &Case<'_>) -> Outcome<Verdict> {
    Err(failed("js.localeCompare has no Rust implementation yet"))
}
