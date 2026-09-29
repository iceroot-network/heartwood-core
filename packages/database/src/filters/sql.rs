//! A [`TransactionFilter`] as an SQL condition on `transactions`, with its parameters.
//!
//! The condition uses the reader's SQL functions ([`crate::reader`]): `heartwood_like` for the
//! memo's pattern and `heartwood_contains` for the asset's containment of one alternative of a
//! criteria. Each value is a bound parameter.

use rusqlite::types::Value as SqlValue;

use crate::errors::DatabaseError;
use crate::filters::{Numeric, TransactionCriteria, TransactionFilter};
use crate::values::int;

/// The type group and type of a transfer, whose recipients are in the side table.
const TRANSFER: (u64, u64) = (1, 6);

/// The top-level amount as the reference's table holds it: a burn's (type group 2, type 0), and
/// `NULL` for every other type, whose data carries no amount (the table here holds 0 for those).
/// `NULL` matches no criterion and sorts as PostgreSQL sorts it.
pub(crate) const AMOUNT: &str = "(CASE WHEN type_group = 2 AND type = 0 THEN amount END)";

/// An SQL condition and the parameters it binds, in order.
#[derive(Debug, Default)]
pub(crate) struct Condition {
    /// The condition, with a `?` for each parameter.
    pub(crate) sql: String,
    /// The parameters.
    pub(crate) params: Vec<SqlValue>,
}

impl Condition {
    /// The condition of `filter`.
    pub(crate) fn of(filter: &TransactionFilter) -> Result<Condition, DatabaseError> {
        Condition::groups(&filter.groups, Condition::criteria)
    }

    /// The condition that every group of `groups` holds, and a group holds when one of its
    /// criteria does, each as `criteria` writes it.
    pub(crate) fn groups<C>(
        groups: &[Vec<C>],
        criteria: impl Fn(&mut Condition, &C) -> Result<String, DatabaseError>,
    ) -> Result<Condition, DatabaseError> {
        let mut condition = Condition::default();
        let mut all_groups = Vec::with_capacity(groups.len());
        for group in groups {
            let each = group
                .iter()
                .map(|one| criteria(&mut condition, one))
                .collect::<Result<Vec<_>, _>>()?;
            all_groups.push(any(each));
        }
        condition.sql = all(all_groups);
        Ok(condition)
    }

    /// The condition of one criteria: each key it gives, and the type group 1 when it gives a
    /// type without a type group.
    fn criteria(&mut self, criteria: &TransactionCriteria) -> Result<String, DatabaseError> {
        let TransactionCriteria {
            address,
            sender_id,
            recipient_id,
            id,
            version,
            block_height,
            block_id,
            sequence,
            timestamp,
            nonce,
            sender_public_key,
            kind,
            type_group,
            memo,
            amount,
            fee,
            burned_fee,
            asset,
        } = criteria;
        let mut keys = Vec::new();
        let mut key = |values: Vec<String>| {
            if !values.is_empty() {
                keys.push(any(values));
            }
        };
        key(address
            .iter()
            .map(|a| format!("({} OR {})", self.text("sender_id", a), self.recipient(a)))
            .collect());
        key(sender_id
            .iter()
            .map(|a| self.text("sender_id", a))
            .collect());
        key(recipient_id.iter().map(|a| self.recipient(a)).collect());
        key(id.iter().map(|prefix| self.id_prefix(prefix)).collect());
        key(self.integers("version", version)?);
        key(self.numerics("block_height", block_height)?);
        key(block_id.iter().map(|v| self.text("block_id", v)).collect());
        key(self.numerics("sequence", sequence)?);
        key(self.numerics("timestamp", timestamp)?);
        key(self.numerics("nonce", nonce)?);
        key(sender_public_key
            .iter()
            .map(|v| self.text("sender_public_key", v))
            .collect());
        key(self.integers("type", kind)?);
        key(self.integers("type_group", type_group)?);
        key(memo
            .iter()
            .map(|pattern| {
                self.params.push(SqlValue::Blob(pattern.clone()));
                "heartwood_like(?, memo)".to_owned()
            })
            .collect());
        key(self.numerics(AMOUNT, amount)?);
        key(self.numerics("fee", fee)?);
        key(self.numerics("burned_fee", burned_fee)?);
        key(asset
            .iter()
            .map(|value| {
                self.params.push(SqlValue::Text(value.to_string()));
                "heartwood_contains(asset, ?)".to_owned()
            })
            .collect());
        if !kind.is_empty() && type_group.is_empty() {
            keys.push("type_group = 1".to_owned());
        }
        Ok(all(keys))
    }

