//! Fuzz the API's reading of a request's head, which Node's parser reads byte by byte before the
//! server serves the connection (`heartwood_api::server::head::head_verdict`, which only the fuzz
//! targets' builds have). No input may panic, the work and the head written for the server stay
//! bounded, and a head reads the same at once and a few bytes at a time.
//!
//! The first byte of an input is the number of bytes the head is fed at a time (0 is one at a
//! time); the rest is the connection's bytes. This target is crash-only: the oracle has no
//! operation that reads a head as Node's parser does, so there is no replay to compare with. The
//! unit tests of the reader hold what Node 18.20.8 and hapi 20.2.1 were measured to do.

#![no_main]
#![allow(unexpected_cfgs)]

use libfuzzer_sys::fuzz_target;

/// A head written for the server grows by at most its percent-encoding (three bytes for one) and
/// the line ends it adds, plus what follows the head unchanged.
const GROWTH: usize = 4;

/// The room the written head has beyond `GROWTH` times the input.
const SLACK: usize = 4096;

#[cfg(fuzzing)]
fn check(data: &[u8]) {
    use heartwood_api::server::head::{HeadVerdict, head_verdict};

    let Some((&step, bytes)) = data.split_first() else {
        return;
    };
    let whole = head_verdict(bytes, usize::MAX);
    let split = head_verdict(bytes, usize::from(step));
    assert_eq!(whole, split, "a head reads the same at once and {step} bytes at a time");
    if let Some(HeadVerdict::Ended(Some(served))) = &whole {
        assert!(
            served.len() <= GROWTH * bytes.len() + SLACK,
            "a head of {} bytes is written as {}",
            bytes.len(),
            served.len()
        );
    }
}

fuzz_target!(|data: &[u8]| {
    #[cfg(fuzzing)]
    check(data);
    #[cfg(not(fuzzing))]
    let _ = (data, GROWTH, SLACK);
});
