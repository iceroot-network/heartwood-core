//! Versions: the version floor, the installed and running versions, this build's features and the
//! peers' versions.

use crate::doctor::registry::{CheckInput, not_built};
use crate::doctor::report::Finding;

/// `version.floor`.
pub fn floor(input: &CheckInput<'_>) -> Vec<Finding> {
    not_built(input)
}

/// `version.installed`.
pub fn installed(input: &CheckInput<'_>) -> Vec<Finding> {
    not_built(input)
}

/// `version.build`.
pub fn build(input: &CheckInput<'_>) -> Vec<Finding> {
    not_built(input)
}

/// `version.peers`.
pub fn peers(input: &CheckInput<'_>) -> Vec<Finding> {
    not_built(input)
}
