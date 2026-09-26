//! The status verifier on arbitrary bytes as a getStatus response: no input may panic the
//! decoder, the reply check, either preimage, or the first two signature checks.

#![no_main]

use heartwood_p2p_wire::status;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|bytes: &[u8]| {
    let _ = status::decode_response(bytes);
});
