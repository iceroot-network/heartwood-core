//! The chain routes' documents: `api.spec` (the patched OpenAPI document), `api.transactionSchemas`
//! (the activated types' schemas), `api.feeStatistics` (the fee statistics of stored rows) and
//! `api.fuzzQuery` (a fuzz input of the query target, the route's choice and the raw query, through
//! the grammar, the route's validation and the pagination).

use super::{Env, Outcome, Run, failed};
use crate::subject::{Case, Verdict};

/// The operations of this module.
pub(super) const OPS: &[(&str, Run)] = &[
    ("api.spec", spec),
    ("api.transactionSchemas", transaction_schemas),
    ("api.feeStatistics", fee_statistics),
    ("api.fuzzQuery", fuzz_query),
];

fn spec(_env: &Env<'_>, _case: &Case<'_>) -> Outcome<Verdict> {
    Err(failed("api.spec has no Rust implementation yet"))
}

fn transaction_schemas(_env: &Env<'_>, _case: &Case<'_>) -> Outcome<Verdict> {
    Err(failed(
        "api.transactionSchemas has no Rust implementation yet",
    ))
}

fn fee_statistics(_env: &Env<'_>, _case: &Case<'_>) -> Outcome<Verdict> {
    Err(failed("api.feeStatistics has no Rust implementation yet"))
}

fn fuzz_query(_env: &Env<'_>, _case: &Case<'_>) -> Outcome<Verdict> {
    Err(failed("api.fuzzQuery has no Rust implementation yet"))
}
