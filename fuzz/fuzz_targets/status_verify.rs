//! The status verifier on arbitrary bytes as a getStatus response: no input may panic the
//! decoder, the reply check, either preimage, or the first two signature checks.
//!
//! A decoded response is also written again, and the writing must read back to the same amounts
//! and be written again byte for byte.

#![no_main]

use heartwood_p2p_wire::status;
use libfuzzer_sys::fuzz_target;

/// The reply schema's bound on keys and signatures under the devnet milestones.
const MAX_DELEGATES: usize = 53;

fuzz_target!(|bytes: &[u8]| {
    let Ok(mut received) = status::decode_response(bytes) else {
        return;
    };

    let written = status::encode_response(&received);
    let again = status::decode_response(&written).expect("a written response reads back");
    assert_eq!(
        (again.total_amount, again.total_fee, again.reward),
        (received.total_amount, received.total_fee, received.reward)
    );
    assert_eq!(status::encode_response(&again), written);

    let _ = status::check_reply(&mut received, MAX_DELEGATES);
    let preimages = [false, true].map(|negative| status::verifying_preimage(&received, negative));
    let message = &received.message;
    for (key, signature) in message.public_keys.iter().zip(&message.signatures).take(2) {
        for preimage in &preimages {
            let _ = status::verify_attestation(preimage, key, signature);
        }
    }
});
