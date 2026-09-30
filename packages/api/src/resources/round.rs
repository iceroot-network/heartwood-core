//! The round resource of `GET /rounds/{id}/delegates`: a delegate's public key and its votes, the
//! balance it was elected with, as a decimal text.

use heartwood_database::RoundRow;
use serde_json::{Value, json};

/// The stored round row `row` as the route shows it.
pub fn round_delegate(row: &RoundRow) -> Value {
    json!({ "publicKey": row.public_key, "votes": row.balance.to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_delegate_shows_its_key_and_its_balance_as_text() {
        let row = RoundRow {
            round: 3,
            public_key: "02ab".to_owned(),
            balance: 18_446_744_073_709_551_615,
        };
        assert_eq!(
            round_delegate(&row).to_string(),
            r#"{"publicKey":"02ab","votes":"18446744073709551615"}"#
        );
    }
}
