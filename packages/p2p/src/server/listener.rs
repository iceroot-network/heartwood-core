//! A connection's HTTP exchange and its WebSocket frames, as the reference's HTTP server and its
//! WebSocket library (`ws` 7) serve them.
//!
//! - The request head is read with the HTTP socket timeout of 2 s between reads; a head still
//!   unfinished 60 s after the connection opened gets a bare `400 Bad Request` and is ended (the
//!   server's `headersTimeout`, which the reference checks every 30 s, so it cuts at 60 to 90 s,
//!   answered by the HTTP framework's client error handler rather than Node's 408). A request
//!   that is no upgrade (no `Upgrade` header with a `Connection: upgrade` token) gets 404, which
//!   bans the caller "Not a websocket connection". An upgrade to any path but `/` is destroyed; one
//!   the WebSocket library refuses (not GET, an `Upgrade` header other than `websocket` alone, a
//!   malformed key, a version other than 8 or 13) gets 400 and is destroyed. Otherwise the
//!   WebSocket opens (echoing the first requested subprotocol, as the library does).
//! - Frames are read as the library's server reads them, messages of at most 20 MiB: each fault
//!   ends the connection with the library's error text, which the nes socket takes as its ban
//!   reason ("WebSocket" written "websocket" and ":" as " -"); an invalid UTF-8 text message or
//!   close reason ends it without a reason.
//! - Frames are written unmasked, in order, by a task of their own, which tells the session how
//!   many bytes it has written, for the heartbeat's liveness check. A socket whose unwritten bytes
//!   pass [`MAX_BACKLOG`] is dropped by its session, and the writes go on after the session ends
//!   for at most [`DRAIN_TIMEOUT`], where the reference buffers without a bound.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::sync::mpsc;
use tokio::task::AbortHandle;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;

use crate::transport::Stream;

/// The largest message a socket takes: 20 MiB.
pub const MAX_MESSAGE: usize = 20 * 1024 * 1024;
/// The HTTP socket timeout before the upgrade.
pub const SOCKET_TIMEOUT: Duration = Duration::from_secs(2);
/// How long the whole request head may take: the HTTP server's `headersTimeout`.
pub const HEAD_TIMEOUT: Duration = Duration::from_secs(60);
/// The longest request head read: the HTTP parser's header limit.
const MAX_HEAD: usize = 16 * 1024;
/// The largest frame length the library reads (2^53 - 1).
const MAX_LENGTH: u64 = (1 << 53) - 1;
/// The most bytes a socket may have queued and not yet written: twice the largest message.
pub const MAX_BACKLOG: u64 = 2 * MAX_MESSAGE as u64;
/// How long the writes may go on once the session has ended: the WebSocket library's close
/// timeout, after which it destroys the socket.
pub const DRAIN_TIMEOUT: Duration = Duration::from_secs(30);

/// How the HTTP exchange before the WebSocket ended.
pub enum Upgrade {
    /// The WebSocket opened.
    Open(FrameReader, FrameWriter),
    /// A request that is no upgrade, answered 404.
    NotWebSocket,
    /// The socket closed, went quiet, or its upgrade was refused.
    Failed,
}

