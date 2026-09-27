//! The applied milestone entries: once the tip reaches an entry's height, the entry is applied,
//! and a start refuses a configuration in which an applied entry changed.
//!
//! An entry's digest is `tagged_hash(MilestoneEntry, stringify(entry))`, over the entry as the
//! file states it (not merged), with the kernel's sorted-key `stringify`, and without the keys
//! of network or node policy ([`EXCLUDED_KEYS`]): every other key is consensus scope. The commit
//! records an entry's digest when the tip first reaches its height, and a revert below the height
//! deletes it. At start, a stored entry whose digest differs or whose entry is gone, or an entry
//! at or below the tip with no stored digest, refuses the start.

use heartwood_database::MilestoneEntryRow;

use crate::errors::StoreError;

/// The keys left out of an entry's digest, as dotted paths: `p2p` is network scope (the version
/// floor), and `fees.staticFees` is pool policy, which never decides a block's validity.
pub const EXCLUDED_KEYS: [&str; 2] = ["p2p", "fees.staticFees"];

/// The height and digest of every entry of the milestone file `milestones_text`, in file order.
pub fn entry_digests(milestones_text: &str) -> Result<Vec<MilestoneEntryRow>, StoreError> {
    let _ = milestones_text;
    todo!("the entries' digests")
}

/// The entries of `configured` whose height lies in `(from, to]`: those a tip that moves from
/// `from` to `to` reaches.
pub fn reached(configured: &[MilestoneEntryRow], from: u32, to: u32) -> Vec<MilestoneEntryRow> {
    let _ = (configured, from, to);
    todo!("the entries reached")
}

/// The check at start: every `stored` entry must be in `configured` with the same digest, and
/// every configured entry at or below `tip` must be stored. Returns the lowest height at which
/// the check fails, if any.
pub fn first_changed(
    stored: &[MilestoneEntryRow],
    configured: &[MilestoneEntryRow],
    tip: u32,
) -> Option<u32> {
    let _ = (stored, configured, tip);
    todo!("compare the applied entries")
}
