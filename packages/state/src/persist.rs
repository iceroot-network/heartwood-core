//! The state's records: what the store keeps of the state, so that a restarted node reads its
//! state back instead of replaying the chain.
//!
//! One record per wallet ([`WalletRecord`]), one for the rest of the tip's state
//! ([`StateRecord`]: the supply totals and the round), and one per block above the genesis block
//! ([`UndoRecord`]: what reverting it needs). Each is a `serde` type that refuses unknown fields;
//! the store writes them as JSON text. Amounts are decimal strings; keys, ids and hashes
//! lowercase hex; addresses Base58 text. Where a record overlaps the state digest's `#W` records,
//! it uses the same keys.
//!
//! [`crate::State::restore`] builds a state from the records and checks it before returning it:
//! the format, the last block against the record, every key and name held once, each delegate's
//! vote balance and voter count against its voters' shares, the supply, the forging list against
//! the round's shuffle, and one undo record per height from 2 to the tip, in order. A state that
//! fails is [`RestoreError`], and the node then replays the chain instead.
//!
//! [`STATE_FORMAT`] is the format of these records; a change of any record changes it.

use std::fmt::Display;
use std::str::FromStr;

use heartwood_crypto::identities::Address;
use heartwood_crypto::{Block, BlockId};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::errors::SupplyError;
use crate::round_state::RoundState;
use crate::stores::state::Totals;
use crate::undo::{BlockUndo, ForgerUndo, RoundUndo, TxUndo};
use crate::wallets::Resignation;

/// The format of the records.
pub const STATE_FORMAT: u32 = 1;

/// A wallet's record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WalletRecord {
    /// The address.
    pub a: String,
    /// The public key, once bound.
    pub pk: Option<String>,
    /// The balance.
    pub bal: String,
    /// The nonce.
    pub n: String,
    /// The second public key.
    pub spk: Option<String>,
    /// The delegate record.
    pub d: Option<DelegateRecord>,
    /// The votes: delegate names and basis points, in the order the wallet keeps them.
    pub v: Vec<(String, u16)>,
    /// The vote shares: delegate names and amounts, in the order the wallet keeps them.
    pub vs: Vec<(String, String)>,
}

/// A delegate's record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DelegateRecord {
    /// The name.
    pub u: String,
    /// The vote balance.
    pub vb: String,
    /// The number of voters.
    pub vc: u64,
    /// The rank at the start of the current round.
    pub rk: Option<u32>,
    /// The resignation: 0 temporary, 1 permanent.
    pub rs: Option<u8>,
    /// The height of the latest resignation transaction.
    pub ra: Option<u32>,
    /// Blocks forged.
    pub pb: u64,
    /// Fees received from forged blocks.
    pub ff: String,
    /// Rewards of forged blocks.
    pub fr: String,
    /// The burned part of the fees.
    pub bf: String,
    /// The donations taken from the rewards.
    pub dn: String,
    /// The id of the latest forged block.
    pub lb: Option<String>,
}

/// A delegate of a round, as the round's record holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoundDelegateRecord {
    /// The name.
    pub u: String,
    /// The public key.
    pub pk: String,
    /// The vote balance at the round start.
    pub vb: String,
}

/// The round state's record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoundRecord {
    /// The round.
    pub round: u64,
    /// Its first height.
    pub rh: u64,
    /// The round of the next height.
    pub nr: u64,
    /// The number of delegates of a round.
    pub md: u64,
    /// The round's delegates in rank order.
    pub d: Vec<RoundDelegateRecord>,
    /// The forging list: the public keys in forging order.
    pub f: Vec<String>,
    /// The names of the delegates that forged the round's blocks so far, in block order.
    pub b: Vec<String>,
}

/// The supply totals' record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TotalsRecord {
    /// What the genesis block issued.
    pub issued: String,
    /// Every block reward since.
    pub rewards: String,
    /// Every burned fee since, genesis fees included.
    pub burned_fees: String,
    /// Every burned amount since.
    pub burned_amounts: String,
}

