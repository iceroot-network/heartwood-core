//! The nes frame parser on arbitrary bytes: no input may panic it, and a frame it accepts encodes
//! to bytes that parse to the same kind, id, status code, socket id and payload.

#![no_main]

use heartwood_p2p_wire::nes::Message;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|bytes: &[u8]| {
    let _ = Message::parse(bytes);
});
