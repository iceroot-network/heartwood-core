//! The clock: its synchronization, the relay's receipt times against the chain's slots, and the
//! clock's symptoms in the logs.

use crate::doctor::registry::{CheckInput, not_built};
use crate::doctor::report::Finding;

/// `clock.sync`.
pub fn sync(input: &CheckInput<'_>) -> Vec<Finding> {
    not_built(input)
}

/// `clock.vs-chain`.
pub fn vs_chain(input: &CheckInput<'_>) -> Vec<Finding> {
    not_built(input)
}

/// `clock.symptoms`.
pub fn symptoms(input: &CheckInput<'_>) -> Vec<Finding> {
    not_built(input)
}
