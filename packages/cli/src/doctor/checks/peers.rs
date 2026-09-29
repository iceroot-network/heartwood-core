//! Peers: their count against `minimumNetworkReach`, our height against theirs, and how many
//! networks they come from.

use crate::doctor::registry::{CheckInput, not_built};
use crate::doctor::report::Finding;

/// `peers.count`.
pub fn count(input: &CheckInput<'_>) -> Vec<Finding> {
    not_built(input)
}

/// `peers.height`.
pub fn height(input: &CheckInput<'_>) -> Vec<Finding> {
    not_built(input)
}

/// `peers.diversity`.
pub fn diversity(input: &CheckInput<'_>) -> Vec<Finding> {
    not_built(input)
}
