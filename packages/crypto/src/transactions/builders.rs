//! Building and signing new transactions.
//!
//! A builder starts from the type's asset, takes the common fields, and ends with a signed
//! [`Transaction`] made by [`Transaction::from_data`], so a built transaction passes the same rules
//! as a decoded one. Unset fields take the reference builders' defaults: the network byte of the
//! milestone, and the type's static fee.

#![expect(
    unused_variables,
    reason = "the bodies are not written yet; remove this once they are"
)]

use crate::crypto::sig::Aux;
use crate::errors::BuildError;
use crate::identities::{KeyPair, PublicKeyBytes};
use crate::managers::Params;
use crate::transactions::types::{Asset, Memo, ResignationType, Transaction, TransferItem, Votes};

/// A transaction under construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransactionBuilder {
    asset: Asset,
    network: Option<u8>,
    nonce: u64,
    fee: Option<u64>,
    memo: Option<Memo>,
}

impl TransactionBuilder {
    fn with_asset(asset: Asset) -> TransactionBuilder {
        TransactionBuilder {
            asset,
            network: None,
            nonce: 0,
            fee: None,
            memo: None,
        }
    }

    /// A transfer (1/6) with these items.
    pub fn transfer(transfers: Vec<TransferItem>) -> TransactionBuilder {
        TransactionBuilder::with_asset(Asset::Transfer { transfers })
    }

    /// A second-signature registration (1/1) of `public_key`.
    pub fn second_signature(public_key: PublicKeyBytes) -> TransactionBuilder {
        TransactionBuilder::with_asset(Asset::SecondSignature { public_key })
    }

    /// A delegate registration (1/2) of `username`.
    pub fn delegate_registration(username: &str) -> TransactionBuilder {
        TransactionBuilder::with_asset(Asset::DelegateRegistration {
            username: username.to_owned(),
        })
    }

    /// A delegate resignation (1/7).
    pub fn delegate_resignation(resignation_type: Option<ResignationType>) -> TransactionBuilder {
        TransactionBuilder::with_asset(Asset::DelegateResignation { resignation_type })
    }

    /// A burn (2/0) of `amount`.
    pub fn burn(amount: u64) -> TransactionBuilder {
        TransactionBuilder::with_asset(Asset::Burn { amount })
    }

    /// A vote (2/2) with these votes, in this order.
    pub fn vote(votes: Votes) -> TransactionBuilder {
        TransactionBuilder::with_asset(Asset::Vote { votes })
    }

    /// Set the network byte.
    pub fn network(mut self, network: u8) -> TransactionBuilder {
        self.network = Some(network);
        self
    }

    /// Set the nonce.
    pub fn nonce(mut self, nonce: u64) -> TransactionBuilder {
        self.nonce = nonce;
        self
    }

    /// Set the fee.
    pub fn fee(mut self, fee: u64) -> TransactionBuilder {
        self.fee = Some(fee);
        self
    }

    /// Set the memo.
    pub fn memo(mut self, memo: Memo) -> TransactionBuilder {
        self.memo = Some(memo);
        self
    }

    /// Sign with `keys` and `aux`, and make the transaction under the merged milestone `params`.
    pub fn build(
        self,
        keys: &KeyPair,
        aux: Aux,
        params: &Params,
    ) -> Result<Transaction, BuildError> {
        todo!()
    }

    /// Sign with `keys` and `aux`, add the second signature of `second_keys` with `second_aux`, and
    /// make the transaction under the merged milestone `params`.
    pub fn build_second_signed(
        self,
        keys: &KeyPair,
        aux: Aux,
        second_keys: &KeyPair,
        second_aux: Aux,
        params: &Params,
    ) -> Result<Transaction, BuildError> {
        todo!()
    }
}
