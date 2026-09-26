//! The transaction types and their wire numbers.

use crate::errors::DecodeError;

/// Type group 1, the core types: transfer, second-signature registration, delegate registration
/// and delegate resignation.
pub const TYPE_GROUP_CORE: u32 = 1;
/// Type group 2: burn and vote.
pub const TYPE_GROUP_2: u32 = 2;

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
        match (type_group, type_id) {
            (TYPE_GROUP_CORE, 1) => Ok(TransactionType::SecondSignature),
            (TYPE_GROUP_CORE, 2) => Ok(TransactionType::DelegateRegistration),
            (TYPE_GROUP_CORE, 6) => Ok(TransactionType::Transfer),
            (TYPE_GROUP_CORE, 7) => Ok(TransactionType::DelegateResignation),
            (TYPE_GROUP_2, 0) => Ok(TransactionType::Burn),
            (TYPE_GROUP_2, 2) => Ok(TransactionType::Vote),
            (TYPE_GROUP_CORE, 0 | 3 | 4 | 5 | 8 | 9 | 10) => Err(DecodeError::RemovedType {
                type_group,
                type_id,
            }),
            _ => Err(DecodeError::UnknownType {
                type_group,
                type_id,
            }),
        }
    }

    /// Every type this implementation carries, in wire order (type group, then type).
    pub const ALL: [TransactionType; 6] = [
        TransactionType::SecondSignature,
        TransactionType::DelegateRegistration,
        TransactionType::Transfer,
        TransactionType::DelegateResignation,
        TransactionType::Burn,
        TransactionType::Vote,
    ];

    /// The wire type group.
    pub const fn type_group(self) -> u32 {
        match self {
            TransactionType::Transfer
            | TransactionType::SecondSignature
            | TransactionType::DelegateRegistration
            | TransactionType::DelegateResignation => TYPE_GROUP_CORE,
            TransactionType::Burn | TransactionType::Vote => TYPE_GROUP_2,
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

    /// The fee the reference's builders use when the milestone's `fees.staticFees` has no entry
    /// for the type.
    pub const fn default_static_fee(self) -> u64 {
        match self {
            TransactionType::Transfer => 10_000_000,
            TransactionType::SecondSignature => 500_000_000,
            TransactionType::DelegateRegistration | TransactionType::DelegateResignation => {
                2_500_000_000
            }
            TransactionType::Burn => 0,
            TransactionType::Vote => 100_000_000,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_numbers_round_trip() {
        for kind in TransactionType::ALL {
            assert_eq!(
                TransactionType::from_wire(kind.type_group(), kind.type_id()),
                Ok(kind)
            );
        }
    }

    #[test]
    fn removed_and_unknown_types() {
        for type_id in [0, 3, 4, 5, 8, 9, 10] {
            assert_eq!(
                TransactionType::from_wire(1, type_id),
                Err(DecodeError::RemovedType {
                    type_group: 1,
                    type_id
                })
            );
        }
        for (type_group, type_id) in [(1, 11), (2, 1), (3, 0), (0, 6), (u32::MAX, u16::MAX)] {
            assert_eq!(
                TransactionType::from_wire(type_group, type_id),
                Err(DecodeError::UnknownType {
                    type_group,
                    type_id
                })
            );
        }
    }
}
