//! The transaction criteria of the served routes, as the reference's transaction filter reads
//! them, and the two SQL functions they need on a reader.
//!
//! A [`TransactionFilter`] is the reference's list of or-criteria: every group must hold; a group
//! holds when one of its [`TransactionCriteria`] holds; a criteria holds when each key it gives
//! holds, and a key holds when one of its values does. On top, a criteria that names a `type`
//! without a `typeGroup` also requires the type group 1, as the reference's filter adds it.
//!
//! The keys keep the reference's semantics: `address` is the sender or a recipient; `senderId`
//! matches only an address the state knows, which the caller resolves ([`TransactionCriteria::
//! sender_id`]); a recipient is the row's `recipient_id` or, for a transfer (1/6), an entry of the
//! recipients side table; `id` is a prefix; `memo` is a `LIKE` pattern on the memo's bytes, with
//! `%`, `_` and `\` escapes, case-sensitive; `asset` is a JSON containment on the stored asset,
//! with the reference's expansion of numeric strings into both a string and a number (at most
//! five of them).

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
    /// `address`: the sender (when the state knows the address) or a recipient.
    pub address: Vec<String>,
    /// `senderId`: addresses the state knows; the caller leaves out those it does not, and a
    /// criteria whose every `senderId` was left out matches nothing (`sender_id_unknown`).
    pub sender_id: Vec<String>,
    /// Whether the query gave `senderId` values that the state does not know: the key then
    /// matches no row, as the reference's `false` expression.
    pub sender_id_unknown: bool,
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
    /// `asset`: JSON objects the stored asset must contain.
    pub asset: Vec<Value>,
}

/// The reference's list of or-criteria: every group must hold, and a group holds when one of its
/// criteria does. No group matches every transaction.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TransactionFilter {
    /// The groups.
    pub groups: Vec<Vec<TransactionCriteria>>,
}

/// Whether `value` matches the `LIKE` pattern `pattern`, byte by byte: `%` matches any run of
/// bytes, `_` one byte, `\` escapes the next byte; case-sensitive, as PostgreSQL's `LIKE` on a
/// `bytea` column.
pub fn like_bytes(pattern: &[u8], value: &[u8]) -> bool {
    let _ = (pattern, value);
    todo!()
}

/// Whether the stored asset `asset` contains `criterion`, as PostgreSQL's `jsonb @>` decides it.
pub fn contains(asset: &Value, criterion: &Value) -> bool {
    let _ = (asset, criterion);
    todo!()
}
