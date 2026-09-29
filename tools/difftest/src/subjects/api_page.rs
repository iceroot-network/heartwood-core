//! The pagination service: `api.searchPage` (the standard criteria and `getPage` over items) and
//! `js.rbTrace` (the comparisons of the red-black tree over a script of inserts and removes).

use super::{Env, Outcome, Run, failed};
use crate::subject::{Case, Verdict};

/// The operations of this module.
pub(super) const OPS: &[(&str, Run)] = &[("api.searchPage", search_page), ("js.rbTrace", rb_trace)];

fn search_page(_env: &Env<'_>, _case: &Case<'_>) -> Outcome<Verdict> {
    Err(failed("api.searchPage has no Rust implementation yet"))
}

fn rb_trace(_env: &Env<'_>, _case: &Case<'_>) -> Outcome<Verdict> {
    Err(failed("js.rbTrace has no Rust implementation yet"))
}
