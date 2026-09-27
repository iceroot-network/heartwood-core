//! The internal routes' JSON bodies on arbitrary bytes: the first byte selects one of the nine
//! bodies of `heartwood_p2p_wire::internal` and the rest is the body. No input may panic the
//! reader, and an accepted body writes bytes that read back to a body that writes the same bytes.
//!
//! The bytes are compared rather than the bodies: a body keeps the JSON values of the fields the
//! forger does not read, and `JSON.stringify` writes some of them in another form than they came
//! in (a number such as `1.0` as `1`, the keys that read as array indexes first), which a second
//! writing no longer changes.

#![no_main]

use heartwood_p2p_wire::internal::{
    self, CurrentRound, EmitEvent, InternalBody, NetworkState, NetworkStateRequest, SlotNumber,
    SlotNumberRequest, StatusSignatures, StatusToSignRequest, StatusToSignResponse,
};
use libfuzzer_sys::fuzz_target;

/// Read `bytes` as `T`; when accepted, write it, and read and write it again.
fn round_trip<T: InternalBody>(bytes: &[u8]) {
    let Ok(body) = internal::decode::<T>(bytes) else {
        return;
    };
    let written = internal::encode(&body);
    let again: T = internal::decode(&written).expect("written bytes read back");
    assert_eq!(
        internal::encode(&again),
        written,
        "a second writing changed the bytes"
    );
}

fuzz_target!(|bytes: &[u8]| {
    let Some((selector, body)) = bytes.split_first() else {
        return;
    };
    match selector % 9 {
        0 => round_trip::<CurrentRound>(body),
        1 => round_trip::<SlotNumberRequest>(body),
        2 => round_trip::<SlotNumber>(body),
        3 => round_trip::<NetworkStateRequest>(body),
        4 => round_trip::<NetworkState>(body),
        5 => round_trip::<EmitEvent>(body),
        6 => round_trip::<StatusToSignRequest>(body),
        7 => round_trip::<StatusToSignResponse>(body),
        _ => round_trip::<StatusSignatures>(body),
    }
});
