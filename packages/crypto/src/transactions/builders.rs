//! Building and signing new transactions.
//!
//! A builder starts from the type's asset, takes the common fields, and ends with a signed
//! [`Transaction`] made as the reference's builders make it: the fields are signed, then passed to
//! [`Transaction::from_data_with`] under the rules without the signature requirement, so a built
//! transaction passes the same rules as a decoded one. Unset fields take the reference builders'
//! defaults: the network byte of the milestone, nonce 0, and the type's static fee from the
//! milestone's `fees.staticFees` (or the type's default when the milestone has none).

use crate::crypto::sig::Aux;
use crate::errors::{BuildError, SchemaError};
use crate::identities::{KeyPair, PublicKeyBytes};
use crate::managers::Params;
use crate::transactions::TransactionType;
use crate::transactions::codec::VERSION;
use crate::transactions::signer;
use crate::transactions::types::{
    Asset, Memo, ResignationType, Transaction, TransactionData, TransferItem, Votes,
};
use crate::utils::sort_votes::sort_votes;
use crate::validation::{Mode, bigint, transfer_limits};

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

    /// A vote (2/2) with these votes, which are put in canonical order ([`sort_votes`]) as the
    /// reference's builder does. No votes at all removes every vote.
    pub fn vote(votes: Votes) -> TransactionBuilder {
        TransactionBuilder::with_asset(Asset::Vote {
            votes: sort_votes(&votes),
        })
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
    ///
    /// A transfer with more items than the milestone's `transfer.maximum` is refused before it is
    /// signed. The result may be unverified: vote names that are not delegate names are signed and
    /// then removed by the rules, as in the reference.
    pub fn build(
        self,
        keys: &KeyPair,
        aux: Aux,
        params: &Params,
    ) -> Result<Transaction, BuildError> {
        let data = self.signed(keys, aux, None, params)?;
        Transaction::from_data_with(data, params, Mode::NON_STRICT)
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
        let data = self.signed(keys, aux, Some((second_keys, second_aux)), params)?;
        Transaction::from_data_with(data, params, Mode::NON_STRICT)
    }

    /// The signed fields, as the reference's builder holds them before `build`.
    fn signed(
        self,
        keys: &KeyPair,
        aux: Aux,
        second: Option<(&KeyPair, Aux)>,
        params: &Params,
    ) -> Result<TransactionData, BuildError> {
        let kind = self.asset.kind();
        if let Asset::Transfer { transfers } = &self.asset {
            let (_, maximum) = transfer_limits(params);
            if transfers.len() > maximum {
                return Err(BuildError::TooManyTransfers {
                    count: transfers.len(),
                    maximum,
                });
            }
        }
        let fee = match self.fee {
            Some(fee) => fee,
            None => static_fee(kind, params)?,
        };
        // The reference signs without the network field, so the milestone's byte is signed; a
        // network set on the builder is only checked by the rules afterwards.
        let mut data = TransactionData {
            version: VERSION,
            network: params.network(),
            kind,
            nonce: self.nonce,
            sender_public_key: keys.public_key().clone(),
            fee,
            memo: self.memo,
            asset: self.asset,
            signature: None,
            second_signature: None,
        };
        signer::sign(&mut data, keys, aux, params)?;
        if let Some((second_keys, second_aux)) = second {
            signer::second_sign(&mut data, second_keys, second_aux, params)?;
        }
        if let Some(network) = self.network {
            data.network = network;
        }
        Ok(data)
    }
}

/// The static fee of `kind` at the height of `params`: the milestone's `fees.staticFees` entry,
/// else the type's default.
fn static_fee(kind: TransactionType, params: &Params) -> Result<u64, BuildError> {
    let entry = params
        .as_json()
        .get("fees")
        .and_then(|fees| fees.get("staticFees"))
        .and_then(|fees| fees.get(kind.key()));
    match entry {
        None => Ok(kind.default_static_fee()),
        Some(value) => bigint(value)
            .and_then(|fee| u64::try_from(fee).ok())
            .ok_or_else(|| {
                SchemaError::invalid("fee", "the milestone's static fee is not an amount").into()
            }),
    }
}
