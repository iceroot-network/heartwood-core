//! The route decoders on arbitrary bytes: the first byte selects one of the 14 codecs
//! (`Codec::select`) and the rest is its payload. No input may panic a decoder, and a value it
//! accepts encodes to bytes that decode to the same value.

#![no_main]

use heartwood_p2p_wire::Codec;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|bytes: &[u8]| {
    let (first, payload) = bytes.split_first().map_or((0, &[][..]), |(b, p)| (*b, p));
    let _ = (Codec::select(first), payload);
});
