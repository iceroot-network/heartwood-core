//! The nes frame parser on arbitrary bytes: no input may panic it, and a frame it accepts encodes
//! to bytes that parse to the same kind, id, status code, socket id and payload.
//!
//! On the way back the writer's defaults apply (id 0 is written as 1, status 0 as 200). The path is
//! not compared, because parsing normalises it again, which is not idempotent. An accepted frame
//! may not be writable: its lossy path or socket id can exceed 255 bytes. A written socket id of
//! 101 to 255 bytes is refused by the parser. When the written frame is not a request and its path
//! is exactly 255 bytes, the parser moves the payload into the path.

#![no_main]

use heartwood_p2p_wire::FrameError;
use heartwood_p2p_wire::nes::{Kind, Message};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|bytes: &[u8]| {
    let Ok(message) = Message::parse(bytes) else {
        return;
    };
    let encoded = match message.encode() {
        Ok(encoded) => encoded,
        Err(FrameError::PathTooLong(n) | FrameError::SocketTooLong(n)) => {
            assert!(n > 255, "a length of {n} bytes fits its byte");
            return;
        }
        Err(error) => panic!("the writer refused a parsed frame: {error:?}"),
    };
    let again = Message::parse(&encoded);
    if message.socket.len() > 100 {
        assert_eq!(again, Err(FrameError::InvalidSocketLength));
        return;
    }
    let again = again.expect("a written frame parses");
    assert_eq!(
        (again.kind, again.version, again.heartbeat),
        (message.kind, message.version, message.heartbeat)
    );
    assert_eq!(again.id, message.id.max(1));
    let status = if message.status_code == 0 {
        200
    } else {
        message.status_code
    };
    assert_eq!(again.status_code, status);
    assert_eq!(again.socket, message.socket);
    if message.kind != Kind::Request && encoded[8] == 255 {
        assert!(again.payload.is_empty());
    } else {
        assert_eq!(again.payload, message.payload);
    }
});
