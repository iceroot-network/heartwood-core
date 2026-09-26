//! The transaction types and their wire numbers.

#![expect(
    unused_variables,
    reason = "the bodies are not written yet; remove this once they are"
)]

use crate::errors::DecodeError;

/// Type group 1, the types inherited from the reference's core.
pub const TYPE_GROUP_CORE: u32 = 1;
/// Type group 2, the reference's own types.
pub const TYPE_GROUP_SOLAR: u32 = 2;

/// A transaction type that this implementation carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TransactionType {
    /// 1/6: transfer to one or more recipients.
    Transfer,
    /// 1/1: second-signature registration.
    SecondSignature,
    /// 1/2: delegate registration.
    DelegateRegistration,
    /// 1/7: delegate resignation.
    DelegateResignation,
    /// 2/0: burn.
    Burn,
    /// 2/2: vote.
    Vote,
}

impl TransactionType {
    /// The type of the wire numbers `type_group` and `type_id`. Types that the reference registers
    /// but this implementation does not carry (1/0, 1/3, 1/4, 1/5, 1/8, 1/9 and 1/10) are
    /// [`DecodeError::RemovedType`]; any other unknown pair is [`DecodeError::UnknownType`].
    pub fn from_wire(type_group: u32, type_id: u16) -> Result<TransactionType, DecodeError> {
        todo!()
    }

    /// The wire type group.
    pub const fn type_group(self) -> u32 {
        match self {
            TransactionType::Transfer
            | TransactionType::SecondSignature
            | TransactionType::DelegateRegistration
            | TransactionType::DelegateResignation => TYPE_GROUP_CORE,
            TransactionType::Burn | TransactionType::Vote => TYPE_GROUP_SOLAR,
        }
    }

    /// The wire type within the type group.
    pub const fn type_id(self) -> u16 {
        match self {
            TransactionType::Transfer => 6,
            TransactionType::SecondSignature => 1,
            TransactionType::DelegateRegistration => 2,
            TransactionType::DelegateResignation => 7,
            TransactionType::Burn => 0,
            TransactionType::Vote => 2,
        }
    }

    /// The reference's key for the type, as used in fee tables.
    pub const fn key(self) -> &'static str {
        match self {
            TransactionType::Transfer => "transfer",
            TransactionType::SecondSignature => "secondSignature",
            TransactionType::DelegateRegistration => "delegateRegistration",
            TransactionType::DelegateResignation => "delegateResignation",
            TransactionType::Burn => "burn",
            TransactionType::Vote => "vote",
        }
    }
}
