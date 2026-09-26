//! Fuzz the transaction handlers on the state: an arbitrary wallet fixture and arbitrary
//! transaction bytes. A transaction that decodes is applied, then reverted when it was accepted;
//! neither may panic, a refusal must leave the wallets unchanged, and a revert must restore the
//! fixture's wallets exactly.
//!
//! The target is empty until the handlers are written: it accepts every input.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = data;
});
