//! The protobuf messages of the public routes, with field presence.
//!
//! The messages are those of the reference's four `.proto` files (`shared`, `peer`, `blocks` and
//! `transactions`), declared field for field. A singular field is an `Option`: `None` is absent on
//! the wire, which the reference treats differently from a present zero, `false` or empty text,
//! because an absent field reads as its prototype default and is not an own key. Repeated fields
//! and the map are always present, possibly empty, as the reference's constructor creates them.
//!
//! [`Proto::decode`] reads bytes as the reference's protobuf library does, malformed bytes
//! included, and records the order in which fields first appeared ([`WireOrder`]), which is the
//! order of the decoded object's own keys ([`Proto::own_keys`]). [`Proto::encode`] writes the
//! present fields in field-number order.

pub mod blocks;
pub mod peer;
pub mod shared;
pub mod transactions;

use crate::errors::DecodeError;

/// A protobuf message as the reference's library encodes and decodes it.
pub trait Proto: Sized {
    /// The fields, in declaration order.
    const FIELDS: &'static [Field];

    /// The message's bytes: every present field, in field-number order.
    fn encode(&self) -> Vec<u8>;

    /// The message in `bytes`, read as the reference reads it.
    fn decode(bytes: &[u8]) -> Result<Self, DecodeError>;

    /// The own keys of the object the reference's decoder builds for this value, in enumeration
    /// order: the repeated fields and the map, in declaration order, then the present singular
    /// fields in [`WireOrder`] (and those absent from it in declaration order).
    fn own_keys(&self) -> Vec<&'static str>;
}

/// A field of a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Field {
    /// The field number.
    pub number: u32,
    /// The reference's (JavaScript) name of the field: `lastBlockHeight`.
    pub name: &'static str,
    /// Whether the field is repeated or the map, which every decoded object has as an own key.
    pub repeated: bool,
}

/// The order in which a decoded message's singular fields first appeared on the wire, as field
/// numbers. Empty for a message built in Rust, whose own keys are then in declaration order.
///
/// A codec that spreads a decoded object and sets a key (`{...decoded, block}`) notes that key
/// here: a key the object has keeps its position, a new one goes last.
///
/// The order is an observation of the decoding, not part of the message's value: two messages
/// with equal fields are equal whatever order their fields came in.
#[derive(Debug, Clone, Default)]
pub struct WireOrder(Vec<u32>);

impl WireOrder {
    /// Note that field `number` appeared: it is appended unless it is already listed.
    pub fn note(&mut self, number: u32) {
        if !self.0.contains(&number) {
            self.0.push(number);
        }
    }

    /// The field numbers, in the order they first appeared.
    pub fn numbers(&self) -> &[u32] {
        &self.0
    }
}

impl PartialEq for WireOrder {
    fn eq(&self, _other: &WireOrder) -> bool {
        true
    }
}

impl Eq for WireOrder {}
