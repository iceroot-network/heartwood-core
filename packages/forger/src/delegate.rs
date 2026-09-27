//! A delegate's key and the block it forges, as the reference's `Delegate.createBlock` builds it:
//! version 0; the delegate's key; the round's timestamp and reward; the previous block's id and
//! height + 1; the number of transactions; the sums of their top-level amounts (a transaction
//! without one counts 0) and of their fees; 32 bytes of payload per transaction; the SHA-256 of
//! the concatenated transaction ids. `heartwood_crypto::blocks::factory::make` signs the draft
//! (BIP340 with the given aux) and makes the block, as `BlockFactory.make` does.

use std::sync::Arc;

use heartwood_crypto::blocks::factory::{self, BlockDraft};
use heartwood_crypto::crypto::hash::sha256_concat;
use heartwood_crypto::{Address, Aux, Block, KeyPair, Milestones, PublicKeyBytes, Transaction};

use crate::errors::ForgeError;
use crate::seams::ForgeInputs;

/// A delegate of this forger: its key pair, public key and address.
#[derive(Debug, Clone)]
pub struct Delegate {
    keys: Arc<KeyPair>,
    public_key: PublicKeyBytes,
    address: Address,
}

impl Delegate {
    /// The delegate of `keys`, its address on the network with address byte `network`.
    pub fn new(keys: Arc<KeyPair>, network: u8) -> Delegate {
        let public_key = keys.public_key().to_compressed();
        let address = Address::from_public_key(keys.public_key(), network);
        Delegate {
            keys,
            public_key,
            address,
        }
    }

    /// The delegate's public key.
    pub fn public_key(&self) -> &PublicKeyBytes {
        &self.public_key
    }

    /// The delegate's address.
    pub fn address(&self) -> &Address {
        &self.address
    }

    /// The header draft of the delegate's block from `inputs`. A sum over the transactions beyond
    /// 64 bits is refused, where the reference's serialiser throws.
    pub fn draft(&self, inputs: &ForgeInputs<'_>) -> Result<BlockDraft, ForgeError> {
        let transactions = inputs.transactions;
        let too_many = || ForgeError::TooManyTransactions {
            count: transactions.len(),
        };
        let count = u32::try_from(transactions.len()).map_err(|_| too_many())?;
        let sum = |field: &'static str, value: fn(&Transaction) -> u64| {
            (transactions.iter())
                .try_fold(0u64, |total, t| total.checked_add(value(t)))
                .ok_or(ForgeError::TotalOverflow { field })
        };
        let height = inputs.previous_height;
        Ok(BlockDraft {
            version: 0,
            timestamp: inputs.timestamp,
            height: height.checked_add(1).ok_or(ForgeError::Height { height })?,
            previous_block: *inputs.previous_id.as_bytes(),
            number_of_transactions: count,
            total_amount: sum("totalAmount", Transaction::amount)?,
            total_fee: sum("totalFee", Transaction::fee)?,
            reward: inputs.reward,
            payload_length: count.checked_mul(32).ok_or_else(too_many)?,
            payload_hash: sha256_concat(transactions.iter().map(|t| t.id().as_bytes().as_slice())),
        })
    }

    /// The delegate's signed block from `inputs`, under the merged milestone at its height.
    pub fn forge(
        &self,
        inputs: &ForgeInputs<'_>,
        aux: Aux,
        milestones: &Milestones,
    ) -> Result<Block, ForgeError> {
        let (draft, transactions) = (self.draft(inputs)?, inputs.transactions.to_vec());
        let block = factory::make(draft, transactions, &self.keys, aux, milestones)?;
        Ok(block)
    }
}