/// Read the request head from `stream` and answer it.
pub async fn upgrade(mut stream: Box<dyn Stream>) -> Upgrade {
    let mut buffer = Vec::new();
    let head = timeout(HEAD_TIMEOUT, async {
        loop {
            if let Some(at) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
                return Some(at);
            }
            let read = timeout(SOCKET_TIMEOUT, stream.read_buf(&mut buffer)).await;
            if buffer.len() > MAX_HEAD || !matches!(read, Ok(Ok(1..))) {
                return None;
            }
        }
    })
    .await;
    let end = match head {
        Ok(Some(end)) => end,
        Ok(None) => return Upgrade::Failed,
        Err(_) => {
            let answer = "HTTP/1.1 400 Bad Request\r\n\r\n";
            let _ = timeout(SOCKET_TIMEOUT, stream.write_all(answer.as_bytes())).await;
            return Upgrade::Failed;
        }
    };
    let rest = buffer.split_off(end.saturating_add(4));
    buffer.truncate(end);
    let Some(head) = Head::parse(&buffer) else {
        return Upgrade::Failed;
    };
    let token = |name: &str, token: &str| {
        (head.header(name).unwrap_or_default().split(','))
            .any(|part| part.trim().eq_ignore_ascii_case(token))
    };
    if head.header("upgrade").is_none() || !token("connection", "upgrade") {
        let answer = "HTTP/1.1 404 Not Found\r\nconnection: close\r\ncontent-length: 0\r\n\r\n";
        let _ = stream.write_all(answer.as_bytes()).await;
        return Upgrade::NotWebSocket;
    }
    if head.target != "/" {
        return Upgrade::Failed;
    }
    let key = head.header("sec-websocket-key").unwrap_or_default().trim();
    let key_valid = key.len() == 24
        && key.ends_with("==")
        && (key.bytes().take(22)).all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/');
    // The library reads the version as a JavaScript number.
    let version = head
        .header("sec-websocket-version")
        .unwrap_or_default()
        .trim();
    let version = version
        .parse::<f64>()
        .ok()
        .filter(|v| *v == 8.0 || *v == 13.0);
    let websocket = (head.header("upgrade")).is_some_and(|u| u.eq_ignore_ascii_case("websocket"));
    if head.method != "GET" || !websocket || !key_valid || version.is_none() {
        let answer = "HTTP/1.1 400 Bad Request\r\nConnection: close\r\nContent-Type: text/html\r\n\
                      Content-Length: 11\r\n\r\nBad Request";
        let _ = stream.write_all(answer.as_bytes()).await;
        return Upgrade::Failed;
    }
    let mut answer = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Accept: {}\r\n",
        derive_accept_key(key.as_bytes())
    );
    let protocol = head.header("sec-websocket-protocol").unwrap_or_default();
    if let Some(first) = protocol
        .split(',')
        .map(str::trim)
        .next()
        .filter(|p| !p.is_empty())
    {
        answer.push_str(&format!("Sec-WebSocket-Protocol: {first}\r\n"));
    }
    answer.push_str("\r\n");
    if stream.write_all(answer.as_bytes()).await.is_err() {
        return Upgrade::Failed;
    }
    let (read, write) = tokio::io::split(stream);
    let reader = FrameReader {
        half: read,
        buffer: rest,
        fragmented: None,
        total: 0,
        message: Vec::new(),
    };
    Upgrade::Open(reader, FrameWriter::spawn(write))
}

/// A request head: the method, the target and the headers (names in lowercase, repeated ones
/// joined with `, `).
struct Head {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
}

impl Head {
    fn parse(bytes: &[u8]) -> Option<Head> {
        let text = std::str::from_utf8(bytes).ok()?;
        let mut lines = text.split("\r\n");
        let mut parts = lines.next()?.split(' ');
        let (method, target) = (parts.next()?.to_owned(), parts.next()?.to_owned());
        if !parts.next()?.starts_with("HTTP/1.") || parts.next().is_some() {
            return None;
        }
        let mut headers: Vec<(String, String)> = Vec::new();
        for line in lines {
            let (name, value) = line.split_once(':')?;
            let (name, value) = (name.trim().to_ascii_lowercase(), value.trim());
            match headers.iter_mut().find(|(known, _)| *known == name) {
                Some((_, joined)) => *joined = format!("{joined}, {value}"),
                None => headers.push((name, value.to_owned())),
            }
        }
        Some(Head {
            method,
            target,
            headers,
        })
    }

    fn header(&self, name: &str) -> Option<&str> {
        let found = self.headers.iter().find(|(known, _)| known == name);
        found.map(|(_, value)| value.as_str())
    }
}

/// What a WebSocket received.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    /// A binary message.
    Binary(Vec<u8>),
    /// A text message.
    Text,
    /// A ping frame.
    Ping,
    /// A pong frame.
    Pong,
    /// A close frame, with its code (1005 when it had none).
    Close(u16),
    /// The connection failed: with the reason the nes socket bans with, or without one (an
    /// invalid UTF-8 text message or close reason, which closes the WebSocket with 1007).
    Failed(Option<String>),
    /// The stream ended or failed.
    Ended,
}

/// The receiving side of a WebSocket.
pub struct FrameReader {
    half: ReadHalf<Box<dyn Stream>>,
    buffer: Vec<u8>,
    /// The opcode of a fragmented message in progress.
    fragmented: Option<u8>,
    /// The lengths of the current message's frames so far.
    total: u64,
    message: Vec<u8>,
}

/// A frame fault, with the library's text.
fn fault(text: &str) -> Frame {
    Frame::Failed(Some(format!("Invalid websocket frame - {text}")))
}

impl FrameReader {
    /// The next message or control frame. Cancel safe: a frame is taken from the buffer only
    /// once it is whole.
    pub async fn next(&mut self) -> Frame {
        loop {
            let frame = match self.frame() {
                Ok(Some(frame)) => frame,
                Ok(None) => continue,
                Err(Some(frame)) => return frame,
                Err(None) => match self.half.read_buf(&mut self.buffer).await {
                    Ok(1..) => continue,
                    _ => return Frame::Ended,
                },
            };
            return frame;
        }
    }

