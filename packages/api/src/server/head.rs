//! A request's head as Node's parser reads it, read before hyper serves the connection.
//!
//! The reference serves its API with Node 18, whose parser (llhttp 6.1.1, built in its loose mode)
//! reads a request's head byte by byte, and whose server refuses a head whose target plus header
//! names and values reach 16 KiB (its `--max-http-header-size`). It counts a value from its first
//! byte that is not a space or a tab, its trailing white space included; not the method, the
//! version, the colons or the line ends. hapi answers every refusal of that parser with a bare
//! `400 Bad Request` and ends the connection.
//!
//! `read` reads a connection to its head's end as that parser does: it counts as Node counts,
//! refuses what the parser refuses, and closes the connection without an answer where Node's
//! server gives none (a `CONNECT`; a method Node's server does not know, on which the reference's
//! process stops; a stream that ends before its head does). A head the parser accepts is written
//! again for hyper, which is stricter, with the meaning Node's server gives it: without the line
//! ends before the request line; with single spaces, `HTTP/1.1` (Node answers every version it
//! accepts as `HTTP/1.1`) and a target without its fragment, in which a byte beyond ASCII is the
//! UTF-8 of the Latin-1 character Node reads and another byte hyper refuses is percent-encoded;
//! with the header fields Node's server passes on (the first 1,000, and after them those that
//! frame the body), less those whose name hyper refuses (empty, or with a space: none that the
//! API reads), an empty `Transfer-Encoding` (no coding for Node's parser), and `Expect` but for
//! `HTTP/1.1`, the only version Node reads it for; with `\r\n` line ends. `Rewind` then gives
//! hyper that head, followed by the rest of the connection. The request carries the target as
//! Node gives it to hapi (`NodeRead`), which the plugins read, and where Node's parser fails on
//! what was read after the head while hapi serves the request, which hapi then answers with its
//! 400 (`ClientError`); a request after it that Node's server closes the connection on leaves
//! the first unanswered.

use std::io;
use std::ops::Range;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::TcpStream;

use crate::plugins::{ClientError, NodeRead};

/// Node's limit on the counted bytes of a request's head.
const MAX_HEAD: usize = 16_384;

/// The most bytes read without the head ending, counted or not: hyper's default read buffer, the
/// bound the server kept before it read heads itself. A head refused only by this bound (white
/// space or line ends Node does not count) is refused the same way.
const MAX_READ: usize = 8192 + 4096 * 100;

/// The header fields Node's server passes on to hapi: its parser's `maxHeaderPairs` (2,000) caps
/// the list of names and values it gives, so 1,000 fields.
const MAX_FIELDS: usize = 1000;

/// hapi's answer to a request that Node's parser refuses.
const BAD_REQUEST: &[u8] = b"HTTP/1.1 400 Bad Request\r\n\r\n";

/// llhttp's methods, in its order. A request line may name any of them; the first 34 are those
/// Node's server knows, and one more, `PRI`, is the rest of those an `HTTP/` request may name.
const METHODS: [&str; 46] = [
    "DELETE",
    "GET",
    "HEAD",
    "POST",
    "PUT",
    "CONNECT",
    "OPTIONS",
    "TRACE",
    "COPY",
    "LOCK",
    "MKCOL",
    "MOVE",
    "PROPFIND",
    "PROPPATCH",
    "SEARCH",
    "UNLOCK",
    "BIND",
    "REBIND",
    "UNBIND",
    "ACL",
    "REPORT",
    "MKACTIVITY",
    "CHECKOUT",
    "MERGE",
    "M-SEARCH",
    "NOTIFY",
    "SUBSCRIBE",
    "UNSUBSCRIBE",
    "PATCH",
    "PURGE",
    "MKCALENDAR",
    "LINK",
    "UNLINK",
    "SOURCE",
    "PRI",
    "DESCRIBE",
    "ANNOUNCE",
    "SETUP",
    "PLAY",
    "PAUSE",
    "TEARDOWN",
    "GET_PARAMETER",
    "SET_PARAMETER",
    "REDIRECT",
    "RECORD",
    "FLUSH",
];

/// The methods Node's server knows (`http.METHODS`).
const KNOWN_METHODS: usize = 34;

/// What llhttp expects after a `PRI` request line: the rest of HTTP/2's connection preface.
const PREFACE: &[u8] = b"\r\n\r\nSM\r\n\r\n";

/// The protocol of a request line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Protocol {
    Http,
    Rtsp,
    Ice,
}

impl Protocol {
    const NAMES: [(&'static [u8], Protocol); 3] = [
        (b"HTTP/", Protocol::Http),
        (b"RTSP/", Protocol::Rtsp),
        (b"ICE/", Protocol::Ice),
    ];

    /// Whether a request line of this protocol may name the method `index` of [`METHODS`].
    fn allows(self, index: usize) -> bool {
        let name = METHODS.get(index).copied().unwrap_or_default();
        match self {
            Protocol::Http => index <= KNOWN_METHODS,
            Protocol::Rtsp => index > KNOWN_METHODS || matches!(name, "OPTIONS" | "GET" | "POST"),
            Protocol::Ice => name == "SOURCE",
        }
    }
}

/// The header fields whose value llhttp reads, and the others it treats apart: no space may come
/// between their name and the colon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    General,
    ContentLength,
    TransferEncoding,
    /// `Connection` and `Proxy-Connection`, whose tokens llhttp reads once the value is read.
    Connection,
    Upgrade,
}

const SPECIAL_FIELDS: [(&[u8], Field); 5] = [
    (b"connection", Field::Connection),
    (b"content-length", Field::ContentLength),
    (b"proxy-connection", Field::Connection),
    (b"transfer-encoding", Field::TransferEncoding),
    (b"upgrade", Field::Upgrade),
];

/// The `Connection` tokens llhttp reads, as flags: `close`, `keep-alive` and `upgrade`.
const CONNECTION_TOKENS: [&[u8]; 3] = [b"close", b"keep-alive", b"upgrade"];
const CLOSE: u8 = 1;
const KEEP_ALIVE: u8 = 2;
const UPGRADE: u8 = 4;

