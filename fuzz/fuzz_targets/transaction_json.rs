//! Fuzz the transaction JSON reader: JSON text read as a transaction's data, strictly. Not built
//! yet: the target accepts every input. When built, no input may panic, and an accepted
//! transaction's bytes decode back to the same JSON; its replay compares the verdict, the id and
//! the bytes with the reference's transaction factory, with its `vendorField` and
//! `asset.payments` aliases.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|_data: &[u8]| {});
