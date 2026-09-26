//! What reverting needs that the reverted data does not hold.
//!
//! The reference reverts a vote to the sender's latest earlier vote, a resignation to the
//! sender's latest earlier resignation, and a forger's last block to its latest earlier block,
//! each looked up in its history. The undo records hold the same values, taken before the change.
//! They are equal because a block holds at most one transaction of each such type per sender (the
//! block processor's unique-type rule): the latest earlier value is the value just before the
//! reverted transaction or block.

use heartwood_crypto::Block;
use heartwood_crypto::BlockId;
use heartwood_crypto::identities::Address;
use heartwood_crypto::transactions::types::Votes;

use crate::round_state::RoundState;
use crate::stores::state::Totals;
use crate::wallets::Resignation;

/// What reverting one transaction needs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum TxUndo {
    /// Nothing: the transaction's data is enough.
    #[default]
    None,
    /// A vote: the sender's votes before it.
    Vote {
        /// The previous votes, in the order they were kept.
        previous: Votes,
    },
    /// A delegate resignation: the delegate's resignation and its height before it.
    Resignation {
        /// The previous resignation.
        previous: Option<Resignation>,
        /// The height of the previous resignation transaction.
        previous_height: Option<u32>,
    },
}

/// What reverting a forger's payout needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgerUndo {
    /// The forger's address.
    pub address: Address,
    /// The forger's last block before this one.
    pub last_block: Option<BlockId>,
}

/// What reverting a round start needs: the round state and every delegate's rank before it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoundUndo {
    /// The round state before the round started.
    pub previous: RoundState,
    /// Every registered delegate's rank before the round started.
    pub ranks: Vec<(Address, Option<u32>)>,
}

/// What reverting one block needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockUndo {
    /// The block itself.
    pub block: Block,
    /// One record per transaction, in block order.
    pub transactions: Vec<TxUndo>,
    /// The forger's payout.
    pub forger: ForgerUndo,
    /// The supply totals before the block.
    pub totals: Totals,
    /// The round state before the block, when the block ended a round.
    pub round: Option<RoundUndo>,
}