/// The flags of the tokens llhttp finds in a `Connection` value: each of [`CONNECTION_TOKENS`]
/// that is a whole item between commas, in any case, with only spaces or tabs before it and
/// spaces after it.
fn connection_flags(value: &[u8]) -> u8 {
    let mut flags = 0;
    for item in value.split(|&byte| byte == b',') {
        let start = item.iter().position(|b| !matches!(b, b' ' | b'\t'));
        let end = item.iter().rposition(|&b| b != b' ').map_or(0, |at| at + 1);
        let token = item.get(start.unwrap_or(end)..end).unwrap_or_default();
        let found = CONNECTION_TOKENS
            .iter()
            .position(|t| token.eq_ignore_ascii_case(t));
        flags |= found.map_or(0, |at| 1 << at);
    }
    flags
}

/// Where a head's reading stands: llhttp's states for a request, merged where they read alike.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Line ends before the request line, which llhttp skips.
    Start,
    Method,
    SpacesBeforeTarget,
    TargetStart,
    Schema,
    SchemaSlashes(u8),
    /// The authority; `at` just after an `@`.
    Server {
        at: bool,
    },
    Path,
    Query,
    Fragment,
    /// A `\r` after the target (an HTTP/0.9 request line): `\n` must follow.
    TargetCr,
    /// Spaces after the target, then a protocol.
    ProtocolStart,
    Protocol(Protocol, usize),
    Major,
    Dot,
    Minor,
    LineEnd,
    LineCr,
    Preface(usize),
    FieldStart,
    /// A `\r` that starts a line ends the head with the next byte, whatever it is.
    HeadCr,
    Name,
    SpaceBeforeValue,
    EmptyValueCr,
    EmptyValueEnd,
    Value,
    ValueCr,
    ValueEnd,
    Digits,
    DigitsSpace,
    Chunked(usize),
    ChunkedLast,
    CodingToken,
    CodingSpace,
}

/// What Node's server makes of the bytes read after a request's head ([`Head::after`]).
enum After {
    Nothing,
    /// Its parser fails while hapi serves the request.
    Failed(ClientError),
    /// It closes the connection before the request's answer.
    Closed,
}

/// What a byte does to the head.
enum Step {
    /// The byte is read.
    Next,
    /// The byte is read again in the new state.
    Again,
    /// The head ended with this byte.
    Ended,
    /// Node's parser refuses the head.
    Refused,
}

/// What the bytes read so far make of a head.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Progress {
    More,
    Ended,
    Refused,
}

/// Node's reading of a request's head, byte by byte.
#[derive(Debug)]
struct Head {
    state: State,
    /// The offset of the byte being read.
    at: usize,
    /// The bytes Node counts toward its limit.
    counted: usize,
    method: Vec<u8>,
    /// The method, once the request line named one of [`METHODS`].
    known: Option<usize>,
    target: Range<usize>,
    /// Where the target's fragment starts.
    fragment: Option<usize>,
    version: (u8, u8),
    field: Field,
    name: Range<usize>,
    value: Range<usize>,
    /// The header fields read, as Node's server counts them.
    fields: usize,
    /// The fields hyper is given: their name and value.
    kept: Vec<(Range<usize>, Range<usize>)>,
    content_length: u64,
    has_length: bool,
    has_coding: bool,
    chunked: bool,
    /// The `Connection` tokens read ([`connection_flags`]).
    connection: u8,
    /// Whether a non-empty `Upgrade` was read.
    upgrade: bool,
}

/// Whether llhttp reads `byte` in a target's path, query or fragment (its loose `URL_CHAR`,
/// without `?` and `#`).
fn is_url_byte(byte: u8) -> bool {
    matches!(byte, b'\t' | 0x0c | 0x80..=0xff) || byte.is_ascii_graphic() && !b"#?".contains(&byte)
}

/// Whether llhttp reads `byte` in a target's authority (`USERINFO_CHARS`, `[` and `]`).
fn is_server_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"-_.!~*'()%;:&=+$,[]".contains(&byte)
}

/// Whether llhttp reads `byte` in a header's name (its loose `TOKEN`, with the space).
fn is_token(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~ ".contains(&byte)
}

/// Whether llhttp reads `byte` in a header's value (`HEADER_CHARS`).
fn is_value_byte(byte: u8) -> bool {
    byte == b'\t' || byte >= b' ' && byte != 0x7f
}

/// Whether hyper takes `byte` as it is in both a target's path and its query.
fn hyper_takes(byte: u8) -> bool {
    matches!(byte, 0x21 | 0x24..=0x3b | 0x3d | 0x3f..=0x5f | 0x61..=0x7e)
}

impl Head {
    fn new() -> Head {
        Head {
            state: State::Start,
            at: 0,
            counted: 0,
            method: Vec::new(),
            known: None,
            target: 0..0,
            fragment: None,
            version: (0, 9),
            field: Field::General,
            name: 0..0,
            value: 0..0,
            fields: 0,
            kept: Vec::new(),
            content_length: 0,
            has_length: false,
            has_coding: false,
            chunked: false,
            connection: 0,
            upgrade: false,
        }
    }

    /// Read the next bytes of the connection, `read` being every byte read so far.
    fn feed(&mut self, read: &[u8]) -> Progress {
        while let Some(&byte) = read.get(self.at) {
            let step = loop {
                match self.step(byte, read) {
                    Step::Again => {}
                    step => break step,
                }
            };
            self.at += 1;
            match step {
                Step::Next | Step::Again => {}
                Step::Ended => return Progress::Ended,
                Step::Refused => return Progress::Refused,
            }
            if self.counted >= MAX_HEAD || self.at >= MAX_READ {
                return Progress::Refused;
            }
        }
        Progress::More
    }

    /// Move to `state`, counting the byte when Node counts it there.
    fn to(&mut self, state: State, counted: bool) -> Step {
        self.state = state;
        self.counted += usize::from(counted);
        Step::Next
    }

