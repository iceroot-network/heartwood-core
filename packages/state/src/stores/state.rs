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
    /// The burned part of every fee since.
    pub burned_fees: u64,
    /// Every amount burned by burn transactions since.
    pub burned_amounts: u64,
}

impl Totals {
    /// The supply the totals imply: issued plus rewards, less burned fees and burned amounts.
    pub fn expected_supply(&self) -> i128 {
        todo!()
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

    /// The supply totals.
    pub fn totals(&self) -> &Totals {
        &self.totals
    }

    /// The supply totals, to change them.
    pub fn totals_mut(&mut self) -> &mut Totals {
        &mut self.totals
    }
}
