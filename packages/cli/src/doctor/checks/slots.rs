//! Slots: whether our delegates are active, and the slots they missed with their likely causes.

use crate::doctor::registry::{CheckInput, not_built};
use crate::doctor::report::Finding;

/// `slots.active`.
pub fn active(input: &CheckInput<'_>) -> Vec<Finding> {
    not_built(input)
}

/// `slots.missed`.
pub fn missed(input: &CheckInput<'_>) -> Vec<Finding> {
    not_built(input)
}
