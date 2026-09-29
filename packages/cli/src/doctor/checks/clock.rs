//! The clock checks: the host's synchronization, the relay's clock against the chain's slots, and
//! the symptoms of a wrong clock in the logs.

use crate::doctor::registry::CheckInput;
use crate::doctor::report::{Basis, Fact, Finding, Status};

use crate::doctor::sources::logs::{LineKind, LogEvent};
use crate::doctor::sources::probes::LeapStatus;
use crate::doctor::view::{NodeFile, Reason, Source, Unknown};
use crate::seams::Role;
use heartwood_kernel::log::Level;

use super::net::{int, run};

/// The fewest blocks whose receipts give a median.
const MIN_RECEIPTS: usize = 10;

/// The host's clock is synchronized, and how far chrony says it is off.
pub fn sync(input: &CheckInput<'_>) -> Vec<Finding> {
    run(input, (None, Basis::System), || {
        let (ntp, chrony) = (input.view.ntp(), input.view.chrony());
        let marker = matches!(input.view.file(NodeFile::TimesyncMarker), Ok(Some(_)));
        let flags = ntp.ok();
        let lost = matches!(chrony, Ok(c) if c.leap == LeapStatus::NotSynchronised);
        let reported = flags.and_then(|n| n.synchronized);
        let synced = lost
            .then_some(false)
            .or(reported)
            .or(marker.then_some(true));
        let offset_ms = chrony.ok().map(|c| c.offset_us.unsigned_abs() / 1000);
        let values = offset_ms.map_or(Vec::new(), |ms| vec![("offset_ms", int(ms))]);
        let (status, key) = match (synced, offset_ms, flags.and_then(|n| n.ntp)) {
            (Some(false), _, _) => (Status::Fail, "not-synchronized"),
            (_, Some(ms), _) if ms >= 500 => (Status::Fail, "offset-large"),
            (_, Some(ms), _) if ms >= 100 => (Status::Warn, "offset"),
            (Some(true), _, Some(false)) => (Status::Warn, "ntp-off"),
            (_, Some(_), _) => (Status::Pass, "synchronized-offset"),
            (Some(true), None, _) => (Status::Pass, "synchronized"),
            (None, None, _) => {
                let blind = Unknown::new(Source::Probes, Reason::Unparsed);
                return Err(ntp.err().or(chrony.err()).unwrap_or(blind));
            }
        };
        Ok((status, key, values))
    })
}

/// The median of `values`, whose two middle values are averaged.
fn median(mut values: Vec<i64>) -> Option<i64> {
    values.sort_unstable();
    let low = *values.get(values.len().checked_sub(1)? / 2)?;
    Some(low.saturating_add(*values.get(values.len() / 2)?) / 2)
}

/// The median of the time each block reached the relay minus its slot's start. The median, not the
/// least: a peer can add a few false receipts, but not move the median of a window's blocks.
pub fn vs_chain(input: &CheckInput<'_>) -> Vec<Finding> {
    run(input, (Some(Role::Relay), Basis::Log), || {
        let log = input.view.log(Role::Relay)?;
        log.shows(Level::Info)?;
        let config = input.view.relay_config()?;
        let few = |count: usize| {
            let values = vec![("blocks", int(count)), ("needed", int(MIN_RECEIPTS))];
            (Status::Unknown, "too-few", values)
        };
        let received = |event: &LogEvent| match event.kind {
            LineKind::BlockReceived { height, .. } => Some((height, event.at_ms)),
            _ => None,
        };
        let receipts: Vec<(u32, i64)> = log.events.iter().filter_map(received).collect();
        let lowest = receipts.iter().map(|receipt| receipt.0).min();
        let Some(lowest) = lowest.filter(|_| receipts.len() >= MIN_RECEIPTS) else {
            return Ok(few(receipts.len()));
        };
        let window = input.view.store_window(&[], lowest)?;
        let offset = |&(height, at): &(u32, i64)| {
            let block = window.blocks.iter().find(|block| block.height == height)?;
            let slot_ms = i64::from(block.timestamp).saturating_mul(1000);
            Some(at.saturating_sub(config.chain.epoch_ms.saturating_add(slot_ms)))
        };
        let offsets: Vec<i64> = receipts.iter().filter_map(offset).collect();
        let count = offsets.len();
        let Some(median) = median(offsets).filter(|_| count >= MIN_RECEIPTS) else {
            return Ok(few(count));
        };
        let (status, key) = match median {
            ..=-500 => (Status::Fail, "early"),
            -499..=-100 => (Status::Warn, "early"),
            2000.. => (Status::Warn, "late"),
            _ => (Status::Pass, "in-step"),
        };
        let values = vec![
            ("median_ms", Fact::Millis(median)),
            ("early_ms", Fact::Millis(median.saturating_neg())),
            ("blocks", int(count)),
        ];
        Ok((status, key, values))
    })
}

/// One role's symptoms of a wrong clock.
fn symptoms_of(input: &CheckInput<'_>, role: Role) -> Vec<Finding> {
    run(input, (Some(role), Basis::Log), || {
        let relay = role == Role::Relay;
        let log = input.view.log(role)?;
        log.shows(if relay { Level::Info } else { Level::Warning })?;
        let block_time = input.view.forger_config().map(|f| f.chain.block_time_s);
        let block_time =
            block_time.or_else(|_| input.view.relay_config().map(|c| c.chain.block_time_s));
        let slot_ms = i64::from(block_time.unwrap_or(8)).saturating_mul(1000);
        let first_slot = |at: i64| {
            let started = |kind: &LineKind| {
                matches!(
                    kind,
                    LineKind::ForgerStarted | LineKind::ForgerManagerStarted
                )
            };
            let in_slot = |start: i64| (0..slot_ms).contains(&at.saturating_sub(start));
            log.events
                .iter()
                .any(|e| started(&e.kind) && in_slot(e.at_ms))
        };
        let (mut late, mut far) = (0_u32, 0_u32);
        for event in &log.events {
            match event.kind {
                LineKind::DiscardedFutureSlot { .. }
                | LineKind::DiscardedReceivedLate { .. }
                | LineKind::DiscardedProcessedLate { .. } => late += 1,
                LineKind::TooLittleTime { .. } | LineKind::AlreadyNextSlot { .. }
                    if !first_slot(event.at_ms) =>
                {
                    late += 1;
                }
                LineKind::WillNotSign { far_slot: true, .. }
                | LineKind::NotAttesting { far_slot: true } => far += 1,
                _ => {}
            }
        }
        let word = if relay { "discarded" } else { "late" };
        let (status, key, count) = match (far, late) {
            (1.., _) => (Status::Fail, "refused", far),
            (0, 3..) => (Status::Fail, word, late),
            (0, 1..) => (Status::Warn, word, late),
            (0, 0) => (Status::Pass, "clean", 0),
        };
        Ok((status, key, vec![("count", int(count))]))
    })
}

/// Blocks discarded for their slot, late forging attempts and refusals of a slot far from the clock.
pub fn symptoms(input: &CheckInput<'_>) -> Vec<Finding> {
    [Role::Relay, Role::Forger]
        .into_iter()
        .flat_map(|role| symptoms_of(input, role))
        .collect()
}
