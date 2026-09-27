//! The crypto files: `config.load`.
//!
//! A case gives a milestone file as JSON text, which is loaded under the devnet network with the
//! merged milestones, rewards and donations as output (`subjects::blocks`), or all four files as
//! text, some possibly missing, which the kernel's loader checks with the strict schema.

use super::{Env, Outcome, Run};
use crate::subject::{Case, Verdict};

/// The operations of this module.
pub(super) const OPS: &[(&str, Run)] = &[("config.load", config_load)];

/// `config.load`.
fn config_load(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    super::blocks::config_load(env, case)
}
