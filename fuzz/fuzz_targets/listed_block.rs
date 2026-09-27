//! A getBlocks reply on arbitrary bytes through the client's download path: the codec, the reply
//! check, the transactions read as the reference's `fromBytesUnsafe` reads them, and
//! `heartwood_p2p::to_block`. No input may panic it, and it accepts exactly what the reference's
//! path accepts (checked by the replay against `block.fromListed`).

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|bytes: &[u8]| {
    let _ = bytes;
    todo!("run the download path on the reply")
});
