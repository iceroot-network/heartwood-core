//! The nes client session fed a script of server frames and delays: no input may panic it, the
//! pending requests stay bounded, every failure is a typed error, and the connection ends where
//! the reference's client would end it.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|bytes: &[u8]| {
    let _ = bytes;
    todo!("drive a session with the script in the input")
});
