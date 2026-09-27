//! The reply checks of the routes other than getStatus on arbitrary bytes: the first byte selects
//! the route (`byte % 6` over `heartwood_p2p_wire::replies::ROUTES`) and the rest is the reply. No
//! input may panic the codec or the check, and an accepted reply encodes to bytes that decode and
//! check to the same value.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|bytes: &[u8]| {
    let _ = bytes;
    todo!("decode and check the selected reply")
});
