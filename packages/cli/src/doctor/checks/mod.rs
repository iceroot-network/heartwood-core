//! The checks, one file per area. Each function reads only its
//! [`crate::doctor::registry::CheckInput`] (the parsed view and the run's options) and gives its
//! findings. The rules: facts first, then fixed advice by key with typed values; `unknown` is never
//! a pass; a finding whose basis is the log alone never gives disruptive advice as a fail (another
//! source must agree).

pub mod clock;
pub mod config;
pub mod disk;
pub mod forger;
pub mod host;
pub mod net;
pub mod onboarding;
pub mod peers;
pub mod process;
pub mod quorum;
pub mod slots;
pub mod sync;
pub mod version;

use crate::doctor::seams::RoleState;
use crate::doctor::sources::logs::source_of;
use crate::doctor::view::{Reason, Unknown, View};
use crate::seams::Role;

/// `Err`, with `role`'s log as the source, when the process table shows `role` stopped or
/// errored: its log then says nothing about the time since, so a check that found nothing in it
/// cannot pass. A table that cannot be read leaves the judgement to the log.
fn running(view: &dyn View, role: Role) -> Result<(), Unknown> {
    match view.process(role).map(|facts| facts.state) {
        Ok(RoleState::Stopped | RoleState::Errored) => {
            Err(Unknown::new(source_of(role), Reason::NotRunning))
        }
        _ => Ok(()),
    }
}
