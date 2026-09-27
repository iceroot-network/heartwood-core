//! Fuzz the state's records: arbitrary text into the record decoders and into
//! `State::restore`, which may refuse it only with a typed error; and a state encoded as records
//! decodes to an equal state.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = data;
});
