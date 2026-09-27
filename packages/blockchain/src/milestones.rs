//! The applied milestone entries: once the tip reaches an entry's height, the entry is applied,
//! and a start refuses a configuration in which an applied entry changed.
//!
//! An entry's digest is `tagged_hash(MilestoneEntry, stringify(entry))`, over the entry as the
//! file states it (not merged), with the kernel's sorted-key `stringify`, and without the keys
//! of network or node policy ([`EXCLUDED_KEYS`]): every other key is consensus scope. The commit
//! records an entry's digest when the tip first reaches its height, and a revert below the height
//! deletes it. At start, a stored entry whose digest differs or whose entry is gone, or an entry
//! at or below the tip with no stored digest, refuses the start.
//!
//! The scope of each key a milestone file may hold (the strict schema's keys):
//!
//! | Key | Scope | Why |
//! |---|---|---|
//! | `height` | consensus | where the entry applies |
//! | `activeDelegates`, `blockTime`, `epoch` | consensus | rounds, slots and the forging schedule |
//! | `block.version`, `block.maxTransactions`, `block.maxPayload` | consensus | block validity |
//! | `blocksToRevokeDelegateResignation` | consensus | the revoke rule of resignations |
//! | `burn.feeBasisPoints`, `burn.txAmount` | consensus | burned fees and the burn type |
//! | `dynamicFees.enabled`, `dynamicFees.minFee`, `dynamicFees.addonBytes` | consensus | the fee floor that applying a transaction checks |
//! | `transfer.minimum`, `transfer.maximum` | consensus | transfer validity |
//! | `reward`, `dynamicReward.*` | consensus | block rewards |
//! | `donations` | consensus | the donations taken from rewards |
//! | `legacyTransfer`, `legacyVote`, `bip340`, `acceptLegacySchnorrTransactions`, `htlcEnabled` | consensus | removed features, accepted only at their off values |
//! | `fees.staticFees` | pool policy (excluded) | the static fees of the pool's static mode, never a block rule |
//! | `p2p.minimumVersions` | network (excluded) | which peers a node talks to |

use heartwood_crypto::crypto::domain::{DomainTag, tagged_hash};
use heartwood_crypto::utils::js;
use heartwood_database::MilestoneEntryRow;
use heartwood_kernel::utils::stringify::stringify;
use serde_json::Value;

use crate::errors::StoreError;

/// The keys left out of an entry's digest, as dotted paths: `p2p` is network scope (the version
/// floor), and `fees.staticFees` is pool policy, which never decides a block's validity.
pub const EXCLUDED_KEYS: [&str; 2] = ["p2p", "fees.staticFees"];

/// The height and digest of every entry of the milestone file `milestones_text`, in file order.
/// A height is read as the milestone loader reads it: any JSON number whose value is an integer
/// that fits a `u32` (`2.0` is 2). An object on the path of an excluded key that is empty without
/// it is removed too (under the strict schema `fees` holds only `staticFees`), so adding or
/// removing an excluded key never changes a digest.
pub fn entry_digests(milestones_text: &str) -> Result<Vec<MilestoneEntryRow>, StoreError> {
    let refused = |detail: String| StoreError::Milestones(detail);
    let entries: Vec<Value> =
        serde_json::from_str(milestones_text).map_err(|e| refused(e.to_string()))?;
    entries
        .into_iter()
        .map(|mut entry| {
            let height = entry.get("height").and_then(js::safe_integer);
            let height = height.and_then(|height| u32::try_from(height).ok());
            let height = height.ok_or_else(|| refused("an entry has no height".into()))?;
            for path in EXCLUDED_KEYS {
                remove(&mut entry, path);
            }
            let text = stringify(&entry);
            let digest = tagged_hash(DomainTag::MilestoneEntry, &[text.as_bytes()]);
            Ok(MilestoneEntryRow { height, digest })
        })
        .collect()
}

/// Remove the key at the dotted `path` from `value`, and each object on the path that is then
/// empty.
fn remove(value: &mut Value, path: &str) {
    let Value::Object(map) = value else { return };
    match path.split_once('.') {
        None => {
            map.shift_remove(path);
        }
        Some((key, rest)) => {
            if let Some(child) = map.get_mut(key) {
                remove(child, rest);
                if child.as_object().is_some_and(serde_json::Map::is_empty) {
                    map.shift_remove(key);
                }
            }
        }
    }
}

/// The entries of `configured` whose height lies in `(from, to]`: those a tip that moves from
/// `from` to `to` reaches.
pub fn reached(configured: &[MilestoneEntryRow], from: u32, to: u32) -> Vec<MilestoneEntryRow> {
    let reached = configured
        .iter()
        .filter(|entry| from < entry.height && entry.height <= to);
    reached.copied().collect()
}

/// The check at start: every `stored` entry must be in `configured` with the same digest, and
/// every configured entry at or below `tip` must be stored. Returns the lowest height at which
/// the check fails, if any.
pub fn first_changed(
    stored: &[MilestoneEntryRow],
    configured: &[MilestoneEntryRow],
    tip: u32,
) -> Option<u32> {
    let changed = stored.iter().filter(|entry| !configured.contains(entry));
    let is_stored = |height: u32| stored.iter().any(|entry| entry.height == height);
    let missing = configured
        .iter()
        .filter(|e| e.height <= tip && !is_stored(e.height));
    changed.chain(missing).map(|entry| entry.height).min()
}
