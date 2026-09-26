//! Blocks built on top of a corpus chain for the synthetic L2 cases.
//!
//! The builder signs blocks and transactions with keys derived at run time from the corpus's
//! public devnet seed, and the same bytes go to the oracle's session and to Heartwood's state.

use heartwood_state::State;

use crate::corpus::Corpus;
use crate::errors::Result;

/// A synthetic case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SyntheticCase {
    /// A block whose second transaction overspends, after missed slots: refused, wallets
    /// unchanged.
    OverspendAfterGap,
    /// A rule that changes at the next height: accepted at one height and refused at the next.
    RuleHeight,
    /// A vote in the last block of a round, which changes the next round's ranks and schedule.
    VoteAtRoundEnd,
    /// Burns at the minimum amount and one below it.
    BurnMinimum,
    /// A delegate registered in a round, then a revert across the round's end.
    RegistrationAcrossRoundEnd,
    /// A block by a key that is not a delegate: refused.
    NonDelegateGenerator,
    /// A second-signature registration, then a transfer without the second signature.
    MissingSecondSignature,
    /// Fees at the dynamic fee floor and one below it, in a block.
    FeeFloorInBlock,
}

impl SyntheticCase {
    /// Every case.
    pub const ALL: [SyntheticCase; 8] = [
        SyntheticCase::OverspendAfterGap,
        SyntheticCase::RuleHeight,
        SyntheticCase::VoteAtRoundEnd,
        SyntheticCase::BurnMinimum,
        SyntheticCase::RegistrationAcrossRoundEnd,
        SyntheticCase::NonDelegateGenerator,
        SyntheticCase::MissingSecondSignature,
        SyntheticCase::FeeFloorInBlock,
    ];
}

/// The block bytes of `case` on top of `state`, the state after the corpus's last block.
pub fn build(case: SyntheticCase, corpus: &Corpus, state: &State) -> Result<Vec<Vec<u8>>> {
    let _ = (case, corpus, state);
    todo!()
}
