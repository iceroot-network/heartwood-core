//! Sync: the relay's sync state and its fork recoveries.

use crate::doctor::registry::{CheckInput, not_built};
use crate::doctor::report::Finding;

/// `sync.state`.
pub fn state(input: &CheckInput<'_>) -> Vec<Finding> {
    not_built(input)
}

/// `sync.forks`.
pub fn forks(input: &CheckInput<'_>) -> Vec<Finding> {
    not_built(input)
}
