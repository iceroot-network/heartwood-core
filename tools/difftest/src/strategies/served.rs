//! P20, the served answers: random requests of the seven public routes within their schemas, over
//! prefixes of the corpora and the synthetic store of full blocks, with random peer repositories
//! and pools (`p2p.controller`); and the property's reach: each public route and the getBlocks cut
//! reached often enough.

use std::collections::BTreeMap;
use std::sync::Arc;

use proptest::strategy::BoxedStrategy;
use serde_json::Value;

use super::kernel::OpCase;
use super::transactions::TxContext;
use crate::report::{CaseResult, Outcome};
use crate::vectors::Expected;

/// The cases of P20.
pub fn served_cases(context: &Arc<TxContext>) -> BoxedStrategy<OpCase> {
    let _ = context;
    todo!("the served answers' cases")
}

/// Count in `seen` what the case with `input`, answered `expected` by the reference, reached: its
/// route, and the getBlocks cut.
pub fn outcomes(input: &Value, expected: &Expected, seen: &mut BTreeMap<String, usize>) {
    let _ = (input, expected, seen);
}

/// P20's own check: each public route reached `served_reach` times and the getBlocks cut
/// `served_reach / 10` times; nothing is checked when `served_reach` is 0.
pub fn reach_checks(
    file: &str,
    seen: &BTreeMap<String, usize>,
    served_reach: u32,
) -> Vec<CaseResult> {
    let _ = seen;
    if served_reach == 0 {
        return Vec::new();
    }
    vec![CaseResult {
        file: file.to_owned(),
        line: 0,
        op: "P20".into(),
        label: format!("P20 reached each public route {served_reach} times"),
        height: 0,
        outcome: Outcome::NoSubject,
    }]
}
