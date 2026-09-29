//! The host: its resources against the planning estimates, its memory and swap, and the doctor's
//! own user.

use crate::doctor::registry::{CheckInput, not_built};
use crate::doctor::report::Finding;

/// `host.resources`.
pub fn resources(input: &CheckInput<'_>) -> Vec<Finding> {
    not_built(input)
}

/// `host.memory`.
pub fn memory(input: &CheckInput<'_>) -> Vec<Finding> {
    not_built(input)
}

/// `host.user`.
pub fn user(input: &CheckInput<'_>) -> Vec<Finding> {
    not_built(input)
}
