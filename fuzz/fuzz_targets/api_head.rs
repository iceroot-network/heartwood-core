//! Fuzz the API's reading of a request's head, which Node's parser reads byte by byte before the
//! server serves the connection (`heartwood_api::server::head::head_verdict`, which only the fuzz
//! targets' builds have). Not built yet: the target accepts every input. When built, no input may
//! panic, the work stays bounded, and a head reads the same at once and a few bytes at a time.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|_data: &[u8]| {});
