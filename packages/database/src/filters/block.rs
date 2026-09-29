//! The block criteria of the block routes, as the reference's block filter reads them: every key a
//! criteria gives must hold, and a key holds when one of its values does. `id` is a prefix, as the
//! reference's `LIKE 'id%'`; the numeric keys take a value or a range.

use super::Numeric;
use super::sql::{Condition, all, any};
use crate::errors::DatabaseError;

/// One criteria of the reference's block criteria. An empty list is a key the criteria does not
/// give.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BlockCriteria {
    /// `id`: prefixes of the block id.
    pub id: Vec<String>,
    /// `version`.
    pub version: Vec<u64>,
    /// `timestamp`.
    pub timestamp: Vec<Numeric>,
    /// `previousBlock`.
    pub previous_block: Vec<String>,
    /// `height`.
    pub height: Vec<Numeric>,
    /// `numberOfTransactions`.
    pub number_of_transactions: Vec<Numeric>,
    /// `totalAmount`.
    pub total_amount: Vec<Numeric>,
    /// `totalFee`.
    pub total_fee: Vec<Numeric>,
    /// `burnedFee`.
    pub burned_fee: Vec<Numeric>,
    /// `reward`.
    pub reward: Vec<Numeric>,
    /// `payloadLength`.
    pub payload_length: Vec<Numeric>,
    /// `payloadHash`.
    pub payload_hash: Vec<String>,
    /// `generatorPublicKey`.
    pub generator_public_key: Vec<String>,
    /// `username`.
    pub username: Vec<String>,
    /// `blockSignature`.
    pub block_signature: Vec<String>,
}

/// The reference's list of or-criteria over blocks: every group must hold, and a group holds when
/// one of its criteria does. No group matches every block.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BlockFilter {
    /// The groups.
    pub groups: Vec<Vec<BlockCriteria>>,
}

impl BlockFilter {
    /// The filter of one criteria, as a route's query gives it.
    pub fn of(criteria: BlockCriteria) -> BlockFilter {
        BlockFilter {
            groups: vec![vec![criteria]],
        }
    }
}

impl BlockFilter {
    /// The filter as an SQL condition on `blocks`, with its parameters.
    pub(crate) fn condition(&self) -> Result<Condition, DatabaseError> {
        Condition::groups(&self.groups, |condition, criteria| criteria.sql(condition))
    }
}

impl BlockCriteria {
    /// Every key the criteria gives, each holding when one of its values does.
    fn sql(&self, c: &mut Condition) -> Result<String, DatabaseError> {
        let mut keys = Vec::new();
        let mut key = |values: Vec<String>| {
            if !values.is_empty() {
                keys.push(any(values));
            }
        };
        key(self.id.iter().map(|prefix| c.id_prefix(prefix)).collect());
        key(c.integers("version", &self.version)?);
        key(c.numerics("timestamp", &self.timestamp)?);
        key(text(c, "previous_block", &self.previous_block));
        key(c.numerics("height", &self.height)?);
        key(c.numerics("number_of_transactions", &self.number_of_transactions)?);
        key(c.numerics("total_amount", &self.total_amount)?);
        key(c.numerics("total_fee", &self.total_fee)?);
        key(c.numerics("burned_fee", &self.burned_fee)?);
        key(c.numerics("reward", &self.reward)?);
        key(c.numerics("payload_length", &self.payload_length)?);
        key(text(c, "payload_hash", &self.payload_hash));
        key(text(c, "generator_public_key", &self.generator_public_key));
        key(text(c, "username", &self.username));
        key(text(c, "block_signature", &self.block_signature));
        Ok(all(keys))
    }
}

/// `column = value` for each value.
fn text(c: &mut Condition, column: &str, values: &[String]) -> Vec<String> {
    values.iter().map(|value| c.text(column, value)).collect()
}
