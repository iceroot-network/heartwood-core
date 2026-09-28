//! The server's nes session on a script of client frames and delays: no script may panic it, and
//! the session ends where the reference's server would, with a typed reason (the ban reason the
//! reference gives), or stays open.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|bytes: &[u8]| {
    let _ = bytes;
    todo!("the server session target")
});