/// What reverting one transaction needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "lowercase")]
pub enum TxUndoRecord {
    /// Nothing.
    None,
    /// A vote: the votes before it.
    Vote {
        /// The previous votes, in the order they were kept.
        p: Vec<(String, u16)>,
    },
    /// A resignation: the resignation and its height before it.
    Resignation {
        /// The previous resignation: 0 temporary, 1 permanent.
        p: Option<u8>,
        /// The height of the previous resignation transaction.
        ph: Option<u32>,
    },
}

/// What reverting a forger's payout needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForgerUndoRecord {
    /// The forger's address.
    pub a: String,
    /// The forger's last block before this one.
    pub lb: Option<String>,
}

/// What reverting a round start needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoundUndoRecord {
    /// The round state before the round started.
    pub p: RoundRecord,
    /// Every registered delegate's address and rank before the round started.
    pub rk: Vec<(String, Option<u32>)>,
}

/// A block's undo record. The block itself is in the store's block rows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UndoRecord {
    /// The block's height.
    pub h: u32,
    /// The block's id.
    pub id: String,
    /// One record per transaction, in block order.
    pub t: Vec<TxUndoRecord>,
    /// The forger's payout.
    pub f: ForgerUndoRecord,
    /// The supply totals before the block.
    pub tot: TotalsRecord,
    /// The round state before the block, when the block ended a round.
    pub r: Option<RoundUndoRecord>,
}

/// The record of the tip's state besides the wallets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateRecord {
    /// The records' format, [`STATE_FORMAT`].
    pub format: u32,
    /// The height of the tip.
    pub height: u32,
    /// The id of the tip.
    pub block_id: String,
    /// The supply totals.
    pub totals: TotalsRecord,
    /// The round state.
    pub round: RoundRecord,
}

/// The result of reading a record.
pub(crate) type Read<T> = Result<T, RestoreError>;

impl TotalsRecord {
    /// The record of `totals`.
    pub fn from_totals(totals: &Totals) -> TotalsRecord {
        TotalsRecord {
            issued: totals.issued.to_string(),
            rewards: totals.rewards.to_string(),
            burned_fees: totals.burned_fees.to_string(),
            burned_amounts: totals.burned_amounts.to_string(),
        }
    }

    /// The totals of the record.
    pub fn to_totals(&self) -> Read<Totals> {
        const W: &str = "totals";
        Ok(Totals {
            issued: amount(W, &self.issued)?,
            rewards: amount(W, &self.rewards)?,
            burned_fees: amount(W, &self.burned_fees)?,
            burned_amounts: amount(W, &self.burned_amounts)?,
        })
    }
}

impl UndoRecord {
    /// The record of `undo`.
    pub fn from_undo(undo: &BlockUndo) -> UndoRecord {
        let transactions = undo.transactions.iter().map(|undo| match undo {
            TxUndo::None => TxUndoRecord::None,
            TxUndo::Vote { previous } => TxUndoRecord::Vote {
                p: previous.clone().into_iter().collect(),
            },
            TxUndo::Resignation {
                previous,
                previous_height,
            } => TxUndoRecord::Resignation {
                p: previous.map(Resignation::number),
                ph: *previous_height,
            },
        });
        let (address, last_block) = (&undo.forger.address, &undo.forger.last_block);
        UndoRecord {
            h: undo.block.height(),
            id: undo.block.id().to_hex(),
            t: transactions.collect(),
            f: ForgerUndoRecord {
                a: address.to_base58(),
                lb: last_block.as_ref().map(BlockId::to_hex),
            },
            tot: TotalsRecord::from_totals(&undo.totals),
            r: undo.round.as_ref().map(|round| RoundUndoRecord {
                p: round.previous.to_record(),
                rk: (round.ranks.iter().map(|(a, rank)| (a.to_base58(), *rank))).collect(),
            }),
        }
    }

