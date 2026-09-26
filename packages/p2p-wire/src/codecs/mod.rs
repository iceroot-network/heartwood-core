//! The route codecs: the reference's encoder and decoder of each public route, request and
//! response, one module per reference codec file.
//!
//! Most directions encode and decode a message as it is. The others post-process, and their
//! values are the objects the reference's decoders return:
//!
//! - getStatus response ([`crate::status::decode_response`]): the three amounts of the header as
//!   [`Amount`]s; a missing `state` or header is refused.
//! - getBlocks response ([`blocks::ListedBlock`]): not a message but a list of block headers, each
//!   with its amounts and its list of transactions.
//! - postBlock request: the block is always present (empty when absent on the wire).
//! - getUnconfirmedTransactions request ([`transactions::UnconfirmedRequest`]): the exclude list
//!   as 32-byte ids.
//! - getUnconfirmedTransactions response and postTransactions request
//!   ([`transactions::WithTransactions`]): the transactions as a list.
//! - getPeers and postTransactions responses: the message's one repeated field.
//!
//! Where the reference spreads a decoded object and sets keys (`{...decoded, block}`), the decoder
//! notes those keys in the message's [`WireOrder`](crate::messages::WireOrder), so that
//! [`Proto::own_keys`](crate::Proto::own_keys) gives the spread object's key order.

pub mod blocks;
pub mod peer;
pub mod transactions;

use std::fmt;

use crate::errors::DecodeError;
use crate::js::{self, BigIntError};

/// An amount as the reference's codecs read it: `new BigNumber(text)`, a JavaScript `BigInt`.
///
/// Its text form ([`fmt::Display`]) is the `BigInt`'s decimal text, which the reference's
/// encoders write (`toString()`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Amount(i128);

impl Amount {
    /// Zero.
    pub const ZERO: Amount = Amount(0);

    /// The amount `value`.
    pub const fn new(value: i128) -> Amount {
        Amount(value)
    }

    /// The value.
    pub const fn value(self) -> i128 {
        self.0
    }

    /// `new BigNumber(text)`: white space around the literal is ignored, an empty text is 0, a
    /// decimal literal may carry a sign, and `0x`, `0o` and `0b` select a base.
    ///
    /// [`DecodeError::AmountSyntax`] where `BigInt` throws, and [`DecodeError::AmountTooWide`]
    /// where the value needs more than 128 bits, which the reference reads (a documented
    /// difference).
    pub fn parse(text: &str) -> Result<Amount, DecodeError> {
        js::parse_bigint(text)
            .map(Amount)
            .map_err(|error| match error {
                BigIntError::Syntax => DecodeError::AmountSyntax,
                BigIntError::TooWide => DecodeError::AmountTooWide,
            })
    }
}

impl From<u64> for Amount {
    fn from(value: u64) -> Amount {
        Amount(i128::from(value))
    }
}

impl fmt::Display for Amount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests;