    /// The next frame from the buffer: a message or control frame, `None` for a fragment, or
    /// `Err(None)` when more bytes are needed.
    fn frame(&mut self) -> Result<Option<Frame>, Option<Frame>> {
        let [first, second, ..] = *self.buffer.as_slice() else {
            return Err(None);
        };
        let (fin, opcode, short) = (first & 0x80 != 0, first & 0x0f, second & 0x7f);
        let opcode = match opcode {
            _ if first & 0x30 != 0 => return Err(Some(fault("RSV2 and RSV3 must be clear"))),
            _ if first & 0x40 != 0 => return Err(Some(fault("RSV1 must be clear"))),
            0 => self
                .fragmented
                .ok_or_else(|| Some(fault("invalid opcode 0")))?,
            1 | 2 if self.fragmented.is_some() => {
                return Err(Some(fault(&format!("invalid opcode {opcode}"))));
            }
            1 | 2 => opcode,
            8..=10 if !fin => return Err(Some(fault("FIN must be set"))),
            8..=10 if short > 125 => {
                return Err(Some(fault(&format!("invalid payload length {short}"))));
            }
            8..=10 => opcode,
            _ => return Err(Some(fault(&format!("invalid opcode {opcode}")))),
        };
        if second & 0x80 == 0 {
            return Err(Some(fault("MASK must be set")));
        }
        let (length, at) = match short {
            126 => (u64::from(u16::from_be_bytes(self.take::<2>(2)?)), 4),
            127 => (u64::from_be_bytes(self.take::<8>(2)?), 10),
            short => (u64::from(short), 2),
        };
        if length > MAX_LENGTH {
            let text = "Unsupported websocket frame - payload length > 2^53 - 1";
            return Err(Some(Frame::Failed(Some(text.to_owned()))));
        }
        if opcode < 8 && self.total.saturating_add(length) > MAX_MESSAGE as u64 {
            let text = "Max payload size exceeded";
            return Err(Some(Frame::Failed(Some(text.to_owned()))));
        }
        let mask = self.take::<4>(at)?;
        let start = at.saturating_add(4);
        let end = usize::try_from(length)
            .ok()
            .and_then(|n| start.checked_add(n));
        let Some(payload) = end.and_then(|end| self.buffer.get(start..end)) else {
            return Err(None);
        };
        let payload: Vec<u8> = payload
            .iter()
            .zip(mask.iter().cycle())
            .map(|(b, m)| b ^ m)
            .collect();
        self.buffer.drain(..start.saturating_add(payload.len()));
        if opcode >= 8 {
            return Ok(Some(control(opcode, &payload)));
        }
        if !fin && self.fragmented.is_none() {
            self.fragmented = Some(opcode);
        }
        self.total = self.total.saturating_add(length);
        self.message.extend_from_slice(&payload);
        if !fin {
            return Ok(None);
        }
        let message = std::mem::take(&mut self.message);
        (self.fragmented, self.total) = (None, 0);
        Ok(Some(match opcode {
            2 => Frame::Binary(message),
            _ if std::str::from_utf8(&message).is_ok() => Frame::Text,
            _ => Frame::Failed(None),
        }))
    }

    /// `N` bytes at `at` of the buffer, or `Err(None)` when they have not come yet.
    fn take<const N: usize>(&self, at: usize) -> Result<[u8; N], Option<Frame>> {
        let end = at.saturating_add(N);
        let bytes = self.buffer.get(at..end).ok_or(None)?;
        bytes.try_into().map_err(|_| None)
    }
}

/// A control frame: a ping, a pong, or a close with a valid code and a UTF-8 reason.
fn control(opcode: u8, payload: &[u8]) -> Frame {
    match (opcode, payload) {
        (9, _) => Frame::Ping,
        (10, _) => Frame::Pong,
        (_, []) => Frame::Close(1005),
        (_, [_]) => fault("invalid payload length 1"),
        (_, [high, low, reason @ ..]) => {
            let code = u16::from_be_bytes([*high, *low]);
            let valid = matches!(code, 1000..=1003 | 1007..=1014 | 3000..=4999);
            match (valid, std::str::from_utf8(reason)) {
                (false, _) => fault(&format!("invalid status code {code}")),
                (true, Ok(_)) => Frame::Close(code),
                (true, Err(_)) => Frame::Failed(None),
            }
        }
    }
}

/// The sending side of a WebSocket: frames queued to a task that writes them in order. Dropping
/// the writer ends the writes after the queued frames, or [`DRAIN_TIMEOUT`] later.
#[derive(Debug)]
pub struct FrameWriter {
    queue: mpsc::UnboundedSender<Vec<u8>>,
    progress: Arc<Progress>,
    task: AbortHandle,
    /// The bytes written at the last heartbeat check.
    checked: AtomicU64,
}

