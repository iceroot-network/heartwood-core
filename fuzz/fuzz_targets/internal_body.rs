//! The internal routes' JSON bodies on arbitrary bytes: the first byte selects one of the nine
//! bodies of `heartwood_p2p_wire::internal` and the rest is the body. No input may panic the
//! reader, and an accepted body writes bytes that read back to the same body.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|bytes: &[u8]| {
    let _ = bytes;
    todo!("the internal bodies target")
});
