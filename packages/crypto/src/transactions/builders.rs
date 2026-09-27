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
        self.build_with(keys, aux, params, Mode::NonStrict)
    }

    /// [`TransactionBuilder::build`] under the rule variant `mode`: [`Mode::Genesis`] builds a
    /// transaction of a genesis block, whose fee may be 0.
    pub fn build_with(
        self,
        keys: &KeyPair,
        aux: Aux,
        params: &Params,
        mode: Mode,
    ) -> Result<Transaction, BuildError> {
        let data = self.signed(keys, aux, None, params)?;
        Transaction::from_data_with(data, params, mode)
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
        Transaction::from_data_with(data, params, Mode::NonStrict)
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

/// The static fee of `kind` at the height of `params`, as the reference's `staticFee` reads it:
/// the milestone's `fees.staticFees` entry, else the type's default. The builders use it for a
/// transaction without a fee, and the pool's static-fee mode requires it exactly.
pub fn static_fee(kind: TransactionType, params: &Params) -> Result<u64, BuildError> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identities::Address;
    use crate::transactions::testing::{NETWORK, devnet, keys};

    const AUX: Aux = Aux::fixed([9; 32]);

    fn votes(entries: &[(&str, u16)]) -> Votes {
        entries
            .iter()
            .map(|&(name, basis_points)| (name.to_owned(), basis_points))
            .collect()
    }

    #[test]
    fn defaults_and_vote_order() {
        let params = devnet();
        let sender = keys("builder sender");
        let tx = TransactionBuilder::vote(votes(&[("b", 5000), ("a", 5000)]))
            .nonce(4)
            .build(&sender, AUX, &params)
            .unwrap();
        assert!(tx.is_verified());
        assert_eq!(tx.fee(), 9_000_000, "the milestone's static fee");
        assert_eq!(tx.data().network, NETWORK);
        assert_eq!(tx.data().nonce, 4);
        let Asset::Vote { votes } = &tx.data().asset else {
            panic!("a vote");
        };
        assert_eq!(votes.keys().collect::<Vec<_>>(), ["a", "b"]);
        let burn = TransactionBuilder::burn(1)
            .build(&sender, AUX, &params)
            .unwrap();
        assert_eq!(
            burn.fee(),
            0,
            "the type's default without a milestone entry"
        );
        let resignation = TransactionBuilder::delegate_resignation(None)
            .fee(1)
            .build(&sender, AUX, &params)
            .unwrap();
        assert!(resignation.is_verified());
    }

    #[test]
    fn refusals() {
        let params = devnet();
        let sender = keys("builder sender");
        let item = TransferItem {
            amount: 1,
            recipient: Address::from_public_key(sender.public_key(), NETWORK),
        };
        assert_eq!(
            TransactionBuilder::transfer(vec![item; 257]).build(&sender, AUX, &params),
            Err(BuildError::TooManyTransfers {
                count: 257,
                maximum: 256
            })
        );
        assert!(matches!(
            TransactionBuilder::transfer(Vec::new()).build(&sender, AUX, &params),
            Err(BuildError::Schema(_))
        ));
        assert!(matches!(
            TransactionBuilder::burn(1)
                .network(30)
                .build(&sender, AUX, &params),
            Err(BuildError::Schema(_))
        ));
        assert!(matches!(
            TransactionBuilder::vote(votes(&[("a", 5000), ("b", 4999)]))
                .build(&sender, AUX, &params),
            Err(BuildError::Schema(_))
        ));
    }

    #[test]
    fn genesis_transactions_may_have_no_fee() {
        let params = devnet();
        let delegate = keys("genesis delegate");
        let registration = || TransactionBuilder::delegate_registration("first").fee(0);
        assert!(matches!(
            registration().build(&delegate, AUX, &params),
            Err(BuildError::Schema(_))
        ));
        let tx = registration()
            .build_with(&delegate, AUX, &params, Mode::Genesis)
            .unwrap();
        assert!(tx.is_verified());
        assert_eq!(tx.fee(), 0);
    }

    #[test]
    fn second_signed() {
        let params = devnet();
        let sender = keys("builder sender");
        let second = keys("builder second");
        let tx = TransactionBuilder::burn(5)
            .build_second_signed(&sender, AUX, &second, AUX, &params)
            .unwrap();
        assert!(tx.is_verified());
        assert!(tx.verify_second_signature(second.public_key().as_bytes(), &params));
        assert!(!tx.verify_second_signature(sender.public_key().as_bytes(), &params));
    }
}
