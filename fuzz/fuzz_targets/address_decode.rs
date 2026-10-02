//! Fuzz the address decoder: text read as an address of the devnet's network or of the reference's
//! mainnet. No input may panic, and an accepted address re-encodes to the same text, validates
//! for its own network byte and for no other, and is the address `from_base58_for` reads for it.
//!
//! The text is the input's lossy UTF-8 reading, which the replay reads the same way: it sends each
//! corpus file to the reference's `Address.validate` and to its payload decoder, which give the
//! verdict and the network error to compare with Heartwood's.

#![no_main]

use heartwood_crypto::identities::Address;
use libfuzzer_sys::fuzz_target;

/// The address bytes of the devnet and of the reference's mainnet.
const NETWORKS: [u8; 2] = [90, 63];

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let text = text.as_ref();
    for network in NETWORKS {
        let valid = Address::validate(text, network);
        let read = Address::from_base58_for(text, network);
        // A text that validates for a network has a payload that starts with its byte; it is an
        // address of it when the payload is 21 bytes long, which `validate` does not check.
        if read.is_ok() {
            assert!(valid, "an address of network {network} validates for it");
        }
    }
    let Ok(address) = Address::from_base58(text) else {
        return;
    };
    assert_eq!(
        address.to_base58(),
        text,
        "an accepted address is written as it was read"
    );
    assert_eq!(Address::from_base58(&address.to_base58()), Ok(address));
    let own = address.network();
    assert!(Address::validate(text, own));
    assert_eq!(Address::from_base58_for(text, own), Ok(address));
    let other = own.wrapping_add(1);
    assert!(!Address::validate(text, other));
    assert!(Address::from_base58_for(text, other).is_err());
});
