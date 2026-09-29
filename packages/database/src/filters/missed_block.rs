//! The missed-block criteria of the missed-block routes, as the reference's missed-block filter
//! reads them: every key a criteria gives must hold, and a key holds when one of its values does.

use super::Numeric;
use super::sql::{Condition, keys};
use crate::errors::DatabaseError;

/// One criteria of the reference's missed-block criteria. An empty list is a key the criteria
/// does not give.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MissedBlockCriteria {
    /// `timestamp`.
    pub timestamp: Vec<Numeric>,
    /// `height`.
    pub height: Vec<Numeric>,
    /// `username`.
    pub username: Vec<String>,
}

/// The reference's list of or-criteria over missed slots: every group must hold, and a group
/// holds when one of its criteria does. No group matches every missed slot.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MissedBlockFilter {
    /// The groups.
    pub groups: Vec<Vec<MissedBlockCriteria>>,
}

impl MissedBlockFilter {
    /// The filter of one criteria, as a route's query gives it.
    pub fn of(criteria: MissedBlockCriteria) -> MissedBlockFilter {
        MissedBlockFilter {
            groups: vec![vec![criteria]],
        }
    }
}

impl MissedBlockFilter {
    /// The filter narrowed to the lookback window that starts at `earliest` (seconds since the
    /// epoch), as the reference's missed-block listing narrows each criteria before it reads: one
    /// without a timestamp starts at `earliest`; one exact timestamp before it becomes 0; one
    /// range that starts before it, or has no start, starts at it; a list of several timestamps
    /// is kept as it is. No criteria at all is one criteria without a timestamp.
    #[must_use]
    pub fn within(mut self, earliest: i64) -> MissedBlockFilter {
        if self.groups.is_empty() {
            self.groups.push(vec![MissedBlockCriteria::default()]);
        }
        let start = u64::try_from(earliest).unwrap_or(0);
        let before = |value: u64| i128::from(value) < i128::from(earliest);
        for criteria in self.groups.iter_mut().flatten() {
            match criteria.timestamp.as_mut_slice() {
                [] => criteria.timestamp.push(Numeric::Range {
                    from: Some(start),
                    to: None,
                }),
                [Numeric::Equal(value)] if before(*value) => *value = 0,
                [Numeric::Range { from, .. }] if from.is_none_or(before) => *from = Some(start),
                _ => {}
            }
        }
        self
    }

    /// The filter as an SQL condition on `missed_blocks`, with its parameters.
    pub(crate) fn condition(&self) -> Result<Condition, DatabaseError> {
        Condition::groups(&self.groups, |condition, criteria| criteria.sql(condition))
    }
}

impl MissedBlockCriteria {
    /// Every key the criteria gives, each holding when one of its values does.
    fn sql(&self, c: &mut Condition) -> Result<String, DatabaseError> {
        Ok(keys(vec![
            c.numerics("timestamp", &self.timestamp)?,
            c.numerics("height", &self.height)?,
            c.texts("username", &self.username),
        ]))
    }
}
