//! The transaction criteria of the served routes, as the reference's transaction filter reads
//! them, and the SQL functions they need on a reader.
//!
//! A [`TransactionFilter`] is the reference's list of or-criteria: every group must hold; a group
//! holds when one of its [`TransactionCriteria`] holds; a criteria holds when each key it gives
//! holds, and a key holds when one of its values does. On top, a criteria that names a `type`
//! without a `typeGroup` also requires the type group 1, as the reference's filter adds it.
//!
//! The keys keep the reference's semantics:
//! - `address` is the sender or a recipient, `senderId` the sender. The reference first asks its
//!   wallets whether they know the address, and matches no sender when they do not; but the
//!   sender of every stored transaction is a wallet the state knows, so that question never
//!   changes a match, and the filter compares the address with the stored sender alone.
//! - A recipient is the row's `recipient_id` or, for a transfer (1/6), an entry of the recipients
//!   side table, which holds exactly what the reference's containment query on the transfers of
//!   the asset finds.
//! - `id` is a prefix, as the reference's `LIKE 'id%'`.
//! - `memo` is a `LIKE` pattern on the memo's bytes, with `%`, `_` and `\` escapes,
//!   case-sensitive ([`like_bytes`]).
//! - `asset` is a JSON containment on the stored asset ([`contains`]), after the reference's
//!   expansion of the query's strings ([`asset_alternatives`]).
//!
//! [`js_number`] is JavaScript's `Number` of a string, which the expansion and the API's block
//! lookup both follow; [`jsonb_order_key`] sorts assets as PostgreSQL's `jsonb` does.

mod asset;
mod js;
mod jsonb;
mod like;

pub use self::asset::{CastLimit, asset_alternatives};
pub use self::js::js_number;
pub use self::jsonb::{contains, jsonb_order_key};
pub use self::like::{PatternError, like_bytes};

use serde_json::Value;

/// A value, or a range with either end open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Numeric {
    /// Equal to the value.
    Equal(u64),
    /// Between the ends, both included; an absent end is open.
    Range {
        /// The lowest value, included.
        from: Option<u64>,
        /// The highest value, included.
        to: Option<u64>,
    },
}

/// One criteria of the reference's transaction criteria: every key it gives must hold, and a key
/// holds when one of its values does. An empty list is a key the criteria does not give.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TransactionCriteria {
    /// `address`: the sender or a recipient.
    pub address: Vec<String>,
    /// `senderId`: the sender.
    pub sender_id: Vec<String>,
    /// `recipientId`: the row's recipient, or a transfer's recipient.
    pub recipient_id: Vec<String>,
    /// `id`: prefixes of the transaction id.
    pub id: Vec<String>,
    /// `version`.
    pub version: Vec<u64>,
    /// `blockHeight`.
    pub block_height: Vec<Numeric>,
    /// `blockId`.
    pub block_id: Vec<String>,
    /// `sequence`.
    pub sequence: Vec<Numeric>,
    /// `timestamp`.
    pub timestamp: Vec<Numeric>,
    /// `nonce`.
    pub nonce: Vec<Numeric>,
    /// `senderPublicKey`.
    pub sender_public_key: Vec<String>,
    /// `type`.
    pub kind: Vec<u64>,
    /// `typeGroup`.
    pub type_group: Vec<u64>,
    /// `memo`: `LIKE` patterns on the memo's bytes.
    pub memo: Vec<Vec<u8>>,
    /// `amount`.
    pub amount: Vec<Numeric>,
    /// `fee`.
    pub fee: Vec<Numeric>,
    /// `burnedFee`.
    pub burned_fee: Vec<Numeric>,
    /// `asset`: JSON values the stored asset must contain, one of them: every alternative of
    /// every value of the query's key ([`asset_alternatives`]).
    pub asset: Vec<Value>,
}

/// The reference's list of or-criteria: every group must hold, and a group holds when one of its
/// criteria does, so a group without criteria matches nothing. No group matches every
/// transaction.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TransactionFilter {
    /// The groups.
    pub groups: Vec<Vec<TransactionCriteria>>,
}

impl TransactionFilter {
    /// The filter of one criteria, as a route's query gives it.
    pub fn of(criteria: TransactionCriteria) -> TransactionFilter {
        TransactionFilter {
            groups: vec![vec![criteria]],
        }
    }
}
