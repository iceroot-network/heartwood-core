use super::*;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

/// A frame with the given type byte, path, socket id and payload, id 1 and status 200.
fn frame(kind: u8, path: &[u8], socket: &[u8], payload: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0, kind, 0, 0, 0, 1, 0, 200];
    bytes.extend([path.len() as u8, socket.len() as u8, 0, 0, 0, 0]);
    bytes.extend([path, socket, payload].concat());
    bytes
}

/// The frames as the reference's writer writes them (Node 18.20.8).
#[test]
fn constructors_write_the_reference_frames() {
    let heartbeat = Heartbeat {
        interval: 15000,
        timeout: 5000,
    };
    let body = br#"{"error":"Unknown","message":"null"}"#;
    let cases = [
        (Message::client_hello(1), "02000000000100c8000000000000"),
        (
            Message::request(2, RouteId::GetStatus, vec![0x0a, 0]),
            "00030000000200c81200000000007032702e706565722e6765745374617475730a00",
        ),
        (
            Message::server_hello(1, heartbeat, "s:1".into()),
            "00000000000100c800033a981388733a31",
        ),
        (
            Message::response(2, vec![0x0a, 0]),
            "00030000000200c80000000000000a00",
        ),
        (Message::ping(0), "00010000000100c8000000000000"),
        (
            Message::graceful_error(3, body),
            "00030000000301f3000000000000\
             7b226572726f72223a22556e6b6e6f776e222c226d657373616765223a226e756c6c227d",
        ),
    ];
    for (message, expected) in cases {
        let bytes = message.encode().unwrap();
        assert_eq!(hex(&bytes), expected, "{message:?}");
        let parsed = Message::parse(&bytes).unwrap();
        assert_eq!(
            (parsed.kind, parsed.id, parsed.socket, parsed.payload),
            (
                message.kind,
                message.id.max(1),
                message.socket,
                message.payload
            )
        );
    }
}

#[test]
fn writer_defaults() {
    let message = Message {
        kind: Kind::Error,
        id: 0,
        status_code: 0,
        ..Message::ping(0)
    };
    assert_eq!(
        hex(&message.encode().unwrap()),
        "00090000000100c8000000000000"
    );
}

#[test]
fn only_the_peer_to_peer_types_are_accepted() {
    for kind in 0..=u8::MAX {
        let parsed = Message::parse(&frame(kind, b"", b"", b""));
        match kind {
            0 | 1 | 3 | 9 => assert_eq!(parsed.unwrap().kind.byte(), kind),
            _ => assert_eq!(parsed, Err(FrameError::InvalidType(kind))),
        }
    }
}

#[test]
fn length_checks() {
    let hello = frame(0, b"", b"", b"");
    for length in 0..14 {
        assert_eq!(
            Message::parse(&hello[..length]),
            Err(FrameError::TooShort(length))
        );
    }
    let mut overrun = frame(3, b"abc", b"", b"");
    overrun[8] = 4;
    assert_eq!(Message::parse(&overrun), Err(FrameError::InvalidPathLength));
    overrun[8] = 3;
    overrun[9] = 1;
    assert_eq!(
        Message::parse(&overrun),
        Err(FrameError::InvalidSocketLength)
    );
    let socket = [b's'; 101];
    let parsed = Message::parse(&frame(0, b"", &socket[..100], b"x")).unwrap();
    assert_eq!((parsed.socket.len(), parsed.payload), (100, b"x".to_vec()));
    assert_eq!(
        Message::parse(&frame(0, b"", &socket, b"")),
        Err(FrameError::InvalidSocketLength)
    );
}

#[test]
fn a_socket_id_the_parser_refuses_is_written() {
    let message = Message::server_hello(1, Heartbeat::default(), "s".repeat(101));
    let bytes = message.encode().unwrap();
    assert_eq!(bytes[9], 101);
    assert_eq!(Message::parse(&bytes), Err(FrameError::InvalidSocketLength));
    let message = Message::server_hello(1, Heartbeat::default(), "s".repeat(256));
    assert_eq!(message.encode(), Err(FrameError::SocketTooLong(256)));
}

#[test]
fn the_path_is_normalised_and_decoded_lossily() {
    let parsed = Message::parse(&frame(3, b"a/../p2p.peer.getStatus", b"", b"")).unwrap();
    assert_eq!(parsed.path, "/p2p.peer.getStatus");
    let parsed = Message::parse(&frame(3, &[0xff, 0xfe], &[0xc3], b"")).unwrap();
    assert_eq!(parsed.path, "/%EF%BF%BD%EF%BF%BD");
    assert_eq!(parsed.socket, "\u{FFFD}");
}

#[test]
fn a_255_byte_path_takes_the_payload_unless_the_frame_is_a_post() {
    let path = [b'a'; 255];
    for kind in [0, 1, 9] {
        let parsed = Message::parse(&frame(kind, &path, b"", b"b\xff")).unwrap();
        assert_eq!(parsed.path, format!("/{}b\u{FFFD}", "a".repeat(255)));
        assert!(parsed.payload.is_empty());
    }
    let parsed = Message::parse(&frame(3, &path, b"", b"b")).unwrap();
    assert_eq!(parsed.path.len(), 256);
    assert_eq!(parsed.payload, b"b");
}

#[test]
fn the_writer_cuts_a_long_path_unless_the_frame_is_a_request() {
    let hello = Message {
        path: "a".repeat(256),
        payload: b"dropped".to_vec(),
        ..Message::client_hello(1)
    };
    let bytes = hello.encode().unwrap();
    assert_eq!(bytes[8], 255);
    assert_eq!(&bytes[14 + 255..], b"a");
    let request = Message {
        kind: Kind::Request,
        ..hello
    };
    assert_eq!(request.encode(), Err(FrameError::PathTooLong(256)));
    // A cut through a surrogate pair leaves U+FFFD at the end of the path, which is then too long.
    let split = Message {
        path: format!("{}\u{1F600}b", "a".repeat(254)),
        ..Message::ping(1)
    };
    assert_eq!(split.encode(), Err(FrameError::PathTooLong(257)));
    let wide = Message {
        path: "\u{e9}".repeat(200),
        ..Message::ping(1)
    };
    assert_eq!(wide.encode(), Err(FrameError::PathTooLong(400)));
}

#[test]
fn header_fields_round_trip() {
    let bytes = unhex("ff09ffffffffffff0000ffffffff0102");
    let parsed = Message::parse(&bytes).unwrap();
    assert_eq!(
        (parsed.version, parsed.id, parsed.status_code),
        (255, u32::MAX, u16::MAX)
    );
    assert_eq!(parsed.path, "");
    assert_eq!(
        parsed.heartbeat,
        Heartbeat {
            interval: u16::MAX,
            timeout: u16::MAX
        }
    );
    assert_eq!(parsed.payload, [1, 2]);
    assert_eq!(parsed.encode().unwrap(), bytes);
}
