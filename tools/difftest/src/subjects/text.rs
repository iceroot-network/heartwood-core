//! The reference's formatting in messages: `text.formatSatoshi` (amounts with the token symbol),
//! `text.pluralise` (counts), `text.toLocaleString` (integers) and `text.transaction` (a
//! transaction's `toString`).

use super::{Env, Outcome, Run};
use crate::subject::{Case, SubjectError, Verdict};

/// The operations of this module.
pub(super) const OPS: &[(&str, Run)] = &[
    ("text.formatSatoshi", not_yet),
    ("text.pluralise", not_yet),
    ("text.toLocaleString", not_yet),
    ("text.transaction", not_yet),
];

/// An operation without a Rust subject yet: its cases are skipped.
fn not_yet(_: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    Err(SubjectError::Unsupported(format!(
        "{} has no subject yet",
        case.op
    )))
}
