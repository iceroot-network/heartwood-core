//! The listings over the state: `api.delegatesPage` and `api.walletsPage` (a page of the delegate
//! and wallet listings over a snapshot fixture) and `api.delegateResource` (a delegate's resource).

use super::{Env, Outcome, Run, failed};
use crate::subject::{Case, Verdict};

/// The operations of this module.
pub(super) const OPS: &[(&str, Run)] = &[
    ("api.delegatesPage", delegates_page),
    ("api.walletsPage", wallets_page),
    ("api.delegateResource", delegate_resource),
];

fn delegates_page(_env: &Env<'_>, _case: &Case<'_>) -> Outcome<Verdict> {
    Err(failed("api.delegatesPage has no Rust implementation yet"))
}

fn wallets_page(_env: &Env<'_>, _case: &Case<'_>) -> Outcome<Verdict> {
    Err(failed("api.walletsPage has no Rust implementation yet"))
}

fn delegate_resource(_env: &Env<'_>, _case: &Case<'_>) -> Outcome<Verdict> {
    Err(failed(
        "api.delegateResource has no Rust implementation yet",
    ))
}
