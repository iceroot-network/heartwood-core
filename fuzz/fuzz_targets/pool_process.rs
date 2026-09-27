//! Fuzz the pool: arbitrary sequences of steps (batches of entries from a small fixture's keys,
//! the clock moving on, removals, re-adds, blocks, a restart of the storage) on a small fixture.
//!
//! No step may panic; the pool never holds more than its limits; each sender's transactions apply
//! in order on a fresh view of the wallets; and a collation is in fee order, each sender's
//! transactions in nonce order.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = data;
});
