//! The forger's link and failover: its relays, its connection, its link failures, one key on two
//! forgers, its signing record and its start errors.

use crate::doctor::registry::{CheckInput, not_built};
use crate::doctor::report::Finding;

/// `forger.hosts`.
pub fn hosts(input: &CheckInput<'_>) -> Vec<Finding> {
    not_built(input)
}

/// `forger.link`.
pub fn link(input: &CheckInput<'_>) -> Vec<Finding> {
    not_built(input)
}

/// `forger.link-history`.
pub fn link_history(input: &CheckInput<'_>) -> Vec<Finding> {
    not_built(input)
}

/// `forger.one-key-one-forger`.
pub fn one_key_one_forger(input: &CheckInput<'_>) -> Vec<Finding> {
    not_built(input)
}

/// `forger.signing-record`.
pub fn signing_record(input: &CheckInput<'_>) -> Vec<Finding> {
    not_built(input)
}

/// `forger.start-errors`.
pub fn start_errors(input: &CheckInput<'_>) -> Vec<Finding> {
    not_built(input)
}
