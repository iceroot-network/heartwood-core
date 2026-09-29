//! The SQL records of the store's arithmetic, recorded by the reference's PostgreSQL: `sql.round2`
//! (`ROUND(numeric, 2)`), `sql.avgInt8` (`AVG(bigint)::int8`) and `sql.productivity` (the
//! productivity query over generated rows).

use super::{Env, Outcome, Run, failed};
use crate::subject::{Case, Verdict};

/// The operations of this module.
pub(super) const OPS: &[(&str, Run)] = &[
    ("sql.round2", round2),
    ("sql.avgInt8", avg_int8),
    ("sql.productivity", productivity),
];

fn round2(_env: &Env<'_>, _case: &Case<'_>) -> Outcome<Verdict> {
    Err(failed("sql.round2 has no Rust implementation yet"))
}

fn avg_int8(_env: &Env<'_>, _case: &Case<'_>) -> Outcome<Verdict> {
    Err(failed("sql.avgInt8 has no Rust implementation yet"))
}

fn productivity(_env: &Env<'_>, _case: &Case<'_>) -> Outcome<Verdict> {
    Err(failed("sql.productivity has no Rust implementation yet"))
}
