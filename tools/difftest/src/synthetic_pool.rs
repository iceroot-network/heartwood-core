//! The synthetic pool cases on a corpus chain's tip: transactions added on both sides, blocks
//! forged from the collated candidates and applied, the synced node's sequence after each block
//! (remove forged, clean up, re-add), reverts with the re-add of their transactions, the online
//! record's expiry, a rule change at the next height, and a restart of the pool's storage.