    /// The undo record of `block`, which the record belongs to, on the network with address byte
    /// `network`.
    pub fn into_undo(self, block: Block, network: u8) -> Read<BlockUndo> {
        const W: &str = "undo";
        let (id, count) = (block.id().to_hex(), block.transactions().len());
        if (self.h, &self.id, self.t.len()) != (block.height(), &id, count) {
            return Err(bad(W, format!("height {} is not its block's", self.h)));
        }
        let mut transactions = Vec::with_capacity(count);
        for undo in self.t {
            transactions.push(match undo {
                TxUndoRecord::None => TxUndo::None,
                TxUndoRecord::Vote { p } => TxUndo::Vote {
                    previous: p.into_iter().collect(),
                },
                TxUndoRecord::Resignation { p, ph } => TxUndo::Resignation {
                    previous: p.map(|number| resignation(W, number)).transpose()?,
                    previous_height: ph,
                },
            });
        }
        let round = self.r.map(|round| -> Read<RoundUndo> {
            let rank = |(a, rank): (String, _)| {
                Ok((read(W, Address::from_base58_for(&a, network))?, rank))
            };
            let ranks = round.rk.into_iter().map(rank);
            Ok(RoundUndo {
                previous: RoundState::from_record(round.p)?,
                ranks: ranks.collect::<Read<_>>()?,
            })
        });
        Ok(BlockUndo {
            transactions,
            forger: ForgerUndo {
                address: read(W, Address::from_base58_for(&self.f.a, network))?,
                last_block: read(W, self.f.lb.as_deref().map(BlockId::from_hex).transpose())?,
            },
            totals: self.tot.to_totals()?,
            round: round.transpose()?,
            block,
        })
    }
}

/// A record of `what` whose value does not read, for the reason `detail`.
pub(crate) fn bad(what: &'static str, detail: impl Display) -> RestoreError {
    let detail = detail.to_string();
    RestoreError::Record { what, detail }
}

/// The value of `result`, or its error as an error of the record of `what`.
pub(crate) fn read<T, E: Display>(what: &'static str, result: Result<T, E>) -> Read<T> {
    result.map_err(|error| bad(what, error))
}

/// The amount in the decimal `text`: digits only, no sign.
pub(crate) fn amount<T: FromStr>(what: &'static str, text: &str) -> Read<T> {
    let digits = !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    read(what, text.parse().ok().filter(|_| digits).ok_or(text))
}

/// The resignation with the reference's number `number`.
pub(crate) fn resignation(what: &'static str, number: u8) -> Read<Resignation> {
    let all = [Resignation::Temporary, Resignation::Permanent];
    let found = all.into_iter().find(|r| r.number() == number);
    read(what, found.ok_or(number))
}

/// A stored state cannot be restored; the node then replays the chain instead.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RestoreError {
    /// The records are of another format.
    #[error("the state records have format {0}; this node reads format {STATE_FORMAT}")]
    Format(u32),
    /// A record does not read: a value is out of its range, or a key, id, address or amount is
    /// malformed.
    #[error("the {what} record cannot be read: {detail}")]
    Record {
        /// Which record.
        what: &'static str,
        /// What is wrong.
        detail: String,
    },
    /// Two wallets hold the same public key.
    #[error("two wallets hold the public key {0}")]
    DuplicatePublicKey(String),
    /// Two wallets hold the same delegate name.
    #[error("two wallets hold the delegate name {0}")]
    DuplicateUsername(String),
    /// The state record is not at the last block.
    #[error("the state record is at height {record_height}, the last block at {block_height}")]
    LastBlock {
        /// The record's height.
        record_height: u32,
        /// The record's block id.
        record_id: String,
        /// The last block's height.
        block_height: u32,
        /// The last block's id.
        block_id: String,
    },
    /// A delegate's vote balance or voter count is not what its voters' shares add up to.
    #[error("the vote balance or voter count of {username} is not its voters' shares")]
    VoteBalance {
        /// The delegate's name.
        username: String,
    },
    /// The balances do not add up to the supply totals.
    #[error(transparent)]
    Supply(#[from] SupplyError),
    /// The forging list is not the shuffle of the round's delegates.
    #[error("the forging list is not the shuffle of the round's delegates")]
    Forging,
    /// The undo records do not cover every height from 2 to the tip, in order.
    #[error("the undo record of height {expected} is missing or out of order")]
    Journal {
        /// The height whose record was expected.
        expected: u32,
    },
}
