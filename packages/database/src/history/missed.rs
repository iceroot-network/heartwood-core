//! The missed-slot listing of `GET /blocks/missed` and `GET /delegates/{id}/blocks/missed`.

use super::blocks_list::{Query, order_by};
use super::{Listing, Order, Page};
use crate::errors::DatabaseError;
use crate::filters::MissedBlockFilter;
use crate::missed::MissedRow;
use crate::reader::Reader;
use crate::values::Columns;

impl Reader<'_> {
    /// The page `page` of the missed slots that `filter` matches, in the order `order`, with every
    /// match's count.
    ///
    /// Ties of the order are broken by the timestamp, in the direction of the order's last key.
    /// An order on a property that is no column finds nothing.
    pub fn missed_blocks(
        &self,
        filter: &MissedBlockFilter,
        order: &[Order],
        page: Page,
    ) -> Result<Listing<MissedRow>, DatabaseError> {
        let Some(order) = order_by(order, missed_column, "timestamp") else {
            return Ok(Listing::empty());
        };
        let condition = filter.condition()?;
        let read = self.connection().unchecked_transaction()?;
        let query = Query {
            table: "missed_blocks",
            columns: "timestamp, height, username",
            condition: &condition,
            order: &order,
            page,
        };
        let (rows, total_count) = self.page(&query, |row| {
            let columns = Columns::new("missed_blocks", row);
            Ok(MissedRow {
                timestamp: columns.get(0)?,
                height: columns.uint(1)?,
                username: columns.get(2)?,
            })
        })?;
        read.commit()?;
        Ok(Listing {
            rows,
            total_count,
            total_count_is_estimate: false,
        })
    }
}

/// The sort expression of the missed-slot property `property`, if it is a column.
fn missed_column(property: &str) -> Option<&'static str> {
    Some(match property {
        "timestamp" => "timestamp",
        "height" => "height",
        "username" => "username",
        _ => return None,
    })
}