    fn step(&mut self, byte: u8, read: &[u8]) -> Step {
        use State as S;
        match self.state {
            S::Start if matches!(byte, b'\r' | b'\n') => Step::Next,
            S::Start => self.again(S::Method),
            S::Method if byte == b' ' => {
                match METHODS.iter().position(|m| m.as_bytes() == self.method) {
                    Some(index) => {
                        self.known = Some(index);
                        self.to(S::SpacesBeforeTarget, false)
                    }
                    None => Step::Refused,
                }
            }
            S::Method => {
                self.method.push(byte);
                let prefix = |m: &&str| m.as_bytes().starts_with(&self.method);
                match METHODS.iter().any(prefix) {
                    true => Step::Next,
                    false => Step::Refused,
                }
            }
            S::SpacesBeforeTarget if byte == b' ' => Step::Next,
            S::SpacesBeforeTarget => {
                self.target = self.at..self.at;
                match self.method.as_slice() {
                    b"CONNECT" => self.again(S::Server { at: false }),
                    _ => self.again(S::TargetStart),
                }
            }
            S::TargetStart => match byte {
                b'/' | b'*' => self.again(S::Path),
                _ if byte.is_ascii_alphabetic() => self.again(S::Schema),
                _ => Step::Refused,
            },
            S::Schema if byte.is_ascii_alphabetic() => self.target_byte(S::Schema),
            S::Schema if byte == b':' => self.target_byte(S::SchemaSlashes(0)),
            S::SchemaSlashes(n) if byte == b'/' && n < 2 => match n {
                0 => self.target_byte(S::SchemaSlashes(1)),
                _ => self.target_byte(S::Server { at: false }),
            },
            S::Server { at } => match byte {
                b'/' => self.again(S::Path),
                b'?' => self.target_byte(S::Query),
                // Two `@` in a row.
                b'@' if at => Step::Refused,
                b'@' => self.target_byte(S::Server { at: true }),
                _ if is_server_byte(byte) => self.target_byte(S::Server { at: false }),
                _ => self.target_end(byte),
            },
            S::Path if is_url_byte(byte) => self.target_byte(S::Path),
            S::Query if is_url_byte(byte) || byte == b'?' => self.target_byte(S::Query),
            S::Fragment if is_url_byte(byte) || matches!(byte, b'?' | b'#') => {
                self.target_byte(S::Fragment)
            }
            S::Path | S::Query if byte == b'#' => {
                self.fragment = Some(self.at);
                self.target_byte(S::Fragment)
            }
            S::Path if byte == b'?' => self.target_byte(S::Query),
            S::Path | S::Query | S::Fragment => self.target_end(byte),
            S::TargetCr if byte == b'\n' => self.to(S::FieldStart, false),
            S::ProtocolStart if byte == b' ' => Step::Next,
            S::ProtocolStart => {
                let named = Protocol::NAMES
                    .iter()
                    .find(|(name, _)| name.first() == Some(&byte));
                match named {
                    Some(&(_, protocol)) => self.to(S::Protocol(protocol, 1), false),
                    None => Step::Refused,
                }
            }
            S::Protocol(protocol, matched) => {
                let name = Protocol::NAMES.iter().find(|(_, p)| *p == protocol);
                let name = name.map(|(name, _)| *name).unwrap_or_default();
                let allowed = self.known.is_some_and(|index| protocol.allows(index));
                match (name.get(matched) == Some(&byte), matched + 1 == name.len()) {
                    (false, _) => Step::Refused,
                    (true, false) => self.to(S::Protocol(protocol, matched + 1), false),
                    (true, true) if allowed => self.to(S::Major, false),
                    (true, true) => Step::Refused,
                }
            }
            S::Major if byte.is_ascii_digit() => {
                self.version.0 = byte - b'0';
                self.to(S::Dot, false)
            }
            S::Dot if byte == b'.' => self.to(S::Minor, false),
            S::Minor if byte.is_ascii_digit() => {
                self.version.1 = byte - b'0';
                let valid = matches!(self.version, (0, 9) | (1, 0) | (1, 1) | (2, 0));
                match (valid, self.method.as_slice()) {
                    (false, _) => Step::Refused,
                    (true, b"PRI") => self.to(S::Preface(0), false),
                    (true, _) => self.to(S::LineEnd, false),
                }
            }
            S::Preface(matched) if PREFACE.get(matched) == Some(&byte) => {
                match matched + 1 == PREFACE.len() {
                    true => Step::Refused,
                    false => self.to(S::Preface(matched + 1), false),
                }
            }
            S::LineEnd if byte == b'\r' => self.to(S::LineCr, false),
            S::LineEnd | S::LineCr if byte == b'\n' => self.to(S::FieldStart, false),
            S::FieldStart => match byte {
                b'\r' => self.to(S::HeadCr, false),
                b'\n' => self.ended(),
                _ => {
                    self.name = self.at..self.at;
                    self.field = Field::General;
                    self.again(S::Name)
                }
            },
            S::HeadCr => self.ended(),
            S::Name => self.name_byte(byte, read),
            S::SpaceBeforeValue => match byte {
                b' ' | b'\t' => Step::Next,
                b'\r' => self.to(S::EmptyValueCr, false),
                b'\n' => Step::Refused,
                _ => self.value_start(),
            },
            S::EmptyValueCr if byte == b'\n' => self.to(S::EmptyValueEnd, false),
            S::EmptyValueEnd | S::ValueEnd if matches!(byte, b' ' | b'\t') => Step::Refused,
            S::EmptyValueEnd if self.field == Field::ContentLength => Step::Refused,
            S::EmptyValueEnd => {
                self.value = self.at..self.at;
                self.field_end(read)
            }
            S::Value if is_value_byte(byte) => self.value_byte(S::Value),
            S::Value | S::ChunkedLast | S::DigitsSpace if byte == b'\r' => {
                self.value.end = self.at;
                match self.field {
                    Field::ContentLength => self.has_length = true,
                    Field::TransferEncoding => self.chunked = self.state == S::ChunkedLast,
                    Field::General | Field::Connection | Field::Upgrade => {}
                }
                self.to(S::ValueCr, false)
            }
            S::ValueCr if byte == b'\n' => self.to(S::ValueEnd, false),
            S::ValueEnd => self.field_end(read),
            S::Digits if byte.is_ascii_digit() => {
                let digit = u64::from(byte - b'0');
                let length = self.content_length.checked_mul(10);
                match length.and_then(|length| length.checked_add(digit)) {
                    Some(length) => {
                        self.content_length = length;
                        self.value_byte(S::Digits)
                    }
                    None => Step::Refused,
                }
            }
            S::Digits => self.again(S::DigitsSpace),
            S::DigitsSpace if byte == b' ' => self.value_byte(S::DigitsSpace),
            S::Chunked(matched) if b"chunked".get(matched) == Some(&byte.to_ascii_lowercase()) => {
                match matched + 1 {
                    7 => self.value_byte(S::ChunkedLast),
                    next => self.value_byte(S::Chunked(next)),
                }
            }
            S::Chunked(_) => self.again(S::CodingToken),
            S::ChunkedLast => match byte {
                b' ' => self.value_byte(S::ChunkedLast),
                // In a request, no coding may follow `chunked`.
                b',' => Step::Refused,
                _ => self.again(S::CodingToken),
            },
            S::CodingToken => match byte {
                b',' => self.value_byte(S::CodingSpace),
                _ if is_value_byte(byte) => self.value_byte(S::CodingToken),
                _ => self.again(S::Value),
            },
            S::CodingSpace if matches!(byte, b' ' | b'\t') => self.value_byte(S::CodingSpace),
            S::CodingSpace => self.again(S::Chunked(0)),
            _ => Step::Refused,
        }
    }

