//! The status verifier on arbitrary bytes as a getStatus response: no input may panic the
//! decoder, the reply check, either preimage of a reply the check accepts, or the first two
//! signature checks.
//!
//! A decoded response is also written again, and the writing must read back to the same amounts
//! and be written again byte for byte.

#![no_main]

use heartwood_p2p_wire::status;
use libfuzzer_sys::fuzz_target;

/// The reply schema's bound on keys and signatures under the devnet milestones.
const MAX_DELEGATES: usize = 53;

fuzz_target!(|bytes: &[u8]| {
    let Ok(received) = status::decode_response(bytes) else {
        return;
    };

    let written = status::encode_response(&received);
    let again = status::decode_response(&written).expect("a written response reads back");
    assert_eq!(
        (again.total_amount, again.total_fee, again.reward),
        (received.total_amount, received.total_fee, received.reward)
    );
    assert_eq!(status::encode_response(&again), written);

    let Ok(checked) = status::check_reply(received, MAX_DELEGATES) else {
        return;
    };
    let preimages = [false, true].map(|negative| status::verifying_preimage(&checked, negative));
    let message = &checked.message;
    for (key, signature) in message.public_keys.iter().zip(&message.signatures).take(2) {
        for preimage in &preimages {
            let _ = status::verify_attestation(preimage, key, signature);
        }
    }
});
