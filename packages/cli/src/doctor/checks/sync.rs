//! The relay's sync state and its fork recoveries.

use heartwood_kernel::log::Level;

use crate::doctor::checks::process::{ints, relay};
use crate::doctor::registry::CheckInput;
use crate::doctor::report::Basis::{Chain, Log, Process};
use crate::doctor::report::Finding;
use crate::doctor::report::Status::{Fail, Pass, Warn};
use crate::doctor::seams::RoleState;
use crate::doctor::sources::logs::LineKind;
use crate::doctor::view::{Reason, Source, Unknown, View};
use crate::seams::Role;

/// One round of blocks: how far behind a relay fails.
pub(crate) const ROUND: i64 = 53;
/// Fork recoveries in the window from which the relay fails.
const FORKS_FAIL: u32 = 3;

/// Whether the relay logged "in sync" since its last start; unknown when its log cannot say: the
/// file level hides info lines, or the window holds neither the start nor an "in sync" line (a
/// relay that started before the window and has been in sync all along logs neither).
pub(crate) fn synced_once(view: &dyn View) -> Result<bool, Unknown> {
    let log = view.log(Role::Relay)?;
    log.shows(Level::Info)?;
    let events = &log.events;
    let start = events
        .iter()
        .rposition(|e| e.kind == LineKind::RelayStarted);
    let since = events.get(start.unwrap_or(0)..).unwrap_or_default();
    match (since.iter().any(|e| e.kind == LineKind::InSync), start) {
        (false, None) => Err(Unknown::new(Source::RelayLog, Reason::Missing)),
        (synced, _) => Ok(synced),
    }
}

/// `sync.state`.
pub fn state(input: &CheckInput<'_>) -> Vec<Finding> {
    relay(input, |view| {
        let (status, syncing) = (view.api_status()?, view.api_syncing()?);
        let (height, blocks) = (status.now, status.blocks_count.max(syncing.blocks).max(0));
        // Only a relay far behind needs the log to tell a first sync from a fall.
        let verdict = match (status.synced, blocks > ROUND, synced_once(view)) {
            (true, ..) => (Pass, "synced"),
            (_, _, Ok(false)) => (Warn, "first-sync"),
            (_, true, Ok(true)) => (Fail, "fell-behind"),
            (_, true, Err(cause)) => return Err(cause),
            _ => (Warn, "catching-up"),
        };
        Ok((Chain, verdict, ints!(height, blocks)))
    })
}

/// `sync.forks`.
pub fn forks(input: &CheckInput<'_>) -> Vec<Finding> {
    relay(input, |view| {
        let log = view.log(Role::Relay)?;
        log.shows(Level::Info)?;
        let (mut forks, mut failures, mut corrupt) = (0u32, 0u32, 0u32);
        for event in &log.events {
            match &event.kind {
                LineKind::ForkRecoveryStarted => forks += 1,
                LineKind::DownloadedBlockNotAccepted
                | LineKind::NoBlocksFromAnyPeer { .. }
                | LineKind::NoPeersToDownload
                | LineKind::DownloadFailed => failures += 1,
                LineKind::StateCorrupted | LineKind::StateMightBeCorrupted => corrupt += 1,
                _ => {}
            }
        }
        let window = view.window();
        let first = log
            .covered
            .whole
            .then_some(window.from_ms)
            .or(log.covered.first_ms);
        let minutes = window
            .to_ms
            .saturating_sub(first.unwrap_or(window.to_ms))
            .max(0)
            / 60_000;
        let state = view.process(Role::Relay).map(|p| p.state);
        let stopped = matches!(state, Ok(RoleState::Stopped | RoleState::Errored));
        // A corruption line alone is only logged; the process table must agree for a fail.
        let (basis, verdict) = match () {
            _ if corrupt > 0 && stopped => (Process, (Fail, "corrupted")),
            _ if forks >= FORKS_FAIL => (Log, (Fail, "many")),
            _ if corrupt > 0 => (Log, (Warn, "corruption-logged")),
            _ if forks > 0 || failures > 0 => (Log, (Warn, "some")),
            _ => (Log, (Pass, "none")),
        };
        Ok((basis, verdict, ints!(forks, failures, minutes)))
    })
}