    fn again(&mut self, state: State) -> Step {
        self.state = state;
        Step::Again
    }

    /// A byte of the target, which Node counts.
    fn target_byte(&mut self, state: State) -> Step {
        self.target.end = self.at + 1;
        self.to(state, true)
    }

    /// The byte after the target: a space before the protocol, or the end of an HTTP/0.9 line.
    fn target_end(&mut self, byte: u8) -> Step {
        match byte {
            b' ' => self.to(State::ProtocolStart, false),
            b'\r' => self.to(State::TargetCr, false),
            b'\n' => self.to(State::FieldStart, false),
            _ => Step::Refused,
        }
    }

    fn name_byte(&mut self, byte: u8, read: &[u8]) -> Step {
        let name = read.get(self.name.clone()).unwrap_or_default();
        let special = SPECIAL_FIELDS
            .iter()
            .find(|(n, _)| name.eq_ignore_ascii_case(n));
        match byte {
            // No space may come between one of these names and the colon.
            b' ' if special.is_some() => Step::Refused,
            b':' => {
                self.field = special.map_or(Field::General, |(_, field)| *field);
                self.to(State::SpaceBeforeValue, false)
            }
            _ if is_token(byte) => {
                self.name.end = self.at + 1;
                self.to(State::Name, true)
            }
            _ => Step::Refused,
        }
    }

    /// The first byte of a value.
    fn value_start(&mut self) -> Step {
        self.value = self.at..self.at;
        match self.field {
            Field::ContentLength if self.has_length => Step::Refused,
            Field::ContentLength => {
                self.content_length = 0;
                self.again(State::Digits)
            }
            // In a request, no coding may follow `chunked`, in this field or another.
            Field::TransferEncoding if self.chunked => Step::Refused,
            Field::TransferEncoding => {
                self.has_coding = true;
                self.chunked = false;
                self.again(State::Chunked(0))
            }
            Field::General | Field::Connection | Field::Upgrade => self.again(State::Value),
        }
    }

    /// A byte of a value, which Node counts.
    fn value_byte(&mut self, state: State) -> Step {
        self.value.end = self.at + 1;
        self.to(state, true)
    }

    /// The field read ends: the byte being read starts the next line.
    fn field_end(&mut self, read: &[u8]) -> Step {
        let value = read.get(self.value.clone()).unwrap_or_default();
        match self.field {
            Field::Connection => self.connection |= connection_flags(value),
            Field::Upgrade => self.upgrade |= !value.is_empty(),
            Field::General | Field::ContentLength | Field::TransferEncoding => {}
        }
        let framing = matches!(self.field, Field::ContentLength | Field::TransferEncoding);
        if self.fields < MAX_FIELDS || framing {
            self.kept.push((self.name.clone(), self.value.clone()));
        }
        self.fields += 1;
        self.again(State::FieldStart)
    }

    /// The head ended with the byte being read.
    fn ended(&self) -> Step {
        match self.has_coding && self.has_length {
            true => Step::Refused,
            false => Step::Ended,
        }
    }

    /// What Node's server makes of what the client sent after this request's head, as far as it
    /// was read with it, while hapi serves the request: its parser fails on a coding without
    /// `chunked` last, which leaves the body's length unknown, or on bytes after the request (and
    /// its body, when they were read too) that start no request it takes; a request after it on
    /// which it closes the connection (a `CONNECT`, or a method its server does not know) ends
    /// the connection before the answer. llhttp reads on after a request only when the
    /// connection is kept alive, and not after an upgrade. A chunked body is not read.
    fn after(&self, read: &[u8]) -> After {
        if self.upgrade && self.connection & UPGRADE != 0 {
            return After::Nothing;
        }
        if self.has_coding && !self.chunked {
            return After::Failed(ClientError::Head);
        }
        let kept_alive = match self.version.0 > 0 && self.version.1 > 0 {
            true => self.connection & CLOSE == 0,
            false => self.connection & KEEP_ALIVE != 0,
        };
        let body = usize::try_from(self.content_length).ok();
        let body = body.filter(|_| kept_alive && !self.chunked);
        let rest = read.get(self.at..).unwrap_or_default();
        let Some(after) = body.and_then(|body| rest.get(body..)) else {
            return After::Nothing;
        };
        let mut next = Head::new();
        match (next.feed(after), body) {
            (Progress::Refused, Some(0)) => After::Failed(ClientError::Head),
            (Progress::Refused, _) => After::Failed(ClientError::AfterBody),
            (Progress::Ended, Some(0)) if next.closes() => After::Closed,
            _ => After::Nothing,
        }
    }

    /// Whether Node's server closes the connection on this request, whose head ended: a
    /// `CONNECT`, or a method it does not know.
    fn closes(&self) -> bool {
        let known = self.known.filter(|index| *index < KNOWN_METHODS);
        known.is_none_or(|index| METHODS.get(index) == Some(&"CONNECT"))
    }

