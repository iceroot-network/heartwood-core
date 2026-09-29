//! The missed-slot listing of `GET /blocks/missed` and `GET /delegates/{id}/blocks/missed`.

use super::{Listing, Order, Page};
use crate::errors::DatabaseError;
use crate::filters::MissedBlockFilter;
use crate::missed::MissedRow;
use crate::reader::Reader;

impl Reader<'_> {
    /// The page `page` of the missed slots that `filter` matches, in the order `order`, with every
    /// match's count.
    pub fn missed_blocks(
        &self,
        filter: &MissedBlockFilter,
        order: &[Order],
        page: Page,
    ) -> Result<Listing<MissedRow>, DatabaseError> {
        let _ = (filter, order, page);
        todo!("the missed-slot listing")
    }
}
