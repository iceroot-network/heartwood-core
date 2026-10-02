//! Fuzz the address decoder: text read as an address of the devnet's network. Not built yet: the
//! target accepts every input. When built, no input may panic, and an accepted address
//! re-encodes to the same text; its replay compares the verdicts and the network error with the
//! reference's address validation.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|_data: &[u8]| {});