    /// What Node's server does with the head that ended: serve it, or close the connection.
    fn outcome(&self, read: &[u8]) -> Option<Served> {
        if self.closes() {
            return None;
        }
        let client_error = match self.after(read) {
            After::Closed => return None,
            After::Failed(error) => Some(error),
            After::Nothing => None,
        };
        let mut head = self.method.clone();
        head.push(b' ');
        let target_end = self.fragment.unwrap_or(self.target.end);
        let target = read.get(self.target.start..target_end).unwrap_or_default();
        let invalid_url = target.first() == Some(&b'*');
        match invalid_url {
            true => head.push(b'/'),
            false => encode_target(target, &mut head),
        }
        head.extend_from_slice(b" HTTP/1.1\r\n");
        // The request has no body for hyper when it is refused, or when llhttp reads none.
        let unframed = client_error.is_some() || self.has_coding && !self.chunked;
        let mut fields = 0;
        for (name, value) in &self.kept {
            let name = read.get(name.clone()).unwrap_or_default();
            let expect = name.eq_ignore_ascii_case(b"expect") && self.version != (1, 1);
            let coding = name.eq_ignore_ascii_case(b"transfer-encoding");
            let framing = coding || name.eq_ignore_ascii_case(b"content-length");
            // An empty `Transfer-Encoding` names no coding for Node's parser; hyper refuses it.
            let no_coding = coding && value.is_empty();
            if name.is_empty() || name.contains(&b' ') || expect || no_coding || unframed && framing
            {
                continue;
            }
            head.extend_from_slice(name);
            head.extend_from_slice(b": ");
            head.extend_from_slice(read.get(value.clone()).unwrap_or_default());
            head.extend_from_slice(b"\r\n");
            fields += 1;
        }
        head.extend_from_slice(b"\r\n");
        if !unframed {
            head.extend_from_slice(read.get(self.at..).unwrap_or_default());
        }
        let path = target.first() == Some(&b'/');
        let node = NodeRead {
            url: path.then(|| latin1(target)),
            invalid_url,
            client_error,
        };
        Some(Served {
            bytes: head,
            fields,
            node,
        })
    }
}

/// `target` for hyper, which takes UTF-8 in a target: a byte beyond ASCII as the UTF-8 of the
/// Latin-1 character Node reads it as (two bytes), and each other byte hyper refuses
/// percent-encoded (three). Node's limit keeps the target within hyper's own (65,534 bytes).
fn encode_target(target: &[u8], out: &mut Vec<u8>) {
    for &byte in target {
        match byte {
            _ if hyper_takes(byte) => out.push(byte),
            0x80.. => {
                let mut utf8 = [0; 2];
                out.extend_from_slice(char::from(byte).encode_utf8(&mut utf8).as_bytes());
            }
            _ => out.extend_from_slice(format!("%{byte:02X}").as_bytes()),
        }
    }
}

/// `bytes` as Latin-1 characters, as Node reads a target.
fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&byte| char::from(byte)).collect()
}

/// A head Node's server serves, as hyper is to read it.
#[derive(Debug)]
pub(super) struct Served {
    /// The head written again, then the bytes read after it.
    pub(super) bytes: Vec<u8>,
    /// Its header fields.
    pub(super) fields: usize,
    /// What Node's server made of it that hyper's reading does not show.
    pub(super) node: NodeRead,
}

/// Read `stream` to the end of its request's head as Node's parser does: the head as hyper is to
/// read it, or `None` when Node's parser refuses it (it is then answered as hapi answers it) or
/// Node's server closes the connection without an answer.
pub(super) async fn read(stream: &mut TcpStream) -> io::Result<Option<Served>> {
    let mut head = Head::new();
    let mut read = Vec::new();
    let mut chunk = [0; 4096];
    loop {
        let length = stream.read(&mut chunk).await?;
        let bytes = chunk.get(..length).unwrap_or_default();
        if bytes.is_empty() {
            return Ok(None);
        }
        read.extend_from_slice(bytes);
        match head.feed(&read) {
            Progress::More => {}
            Progress::Ended => return Ok(head.outcome(&read)),
            Progress::Refused => break,
        }
    }
    // hapi writes its answer and ends its side; what the client still sends is read and dropped,
    // so that closing the socket does not reset the connection before the answer is read.
    stream.write_all(BAD_REQUEST).await?;
    stream.shutdown().await?;
    while stream.read(&mut chunk).await? > 0 {}
    Ok(None)
}

/// A connection whose first bytes were read already: they are read again first.
#[derive(Debug)]
pub(super) struct Rewind {
    read: Vec<u8>,
    at: usize,
    stream: TcpStream,
}

impl Rewind {
    /// `stream`, of which `read` was read already.
    pub(super) fn new(read: Vec<u8>, stream: TcpStream) -> Rewind {
        Rewind {
            read,
            at: 0,
            stream,
        }
    }
}

impl AsyncRead for Rewind {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let rest = this.read.get(this.at..).unwrap_or_default();
        if rest.is_empty() || buf.remaining() == 0 {
            return Pin::new(&mut this.stream).poll_read(cx, buf);
        }
        let length = rest.len().min(buf.remaining());
        buf.put_slice(rest.get(..length).unwrap_or_default());
        this.at += length;
        if this.at == this.read.len() {
            this.read = Vec::new();
            this.at = 0;
        }
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for Rewind {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().stream).poll_write(cx, bytes)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffers: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().stream).poll_write_vectored(cx, buffers)
    }

    fn is_write_vectored(&self) -> bool {
        self.stream.is_write_vectored()
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_shutdown(cx)
    }
}

/// What Node's server makes of a request's head that has ended: the head fuzz target's entry,
/// built only for the fuzz targets (`cargo fuzz` sets `fuzzing`) and the tests, never in a
/// release build.
#[cfg(any(test, fuzzing))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeadVerdict {
    /// Node's parser refuses it: hapi's bare 400.
    Refused,
    /// It ended: served (the head as hyper is to read it, then the bytes after it), or `None` when
    /// Node's server closes the connection without an answer.
    Ended(Option<Vec<u8>>),
}

