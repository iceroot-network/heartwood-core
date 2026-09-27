//! The attester on arbitrary offers: an offer's status text and key list. No input may panic it,
//! and it signs only an offer whose text writes back byte for byte as a status tree and passes the
//! attester's checks (height, slot window, network, own keys).

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|bytes: &[u8]| {
    let _ = bytes;
    todo!("the status offer target")
});
