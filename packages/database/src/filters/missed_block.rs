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
