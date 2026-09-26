//! The genesis block, the last block and the supply totals.

use heartwood_crypto::Block;

/// The totals the supply check reads. Donations move part of a reward to other wallets and do
/// not change the supply, so they are not here.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Totals {
    /// What the genesis block issued.
    pub issued: u64,
    /// Every block reward since, donations included.
    pub rewards: u64,
    /// The burned part of every fee since, and the whole fee of every genesis transaction, which
    /// no forger receives.
    pub burned_fees: u64,
    /// Every amount burned by burn transactions, those of the genesis block included.
    pub burned_amounts: u64,
}

impl Totals {
    /// The supply the totals imply: issued plus rewards, less burned fees and burned amounts.
    pub fn expected_supply(&self) -> i128 {
        // Four u64 values cannot leave the range of an i128.
        i128::from(self.issued)
            .saturating_add(i128::from(self.rewards))
            .saturating_sub(i128::from(self.burned_fees))
            .saturating_sub(i128::from(self.burned_amounts))
    }

    /// The totals after a block with this reward, burned fee and burned amount; `None` when one
    /// would pass `2^64 − 1`.
    pub fn with_block(&self, reward: u64, burned_fee: u64, burned_amount: u64) -> Option<Totals> {
        Some(Totals {
            issued: self.issued,
            rewards: self.rewards.checked_add(reward)?,
            burned_fees: self.burned_fees.checked_add(burned_fee)?,
            burned_amounts: self.burned_amounts.checked_add(burned_amount)?,
        })
    }
}

/// The genesis block, the last block and the supply totals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateStore {
    genesis_block: Block,
    last_block: Block,
    totals: Totals,
}

impl StateStore {
    /// The store right after `genesis_block`, which issued `issued`.
    pub fn new(genesis_block: Block, issued: u64) -> StateStore {
        StateStore {
            last_block: genesis_block.clone(),
            genesis_block,
            totals: Totals {
                issued,
                ..Totals::default()
            },
        }
    }

    /// The genesis block.
    pub fn genesis_block(&self) -> &Block {
        &self.genesis_block
    }

    /// The last applied block.
    pub fn last_block(&self) -> &Block {
        &self.last_block
    }

    /// Make `block` the last applied block.
    pub fn set_last_block(&mut self, block: Block) {
        self.last_block = block;
    }

    /// Make `block` the last applied block, returning the block it replaces.
    pub fn replace_last_block(&mut self, block: Block) -> Block {
        std::mem::replace(&mut self.last_block, block)
    }

    /// The supply totals.
    pub fn totals(&self) -> &Totals {
        &self.totals
    }

    /// The supply totals, to change them.
    pub fn totals_mut(&mut self) -> &mut Totals {
        &mut self.totals
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expected_supply_and_additions() {
        let totals = Totals {
            issued: u64::MAX,
            rewards: u64::MAX,
            burned_fees: 3,
            burned_amounts: 4,
        };
        assert_eq!(totals.expected_supply(), 2 * i128::from(u64::MAX) - 7);
        let small = Totals {
            issued: 10,
            ..Totals::default()
        };
        assert_eq!(
            small.with_block(5, 2, 1),
            Some(Totals {
                issued: 10,
                rewards: 5,
                burned_fees: 2,
                burned_amounts: 1,
            })
        );
        assert_eq!(
            small.with_block(5, 20, 1).map(|t| t.expected_supply()),
            Some(-6)
        );
        assert_eq!(totals.with_block(1, 0, 0), None);
        assert_eq!(totals.with_block(0, u64::MAX, 0), None);
        assert_eq!(totals.with_block(0, 0, u64::MAX), None);
    }
}
