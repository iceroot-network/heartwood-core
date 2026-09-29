//! Processes: whether each role this host runs is running, the `core` role, and the relay's start
//! errors.

use crate::doctor::registry::{CheckInput, not_built};
use crate::doctor::report::Finding;

/// `process.roles`.
pub fn roles(input: &CheckInput<'_>) -> Vec<Finding> {
    not_built(input)
}

/// `process.core-role`.
pub fn core_role(input: &CheckInput<'_>) -> Vec<Finding> {
    not_built(input)
}

/// `relay.start-errors`.
pub fn relay_start_errors(input: &CheckInput<'_>) -> Vec<Finding> {
    not_built(input)
}
