//! A wallet and its delegate record.

use heartwood_crypto::BlockId;
use heartwood_crypto::identities::{Address, PublicKey, PublicKeyBytes};
use heartwood_crypto::transactions::types::Votes;
use heartwood_crypto::utils::sort_votes::sort_votes;
use indexmap::IndexMap;

use crate::errors::WalletError;

/// How a delegate has resigned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Resignation {
    /// For now: a resignation of type 2 revokes it.
    Temporary,
    /// For good.
    Permanent,
}

impl Resignation {
    /// The reference's number of the status: 0 temporary, 1 permanent.
    pub const fn number(self) -> u8 {
        match self {
            Resignation::Temporary => 0,
            Resignation::Permanent => 1,
        }
    }
}

/// The delegate record of a wallet: the reference's `delegate.*` attributes, and the height of the
/// latest resignation, which the reference looks up in its history instead.
///
/// The name is fixed at registration; it is what the repository's username index holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delegate {
    username: String,
    /// The sum of the voters' shares for this delegate.
    pub vote_balance: u64,
    /// The number of wallets that vote for this delegate.
    pub voters: u64,
    /// The rank at the start of the current round; `None` for a delegate that was resigned then,
    /// or registered since.
    pub rank: Option<u32>,
    /// The resignation, if the delegate has resigned.
    pub resigned: Option<Resignation>,
    /// The height of the delegate's latest resignation transaction (of any type).
    pub resigned_at: Option<u32>,
    /// Blocks forged.
    pub produced_blocks: u64,
    /// Fees received from forged blocks.
    pub forged_fees: u64,
    /// The burned part of those fees.
    pub burned_fees: u64,
    /// Rewards of forged blocks, donations included.
    pub forged_rewards: u64,
    /// The donations taken from those rewards.
    pub donations: u64,
    /// The id of the latest forged block.
    pub last_block: Option<BlockId>,
}

impl Delegate {
    /// A new delegate named `username`: every counter 0, no rank, not resigned.
    pub fn new(username: String) -> Delegate {
        Delegate {
            username,
            vote_balance: 0,
            voters: 0,
            rank: None,
            resigned: None,
            resigned_at: None,
            produced_blocks: 0,
            forged_fees: 0,
            burned_fees: 0,
            forged_rewards: 0,
            donations: 0,
            last_block: None,
        }
    }

    /// The delegate's name.
    pub fn username(&self) -> &str {
        &self.username
    }
}

/// A wallet: an address with its key, balance, nonce, second key, delegate record and votes.
///
/// The public key and the delegate record are indexed by the [`crate::WalletRepository`], so
/// they change only through it. The balance changes only through [`Wallet::credit`] and
/// [`Wallet::debit`], which never wrap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wallet {
    address: Address,
    public_key: Option<PublicKey>,
    balance: u64,
    nonce: u64,
    second_public_key: Option<PublicKeyBytes>,
    delegate: Option<Delegate>,
    votes: Votes,
    vote_shares: IndexMap<String, u64>,
}

impl Wallet {
    /// The wallet of `address` before anything happened to it: no key, balance and nonce 0.
    pub fn new(address: Address) -> Wallet {
        Wallet {
            address,
            public_key: None,
            balance: 0,
            nonce: 0,
            second_public_key: None,
            delegate: None,
            votes: Votes::new(),
            vote_shares: IndexMap::new(),
        }
    }

    /// The address.
    pub fn address(&self) -> &Address {
        &self.address
    }

    /// The public key, once a transaction of the wallet has bound it.
    pub fn public_key(&self) -> Option<&PublicKey> {
        self.public_key.as_ref()
    }

    pub(crate) fn set_public_key(&mut self, public_key: Option<PublicKey>) {
        self.public_key = public_key;
    }

    /// The balance.
    pub fn balance(&self) -> u64 {
        self.balance
    }

    /// Add `amount` to the balance.
    pub fn credit(&mut self, amount: u64) -> Result<(), WalletError> {
        self.balance = self
            .balance
            .checked_add(amount)
            .ok_or(WalletError::BalanceOverflow {
                address: self.address,
            })?;
        Ok(())
    }

    /// Take `amount` from the balance.
    pub fn debit(&mut self, amount: u64) -> Result<(), WalletError> {
        self.balance = self
            .balance
            .checked_sub(amount)
            .ok_or(WalletError::NegativeBalance {
                address: self.address,
            })?;
        Ok(())
    }

    /// The nonce: the number of transactions the wallet has sent.
    pub fn nonce(&self) -> u64 {
        self.nonce
    }

    /// Set the nonce.
    pub fn set_nonce(&mut self, nonce: u64) {
        self.nonce = nonce;
    }

    /// The registered second public key.
    pub fn second_public_key(&self) -> Option<&PublicKeyBytes> {
        self.second_public_key.as_ref()
    }

    /// Set or forget the second public key.
    pub fn set_second_public_key(&mut self, key: Option<PublicKeyBytes>) {
        self.second_public_key = key;
    }

    /// The delegate record, if the wallet is a delegate.
    pub fn delegate(&self) -> Option<&Delegate> {
        self.delegate.as_ref()
    }

    /// The delegate record, to change its values (its name stays).
    pub fn delegate_mut(&mut self) -> Option<&mut Delegate> {
        self.delegate.as_mut()
    }

    pub(crate) fn set_delegate(&mut self, delegate: Option<Delegate>) -> Option<Delegate> {
        std::mem::replace(&mut self.delegate, delegate)
    }

    /// Whether the wallet is a registered delegate.
    pub fn is_delegate(&self) -> bool {
        self.delegate.is_some()
    }

    /// The votes: delegate names and basis points, in the canonical order.
    pub fn votes(&self) -> &Votes {
        &self.votes
    }

    /// Set the votes; they are kept in the canonical order of
    /// [`heartwood_crypto::utils::sort_votes::sort_votes`], whatever the order of `votes`.
    pub fn set_votes(&mut self, votes: &Votes) {
        self.votes = sort_votes(votes);
    }

    /// Whether the wallet votes for any delegate.
    pub fn has_voted(&self) -> bool {
        !self.votes.is_empty()
    }

    /// The wallet's current share of its balance for each delegate it votes for, as last added to
    /// the delegates' vote balances (the reference's per-wallet vote balances).
    pub fn vote_shares(&self) -> &IndexMap<String, u64> {
        &self.vote_shares
    }

    /// Replace the wallet's shares. Only the vote balances' update calls it, which changes the
    /// delegates' vote balances by the same amounts.
    pub(crate) fn set_vote_shares(&mut self, shares: IndexMap<String, u64>) {
        self.vote_shares = shares;
    }
}
