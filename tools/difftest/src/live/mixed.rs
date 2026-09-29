//! The checks of a mixed run (`w0`, `w`, `p`, `s`: reference nodes beside Heartwood relays, and
//! `w0` their reference baseline): one chain with the judge's lines, the judge of node 1's chain,
//! the reference nodes' acceptance of every Heartwood relay they reach, the reference logs and the
//! Heartwood logs counted per fault window's class against the baseline, the propagation through a
//! Heartwood relay, the fork of run `p`, the relays' processes and listeners, and what is recorded
//! without a gate.

use super::{Cases, MixedRun};
use crate::errors::Result;

/// Every Heartwood relay's last `apply` or `reapply` line per height of the final chain is the
/// judge's.
pub(super) fn one_chain(run: &MixedRun<'_>, cases: &mut Cases) -> Result<()> {
    let _ = run;
    cases.not_built("mixed", "one chain with the judge's lines");
    Ok(())
}

/// Node 1's chain (its archive, or in run `s` its block files) judged, with every block accepted.
pub(super) fn judge(run: &MixedRun<'_>, cases: &mut Cases) -> Result<()> {
    let _ = run;
    cases.not_built("mixed", "the judge of node 1's chain");
    Ok(())
}

/// Every reference node that reaches a Heartwood relay accepted it and lists it outside the fault
/// windows of both, with its API port recorded.
pub(super) fn acceptance(run: &MixedRun<'_>, cases: &mut Cases) -> Result<()> {
    let _ = run;
    cases.not_built(
        "mixed",
        "the reference nodes' acceptance of the Heartwood relays",
    );
    Ok(())
}

/// No ban, reply refusal or validation error naming a Heartwood address in the reference logs,
/// and their transport lines per fault window's class no more than the baseline's.
pub(super) fn reference_logs(run: &MixedRun<'_>, cases: &mut Cases) -> Result<()> {
    let _ = run;
    cases.not_built("mixed", "the reference logs");
    Ok(())
}

/// No request-refusal ban of a reference address in the Heartwood logs, the socket-close bans per
/// class no more than the baseline's, and no violation recorded against a devnet address.
pub(super) fn heartwood_logs(run: &MixedRun<'_>, cases: &mut Cases) -> Result<()> {
    let _ = run;
    cases.not_built("mixed", "the Heartwood logs");
    Ok(())
}

/// Transactions and blocks through a Heartwood relay, both ways.
pub(super) fn propagation(run: &MixedRun<'_>, cases: &mut Cases) -> Result<()> {
    let _ = run;
    cases.not_built("mixed", "the propagation through a Heartwood relay");
    Ok(())
}

/// Run `p`: the minority's blocks through node 4 only, and both sides on the majority's chain
/// after the heal.
pub(super) fn fork(run: &MixedRun<'_>, cases: &mut Cases) -> Result<()> {
    let _ = run;
    cases.not_built("mixed", "the fork healed through a Heartwood relay");
    Ok(())
}

/// Every Heartwood relay listens on its node address only; run `s`'s forgers listen on nothing.
pub(super) fn processes(run: &MixedRun<'_>, cases: &mut Cases) -> Result<()> {
    let _ = run;
    cases.not_built("mixed", "the processes and their listeners");
    Ok(())
}

/// The records that are not gated: the sync and re-sync times, the block lags, the cleanse
/// infractions, the probe's timings and the recipe's workarounds.
pub(super) fn recorded(run: &MixedRun<'_>, cases: &mut Cases) -> Result<()> {
    let _ = run;
    cases.not_built("mixed", "the records");
    Ok(())
}