    /// `column = value`.
    pub(crate) fn text(&mut self, column: &str, value: &str) -> String {
        self.params.push(SqlValue::Text(value.to_owned()));
        format!("{column} = ?")
    }

    /// `address` is a recipient: the row's recipient, or one of a transfer's recipients.
    fn recipient(&mut self, address: &str) -> String {
        self.params.push(SqlValue::Text(address.to_owned()));
        self.params.push(SqlValue::Text(address.to_owned()));
        format!(
            "(recipient_id = ? OR (type_group = {} AND type = {} AND EXISTS (SELECT 1 FROM \
             transaction_recipients r WHERE r.transaction_id = transactions.id AND \
             r.recipient_id = ?)))",
            TRANSFER.0, TRANSFER.1
        )
    }

    /// The id starts with `prefix`, as `id LIKE 'prefix%'`: a prefix without wildcards compares
    /// directly (by the index for a whole id), any other goes through the pattern function.
    pub(crate) fn id_prefix(&mut self, prefix: &str) -> String {
        if prefix.bytes().any(|b| matches!(b, b'%' | b'_' | b'\\')) {
            self.params.push(SqlValue::Text(format!("{prefix}%")));
            return "heartwood_like(?, id)".to_owned();
        }
        if prefix.len() == 64 {
            return self.text("id", prefix);
        }
        self.params.push(SqlValue::Text(prefix.to_owned()));
        self.params.push(SqlValue::Text(prefix.to_owned()));
        "substr(id, 1, length(?)) = ?".to_owned()
    }

    /// `column = value` for each value.
    pub(crate) fn integers(
        &mut self,
        column: &str,
        values: &[u64],
    ) -> Result<Vec<String>, DatabaseError> {
        values
            .iter()
            .map(|&value| {
                self.params.push(SqlValue::Integer(int("criteria", value)?));
                Ok(format!("{column} = ?"))
            })
            .collect()
    }

    /// The numeric criteria `values` on `column`.
    pub(crate) fn numerics(
        &mut self,
        column: &str,
        values: &[Numeric],
    ) -> Result<Vec<String>, DatabaseError> {
        let mut out = Vec::with_capacity(values.len());
        for value in values {
            let mut bound = |value: u64| -> Result<(), DatabaseError> {
                self.params.push(SqlValue::Integer(int("criteria", value)?));
                Ok(())
            };
            out.push(match *value {
                Numeric::Equal(value) => {
                    bound(value)?;
                    format!("{column} = ?")
                }
                Numeric::Range {
                    from: Some(from),
                    to: Some(to),
                } => {
                    bound(from)?;
                    bound(to)?;
                    format!("{column} BETWEEN ? AND ?")
                }
                Numeric::Range {
                    from: Some(from),
                    to: None,
                } => {
                    bound(from)?;
                    format!("{column} >= ?")
                }
                Numeric::Range {
                    from: None,
                    to: Some(to),
                } => {
                    bound(to)?;
                    format!("{column} <= ?")
                }
                Numeric::Range {
                    from: None,
                    to: None,
                } => "1".to_owned(),
            });
        }
        Ok(out)
    }
}

/// Every one of `conditions`; true when there is none.
pub(crate) fn all(conditions: Vec<String>) -> String {
    if conditions.is_empty() {
        "1".to_owned()
    } else {
        format!("({})", conditions.join(" AND "))
    }
}

/// One of `conditions`; false when there is none.
pub(crate) fn any(conditions: Vec<String>) -> String {
    if conditions.is_empty() {
        "0".to_owned()
    } else {
        format!("({})", conditions.join(" OR "))
    }
}
