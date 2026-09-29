//! The node's side of its own forger's blocks: the two lateness checks of a block that came from
//! the node's forger.
//!
//! - At receipt ([`crate::Blockchain::handle_incoming_block`], before the future-slot check): a
//!   block in another slot than now, or with less than 2,000 ms left in its slot, is discarded:
//!   `Discarded block <h> because it was received too late`.
//! - At the job's end, when its last result allows a broadcast and the machine is in `newBlock`:
//!   a block in another slot than now, or with less than 2,000 ms left, logs `Discarded block <h>
//!   because it was processed too late` and is marked as forked with one block to roll back,
//!   which removes it through the fork recovery, as the reference removes it; otherwise it is
//!   broadcast.
//!
//! Both read the clock as the reference's slots do: whole seconds since the epoch, so the time
//! left is a multiple of 1,000 ms.

use std::sync::Arc;

use heartwood_crypto::crypto::slots;
use heartwood_crypto::{Block, Milestones};
use heartwood_kernel::info;

use crate::blockchain::{Blockchain, Queue, locale};
use crate::network::Network;
use crate::work::IncomingBlock;

/// The time a forger's block needs left in its slot, in milliseconds.
pub const MINIMUM_MS: i64 = 2_000;

/// Whether a forger's block with `timestamp` at `height` is too late at `now_ms` (milliseconds
/// since the Unix epoch): its slot is not the current one, or less than [`MINIMUM_MS`] are left
/// in the current slot.
pub fn too_late(timestamp: u32, height: u32, now_ms: i64, milestones: &Milestones) -> bool {
    let params = milestones.at(height);
    let block_time = params.block_time();
    let epoch = slots::epoch_unix_ms(params.epoch()).unwrap_or(0);
    let now = slots::epoch_time(now_ms, epoch);
    let current = slots::slot_number(now, block_time);
    let next = slots::slot_time(current.saturating_add(1), block_time);
    let left_ms = next.saturating_sub(now).saturating_mul(1_000);
    current != slots::slot_number(timestamp.into(), block_time) || left_ms < MINIMUM_MS
}

impl<N: Network, Q: Queue> Blockchain<N, Q> {
    /// Whether `block`, from the node's forger, came too late to be processed; logged when it
    /// did.
    pub(crate) fn received_too_late(&self, block: &IncomingBlock) -> bool {
        let timestamp = block.block.header.header.timestamp.unwrap_or(0);
        let (height, now) = (block.height(), self.parts.clock.now_ms());
        let late = too_late(timestamp, height, now, &self.parts.milestones);
        if late {
            let height = locale(height);
            info!("Discarded block {height} because it was received too late :exclamation:");
        }
        late
    }

    /// Whether `block`, from the node's forger, was processed too late to be broadcast; when it
    /// was, it is logged and marked as forked with one block to roll back.
    pub(crate) fn processed_too_late(self: &Arc<Self>, block: &Arc<Block>) -> bool {
        let (timestamp, height) = (block.header().timestamp, block.height());
        let now = self.parts.clock.now_ms();
        if !too_late(timestamp, height, now, &self.parts.milestones) {
            return false;
        }
        let shown = locale(height);
        info!("Discarded block {shown} because it was processed too late :exclamation:");
        self.fork_block(Arc::clone(block), Some(1));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use heartwood_crypto::Network as ChainNetwork;

    fn milestones() -> Milestones {
        let network = ChainNetwork {
            name: "devnet".into(),
            message_prefix: "devnet message:\n".into(),
            address_character: Some("D".into()),
            bip32_public: 70_617_039,
            bip32_private: 70_615_956,
            pub_key_hash: 30,
            nethash: "0".repeat(64),
            wif: 170,
            slip44: 1,
            token: "DEV".into(),
            symbol: "D".into(),
            explorer: String::new(),
        };
        let text = r#"[{"height": 1, "activeDelegates": 53, "blockTime": 8,
            "block": {"version": 0, "maxTransactions": 150, "maxPayload": 2097152},
            "epoch": "2026-01-01T00:00:00.000Z", "reward": 0,
            "burn": {"feeBasisPoints": 9000, "txAmount": 2000000}, "donations": {},
            "dynamicReward": {"enabled": false}, "transfer": {"maximum": 256, "minimum": 1}}]"#;
        Milestones::load(text, &network).unwrap()
    }

    #[test]
    fn a_block_needs_its_own_slot_and_two_whole_seconds_left() {
        let milestones = milestones();
        let epoch = slots::epoch_unix_ms("2026-01-01T00:00:00.000Z").unwrap();
        // Slot 10 runs from 80 s to 87.999 s after the epoch.
        let at = |ms: i64| epoch + ms;
        assert!(!too_late(80, 5, at(80_000), &milestones));
        // At 85.999 s the whole seconds read 85: 3,000 ms left.
        assert!(!too_late(80, 5, at(85_999), &milestones));
        // At 86 s, 2,000 ms left: still in time.
        assert!(!too_late(80, 5, at(86_000), &milestones));
        // At 86.001 s, 1,999 ms are left, read as 2,000: still in time. At 87 s, 1,000 ms left:
        // too late.
        assert!(!too_late(80, 5, at(86_001), &milestones));
        assert!(too_late(80, 5, at(87_000), &milestones));
        assert!(too_late(80, 5, at(87_999), &milestones));
        // Another slot: before, or after.
        assert!(too_late(80, 5, at(79_999), &milestones));
        assert!(too_late(80, 5, at(88_000), &milestones));
    }
}
