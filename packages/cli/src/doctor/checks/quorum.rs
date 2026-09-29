//! Attestation and quorum: the forger's refusals at our slots, the relay's attestation of its
//! forger, and (later) the relay's reach.

use crate::doctor::registry::{CheckInput, not_built};
use crate::doctor::report::Finding;

/// `quorum.forging-allowed`.
pub fn forging_allowed(input: &CheckInput<'_>) -> Vec<Finding> {
    not_built(input)
}

/// `quorum.attestation`.
pub fn attestation(input: &CheckInput<'_>) -> Vec<Finding> {
    not_built(input)
}

/// `quorum.reach`.
pub fn reach(input: &CheckInput<'_>) -> Vec<Finding> {
    not_built(input)
}
