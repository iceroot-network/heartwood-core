//! The cases of the message formats: amounts over the whole `i64` and `u64` range, weighted to
//! `2^53` and to amounts whose double lies at a tie of the ninth fraction digit; token symbols;
//! counts; and transactions of the six kept types.

use proptest::strategy::BoxedStrategy;

use crate::strategies::kernel::OpCase;

/// The cases of the `text.*` operations.
pub fn text_cases() -> BoxedStrategy<OpCase> {
    todo!("the text cases")
}
