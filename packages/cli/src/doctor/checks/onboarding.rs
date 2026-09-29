//! The onboarding profile: whether the application's sync check would pass.

use crate::doctor::registry::{CheckInput, not_built};
use crate::doctor::report::Finding;

/// `onboarding.sync-proof`.
pub fn sync_proof(input: &CheckInput<'_>) -> Vec<Finding> {
    not_built(input)
}