/// The verdict on `bytes` read `step` bytes at a time (one when `step` is 0, all at once when it
/// is at least their length), as the server reads a connection: `None` while the head has not
/// ended.
#[cfg(any(test, fuzzing))]
pub fn head_verdict(bytes: &[u8], step: usize) -> Option<HeadVerdict> {
    let (mut head, all) = (Head::new(), bytes.len());
    let ends = (step.max(1)..all).step_by(step.max(1)).chain([all]);
    let progress = ends
        .map(|end| head.feed(bytes.get(..end).unwrap_or_default()))
        .find(|progress| *progress != Progress::More)?;
    Some(match progress {
        Progress::Ended => HeadVerdict::Ended(head.outcome(bytes).map(|served| served.bytes)),
        _ => HeadVerdict::Refused,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What Node's server makes of `head`, fed at once, checked against the same bytes fed one by
    /// one: the head hyper is given, or `Err` with the progress where there is none.
    fn verdict(head: &[u8]) -> Result<Vec<u8>, Progress> {
        let mut whole = Head::new();
        let progress = whole.feed(head);
        let mut bytewise = Head::new();
        let mut last = Progress::More;
        for length in 1..=head.len() {
            last = bytewise.feed(&head[..length]);
            if last != Progress::More {
                break;
            }
        }
        let shown = String::from_utf8_lossy(&head[..40.min(head.len())]).into_owned();
        assert_eq!(progress, last, "{shown}");
        match progress {
            Progress::Ended => whole
                .outcome(head)
                .map(|served| served.bytes)
                .ok_or(progress),
            _ => Err(progress),
        }
    }

    #[test]
    fn the_fuzz_entry_reads_a_head_as_the_server_does() {
        let heads: [&[u8]; 5] = [
            b"GET /api/blocks HTTP/1.1\r\nHost: a\r\n\r\n",
            b"GET /api/blocks HTTP/1.1\r\nHost: a\r\n",
            b"GET /a b HTTP/1.1\r\n\r\n",
            b"CONNECT a:1 HTTP/1.1\r\n\r\n",
            b"",
        ];
        for head in heads {
            let whole = head_verdict(head, usize::MAX);
            for step in [0, 1, 2, 7, head.len()] {
                assert_eq!(head_verdict(head, step), whole, "{step}");
            }
            let expected = match verdict(head) {
                Ok(bytes) => Some(HeadVerdict::Ended(Some(bytes))),
                Err(Progress::More) => None,
                Err(Progress::Refused) => Some(HeadVerdict::Refused),
                Err(Progress::Ended) => Some(HeadVerdict::Ended(None)),
            };
            assert_eq!(whole, expected);
        }
        assert!(matches!(
            head_verdict(heads[0], 1),
            Some(HeadVerdict::Ended(Some(_)))
        ));
        assert_eq!(head_verdict(heads[1], 1), None);
        assert_eq!(head_verdict(heads[3], 3), Some(HeadVerdict::Ended(None)));
    }

    #[test]
    fn heads_are_refused_where_node_refuses_them() {
        // Each shape with `@` repeated: the first count that Node 18 refuses, under hapi. Line ends
        // before the request line are skipped, uncounted.
        let shapes = [
            ("GET /@ HTTP/1.1\r\n\r\n", "q", 16_383),
            ("\r\nGET /@ HTTP/1.1\r\n\r\n", "q", 16_383),
            ("\nGET /@ HTTP/1.1\r\n\r\n", "q", 16_383),
            ("\r\n\r\nGET /@ HTTP/1.1\r\n\r\n", "q", 16_383),
            ("\n\nGET / HTTP/1.1\r\nx: @\r\n\r\n", "v", 16_382),
            ("GET /@ HTTP/1.1\r\nhost: ab\r\n\r\n", "q", 16_377),
            ("GET / HTTP/1.1\r\nx: @\r\n\r\n", "v", 16_382),
            ("GET / HTTP/1.1\r\nx:   @   \r\n\r\n", "v", 16_379),
            ("GET / HTTP/1.1\r\n@\r\n", "ab: c\r\n", 5_461),
            (
                "POST /@ HTTP/1.1\r\ncontent-length: 5\r\n\r\nhello",
                "q",
                16_368,
            ),
        ];
        for (shape, fill, refused) in shapes {
            let head = |count: usize| shape.replace('@', &fill.repeat(count));
            assert!(verdict(head(refused - 1).as_bytes()).is_ok(), "{shape}");
            let over = verdict(head(refused).as_bytes());
            assert_eq!(over, Err(Progress::Refused), "{shape}");
        }
        assert_eq!(verdict(b"GET / HTTP/1.1\r\nhost: a"), Err(Progress::More));
        assert_eq!(verdict(&b"\r\n".repeat(MAX_READ)), Err(Progress::Refused));
        assert_eq!(verdict(&b" ".repeat(MAX_READ)), Err(Progress::Refused));
    }

    #[test]
    fn heads_are_read_as_node_reads_them() {
        // Measured with Node 18.20.8 and hapi 20.2.1: a bare 400 for each of these.
        let refused: [&[u8]; 43] = [
            b"FOO / HTTP/1.1\r\n\r\n",
            b"get / HTTP/1.1\r\n\r\n",
            b"QUERY / HTTP/1.1\r\n\r\n",
            b"GETX / HTTP/1.1\r\n\r\n",
            b"GE / HTTP/1.1\r\n\r\n",
            b" GET / HTTP/1.1\r\n\r\n",
            b"GET\t/ HTTP/1.1\r\n\r\n",
            b"DESCRIBE / HTTP/1.1\r\n\r\n",
            b"GET x:80 HTTP/1.1\r\n\r\n",
            b"GET api HTTP/1.1\r\n\r\n",
            b"GET /a\x01 HTTP/1.1\r\n\r\n",
            b"GET /a\x7f HTTP/1.1\r\n\r\n",
            b"GET http://a@@b/ HTTP/1.1\r\n\r\n",
            b"GET / HTTP/1.2\r\n\r\n",
            b"GET / HTTP/3.0\r\n\r\n",
            b"GET / HTTP/1.10\r\n\r\n",
            b"GET / http/1.1\r\n\r\n",
            b"GET / HTTP/1.1 \r\n\r\n",
            b"GET / HTTP/1.1\rx: y\r\r",
            b"GET / HTTP/1.1\nx: y\n\n",
            b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n",
            b"PRI * HTTP/2.0\r\n\r\nSX",
            b"GET / HTTP/1.1\r\nx\ty: z\r\n\r\n",
            b"GET / HTTP/1.1\r\nx\"y: z\r\n\r\n",
            b"GET / HTTP/1.1\r\nx@y: z\r\n\r\n",
            b"GET / HTTP/1.1\r\nx\xe9: z\r\n\r\n",
            b"GET / HTTP/1.1\r\nxyz\r\n\r\n",
            b"GET / HTTP/1.1\r\nupgrade x: y\r\n\r\n",
            b"GET / HTTP/1.1\r\nContent-Length : 0\r\n\r\n",
            b"GET / HTTP/1.1\r\nx: a\x01b\r\n\r\n",
            b"GET / HTTP/1.1\r\nx: a\x7fb\r\n\r\n",
            b"GET / HTTP/1.1\r\nx: a\x00b\r\n\r\n",
            b"GET / HTTP/1.1\r\nx: a\r\n b\r\n\r\n",
            b"GET / HTTP/1.1\r\nx:\n\r\n",
            b"POST / HTTP/1.1\r\ncontent-length: 2\r\ncontent-length: 2\r\n\r\n{}",
            b"POST / HTTP/1.1\r\ncontent-length: 2, 2\r\n\r\n{}",
            b"POST / HTTP/1.1\r\ncontent-length: +2\r\n\r\n{}",
            b"POST / HTTP/1.1\r\ncontent-length:\r\n\r\n",
            b"POST / HTTP/1.1\r\ncontent-length: 99999999999999999999\r\n\r\n",
            b"POST / HTTP/1.1\r\ncontent-length: 2\r\ntransfer-encoding: chunked\r\n\r\n",
            b"POST / HTTP/1.1\r\ntransfer-encoding: chunked\r\ntransfer-encoding: chunked\r\n\r\n",
            b"POST / HTTP/1.1\r\ntransfer-encoding: chunked, gzip\r\n\r\n",
            b"POST / HTTP/1.1\r\ntransfer-encoding: chunked\n\r\n",
        ];
        for head in refused {
            let shown = String::from_utf8_lossy(head).into_owned();
            assert_eq!(verdict(head), Err(Progress::Refused), "{shown:?}");
        }
        // Node's server closes these without an answer: a CONNECT; a method its server does not
        // know, on which the reference's process stops; the stream's end before the head's.
        let closed: [&[u8]; 4] = [
            b"CONNECT a:1 HTTP/1.1\r\n\r\n",
            b"DESCRIBE / RTSP/1.0\r\n\r\n",
            b"PRI /\r\n\r\n",
            b"FLUSH /x\n\n",
        ];
        for head in closed {
            let shown = String::from_utf8_lossy(head).into_owned();
            assert_eq!(verdict(head), Err(Progress::Ended), "{shown:?}");
        }
        // Served, and written again as hyper reads it. An empty `Transfer-Encoding` names no
        // coding for Node's parser, and hyper refuses it: it is left out.
        let served: [(&[u8], &[u8]); 18] = [
            (b"GET / HTTP/1.1\r\n\r\nGE", b"GET / HTTP/1.1\r\n\r\nGE"),
            (
                b"\r\r\n\nGET  /a?b=\"<x>\"`|\t\x0c#f\"g HTTP/1.1\r\n\r\n",
                b"GET /a?b=%22%3Cx%3E%22%60|%09%0C HTTP/1.1\r\n\r\n",
            ),
            (
                b"GET /\xff\xc3\xa9?q=\xe9 HTTP/1.1\r\n\r\n",
                b"GET /\xc3\xbf\xc3\x83\xc2\xa9?q=\xc3\xa9 HTTP/1.1\r\n\r\n",
            ),
            (b"GET /x   HTTP/2.0\n\n", b"GET /x HTTP/1.1\r\n\r\n"),
            (b"GET /x\r\nOrigin: o\r\n\r\n", b"GET /x HTTP/1.1\r\nOrigin: o\r\n\r\n"),
            (b"GET /x\n\rX", b"GET /x HTTP/1.1\r\n\r\n"),
            (b"SOURCE /x ICE/1.0\r\n\r\n", b"SOURCE /x HTTP/1.1\r\n\r\n"),
            (b"GET /x RTSP/1.0\r\n\r\n", b"GET /x HTTP/1.1\r\n\r\n"),
            (b"PROPFIND /x HTTP/0.9\r\n\r\n", b"PROPFIND /x HTTP/1.1\r\n\r\n"),
            (
                b"GET /x HTTP/1.1\r\nx y: z\r\n: w\r\nxy : v\r\n:\r\nOrigin:\t o \t\r\n\r\n",
                b"GET /x HTTP/1.1\r\nOrigin: o \t\r\n\r\n",
            ),
            (
                b"POST /x HTTP/1.0\r\nExpect: 100-continue\r\ncontent-length: 02 \r\n\r\n{}",
                b"POST /x HTTP/1.1\r\ncontent-length: 02 \r\n\r\n{}",
            ),
            (
                b"POST /x HTTP/1.1\r\nExpect: 100-continue\r\n\r\n",
                b"POST /x HTTP/1.1\r\nExpect: 100-continue\r\n\r\n",
            ),
            (
                b"POST /x HTTP/1.1\r\ntransfer-encoding: gzip\r\ntransfer-encoding:  x , Chunked \r\n\r\n",
                b"POST /x HTTP/1.1\r\ntransfer-encoding: gzip\r\ntransfer-encoding: x , Chunked \r\n\r\n",
            ),
            (
                b"POST /x HTTP/1.1\r\nTransfer-Encoding:  \r\ncontent-length: 2\r\n\r\n{}",
                b"POST /x HTTP/1.1\r\ncontent-length: 2\r\n\r\n{}",
            ),
            (
                b"POST /x HTTP/1.1\r\ntransfer-encoding: chunked\r\ntransfer-encoding:\r\n\r\n",
                b"POST /x HTTP/1.1\r\ntransfer-encoding: chunked\r\n\r\n",
            ),
            // A coding without `chunked` last: no body, and hapi's 400.
            (
                b"POST /x HTTP/1.1\r\ntransfer-encoding: chunkedx\r\ncontent-lengthx: 1\r\n\r\n",
                b"POST /x HTTP/1.1\r\ncontent-lengthx: 1\r\n\r\n",
            ),
            (b"GET *abc?x HTTP/1.1\r\n\r\n", b"GET / HTTP/1.1\r\n\r\n"),
            (
                b"GET http://a:1/p?q HTTP/1.1\r\n\r\n",
                b"GET http://a:1/p?q HTTP/1.1\r\n\r\n",
            ),
        ];
        for (head, written) in served {
            let shown = String::from_utf8_lossy(head).into_owned();
            let expected = String::from_utf8_lossy(written).into_owned();
            let answer = verdict(head).map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
            assert_eq!(answer, Ok(expected), "{shown:?}");
        }
    }

    #[test]
    fn the_target_is_given_as_node_gives_it_to_hapi() {
        let node = |head: &[u8]| {
            let mut parsed = Head::new();
            assert_eq!(parsed.feed(head), Progress::Ended);
            parsed.outcome(head).map(|served| served.node)
        };
        let read = node(b"GET /a\"\xe9?b=%zz<#c HTTP/1.1\r\n\r\n").unwrap();
        assert_eq!(read.url.as_deref(), Some("/a\"\u{e9}?b=%zz<"));
        assert!(!read.invalid_url);
        let absolute = node(b"GET http://a/b?c HTTP/1.1\r\n\r\n").unwrap();
        assert!(absolute.url.is_none() && !absolute.invalid_url);
        let star = node(b"GET *x HTTP/1.1\r\n\r\n").unwrap();
        assert!(star.url.is_none() && star.invalid_url);
    }

    #[test]
    fn node_s_parser_fails_on_what_follows_a_request_as_node_s_does() {
        // Measured with Node 18.20.8 and hapi 20.2.1: where the parser failed on the bytes sent
        // with the request, which hapi then answers with its 400. llhttp reads on after a
        // request only when the connection is kept alive and is not upgraded.
        let (head, body) = (Some(ClientError::Head), Some(ClientError::AfterBody));
        let cases: [(&[u8], Option<ClientError>); 22] = [
            (b"GET / HTTP/1.1\r\n\r\n\x00", head),
            (b"GET / HTTP/1.1\r\n\r\n\r\n\r\n\x00", head),
            (b"GET / HTTP/1.1\r\n\r\n GET", head),
            (b"GET / HTTP/1.1\r\n\r\nG", None),
            (b"GET / HTTP/1.1\r\n\r\nGET /x HTTP/1.1\r\n\r\n\x00", None),
            (b"GET / HTTP/1.1\r\nConnection: x, close \r\n\r\n\x00", None),
            (b"GET / HTTP/1.1\r\nConnection: CLOSE,\r\n\r\n\x00", None),
            (b"GET / HTTP/1.1\r\nProxy-Connection: close\r\n\r\n\x00", None),
            (b"GET / HTTP/1.1\r\nConnection: close\t\r\n\r\n\x00", head),
            (b"GET / HTTP/1.1\r\nConnection: xclose\r\n\r\n\x00", head),
            (b"GET / HTTP/1.1\r\nConnection: clo\r\n\r\n\x00", head),
            (b"GET / HTTP/1.1\r\nUpgrade: x\r\nConnection: upgrade\r\n\r\n\x00", None),
            (b"GET / HTTP/1.0\r\n\r\n\x00", None),
            (b"GET /\r\n\r\n\x00", None),
            (b"GET / HTTP/1.0\r\nConnection: keep-alive, close\r\n\r\n\x00", head),
            (b"GET / HTTP/2.0\r\nConnection: keep-alive\r\n\r\n\x00", head),
            (b"POST / HTTP/1.1\r\ncontent-length: 2\r\n\r\n{}GARBAGE", body),
            (b"POST / HTTP/1.1\r\ncontent-length: 2\r\n\r\n{}\r\n", None),
            (b"POST / HTTP/1.1\r\ncontent-length: 5\r\n\r\n{}", None),
            (b"POST / HTTP/1.1\r\ntransfer-encoding: gzip\r\n\r\n", head),
            (b"POST / HTTP/1.1\r\ntransfer-encoding: gzip\r\nconnection: close\r\n\r\n", head),
            (
                b"GET / HTTP/1.1\r\nupgrade: x\r\nconnection: upgrade\r\ntransfer-encoding: gzip\r\n\r\n",
                None,
            ),
        ];
        for (bytes, error) in cases {
            let shown = String::from_utf8_lossy(bytes).into_owned();
            let mut parsed = Head::new();
            assert_eq!(parsed.feed(bytes), Progress::Ended, "{shown:?}");
            let served = parsed.outcome(bytes).unwrap();
            assert_eq!(served.node.client_error, error, "{shown:?}");
            // The body is not framed for hyper when the parser fails, or when it reads none.
            let text = String::from_utf8_lossy(&served.bytes).into_owned();
            let unframed = error.is_some() || shown.contains("gzip");
            let framing = ["content-length", "transfer-encoding", "GARBAGE"];
            let framed = framing.iter().any(|name| text.contains(name));
            assert!(!(unframed && framed), "{shown:?}: {text:?}");
        }
        // A request after it on which Node's server closes the connection: no answer.
        let closed: [&[u8]; 2] = [
            b"GET / HTTP/1.1\r\n\r\nCONNECT a:1 HTTP/1.1\r\n\r\n",
            b"GET / HTTP/1.1\r\n\r\nDESCRIBE / RTSP/1.0\r\n\r\n",
        ];
        for bytes in closed {
            assert_eq!(verdict(bytes), Err(Progress::Ended));
        }
    }

    #[test]
    fn node_s_server_passes_on_its_first_1000_fields_and_those_that_frame_the_body() {
        // Measured with Node 18.20.8 and hapi 20.2.1: an `Origin` that is the 1,000th field is
        // read, one that is the 1,001st is not; a body framed by a later field is read.
        let head = |origin_at: usize| {
            let mut head = b"POST / HTTP/1.1\r\n".to_vec();
            for i in 1..=1_200 {
                let field = match i == origin_at {
                    true => "origin: late".to_owned(),
                    false => format!("x{i}: v"),
                };
                head.extend_from_slice(format!("{field}\r\n").as_bytes());
            }
            head.extend_from_slice(b"content-length: 2\r\n\r\n{}");
            head
        };
        for (origin_at, read) in [(1_000, true), (1_001, false)] {
            let head = head(origin_at);
            let mut parsed = Head::new();
            assert_eq!(parsed.feed(&head), Progress::Ended);
            let served = parsed.outcome(&head).unwrap();
            assert_eq!(served.fields, 1_001);
            let text = String::from_utf8_lossy(&served.bytes).into_owned();
            assert_eq!(text.contains("origin: late"), read, "{origin_at}");
            assert!(!text.contains("x1001:"), "{origin_at}");
            assert!(
                text.ends_with("\r\ncontent-length: 2\r\n\r\n{}"),
                "{origin_at}"
            );
            assert!(served.bytes.starts_with(b"POST / HTTP/1.1\r\nx1: v\r\n"));
        }
    }
}