/// How far the writes are, in bytes counted from the WebSocket's opening.
#[derive(Debug, Default)]
struct Progress {
    queued: AtomicU64,
    /// Where the last frame queued that counts as the socket's activity ends.
    activity: AtomicU64,
    /// The bytes written; `u64::MAX` once a write failed, as nothing more is written then.
    written: AtomicU64,
}

impl FrameWriter {
    fn spawn(mut half: WriteHalf<Box<dyn Stream>>) -> FrameWriter {
        let (queue, mut frames) = mpsc::unbounded_channel::<Vec<u8>>();
        let progress = Arc::new(Progress::default());
        let written = Arc::clone(&progress);
        let task = tokio::spawn(async move {
            while let Some(frame) = frames.recv().await {
                // Each write takes what the stream accepts, so that a slow reader's progress
                // shows within a large frame.
                let mut rest = frame.as_slice();
                while !rest.is_empty() {
                    let Ok(n @ 1..) = half.write(rest).await else {
                        written.written.store(u64::MAX, Ordering::Release);
                        return;
                    };
                    rest = rest.get(n..).unwrap_or_default();
                    written.written.fetch_add(n as u64, Ordering::AcqRel);
                }
            }
            let _ = half.shutdown().await;
        });
        let task = task.abort_handle();
        FrameWriter {
            queue,
            progress,
            task,
            checked: AtomicU64::new(0),
        }
    }

    /// Send `bytes` as one binary frame. A frame that is `activity` (every message but the
    /// server's ping) marks the socket active once it is written, as the reference's `_flush`
    /// does ([`FrameWriter::check`]).
    pub fn binary(&self, bytes: &[u8], activity: bool) {
        let end = self.send(2, bytes);
        if activity {
            self.progress.activity.store(end, Ordering::Release);
        }
    }

    /// The heartbeat's check: whether the writes show the socket active since the check before.
    /// They do when bytes were written since then and either some are still queued (the
    /// reference's `_sending`, of a peer that reads) or a frame that counts as activity was
    /// written to its end since then (its `_flush` marking the socket). A queue that did not move
    /// shows nothing: a peer that stops reading is not kept by what waits for it.
    pub fn check(&self) -> bool {
        let written = self.written();
        let before = self.checked.swap(written, Ordering::AcqRel);
        let (queued, activity) = (
            self.queued(),
            self.progress.activity.load(Ordering::Acquire),
        );
        written != u64::MAX && written > before && (written < queued || activity > before)
    }

    /// The bytes queued and not yet written.
    pub fn backlog(&self) -> u64 {
        self.queued().saturating_sub(self.written())
    }

    fn queued(&self) -> u64 {
        self.progress.queued.load(Ordering::Acquire)
    }

    fn written(&self) -> u64 {
        self.progress.written.load(Ordering::Acquire)
    }

    /// Send a close frame, with `code` unless it is 1005, and end the writes.
    pub fn close(self, code: u16) {
        let payload = if code == 1005 {
            Vec::new()
        } else {
            code.to_be_bytes().to_vec()
        };
        self.send(8, &payload);
    }

    /// Drop the connection at once, with whatever is still queued.
    pub fn terminate(self) {
        self.task.abort();
    }

    /// Queue a frame; where it ends.
    fn send(&self, opcode: u8, payload: &[u8]) -> u64 {
        let mut frame = vec![0x80 | opcode];
        match (u8::try_from(payload.len()), u16::try_from(payload.len())) {
            (Ok(short @ ..=125), _) => frame.push(short),
            (_, Ok(length)) => {
                frame.push(126);
                frame.extend_from_slice(&length.to_be_bytes());
            }
            _ => {
                frame.push(127);
                frame.extend_from_slice(&(payload.len() as u64).to_be_bytes());
            }
        }
        frame.extend_from_slice(payload);
        let length = frame.len() as u64;
        let start = self.progress.queued.fetch_add(length, Ordering::AcqRel);
        let _ = self.queue.send(frame);
        start.saturating_add(length)
    }
}

impl Drop for FrameWriter {
    /// The writes still queued get [`DRAIN_TIMEOUT`], as the library destroys a socket that
    /// closed: a peer that stops reading holds no memory after its session.
    fn drop(&mut self) {
        if self.task.is_finished() {
            return;
        }
        let task = self.task.clone();
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => drop(runtime.spawn(async move {
                tokio::time::sleep(DRAIN_TIMEOUT).await;
                task.abort();
            })),
            Err(_) => task.abort(),
        }
    }
}
