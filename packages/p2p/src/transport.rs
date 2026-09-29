//! The WebSocket connection of a nes session: `ws://<ip>:<port>/`, binary frames only, no
//! compression. The client never sends a WebSocket ping.
//!
//! The upgrade request and the frame format come from tungstenite; the frames are read here, as
//! the reference's WebSocket library reads them on the client side, because the receive limit is
//! connection state that changes while the connection is open. A data frame's declared length is
//! checked against the limit in force when its header is read, before its payload is read, and
//! is added to the lengths of the earlier frames of the same message. A frame above the limit
//! fails the connection with close code 1009, a frame that breaks the protocol with 1002, and a
//! text message or close reason that is not UTF-8 with 1007.

use std::future::Future;
use std::io::{self, Cursor};
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::task::AbortHandle;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::handshake::client::{Response, generate_request};
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::handshake::machine::TryParse;
use tokio_tungstenite::tungstenite::protocol::frame::coding::{Control, Data, OpCode};
use tokio_tungstenite::tungstenite::protocol::frame::{Frame, FrameHeader};

/// The longest upgrade response read: the header limit of the reference's HTTP parser.
const MAX_RESPONSE: usize = 16 * 1024;
/// The largest length the reference reads exactly (2^53 - 1); a longer frame fails with 1009.
const MAX_LENGTH: u64 = (1 << 53) - 1;
const PROTOCOL: Received = Received::Failed(1002);
const TOO_BIG: Received = Received::Failed(1009);
const NOT_UTF8: Received = Received::Failed(1007);

/// A byte stream a connection runs on: TCP, or an in-memory pipe in tests.
pub trait Stream: AsyncRead + AsyncWrite + Send + Unpin + 'static {}

impl<T: AsyncRead + AsyncWrite + Send + Unpin + 'static> Stream for T {}

/// Opens the byte stream to an address.
pub type Dial = Arc<
    dyn Fn(SocketAddr) -> Pin<Box<dyn Future<Output = io::Result<Box<dyn Stream>>> + Send>>
        + Send
        + Sync,
>;

/// Streams over TCP, without Nagle's delay, as the reference's sockets are.
pub fn tcp() -> Dial {
    Arc::new(|address| {
        Box::pin(async move {
            let stream = TcpStream::connect(address).await?;
            stream.set_nodelay(true)?;
            Ok(Box::new(stream) as Box<dyn Stream>)
        })
    })
}

/// What the connection received.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Received {
    /// A binary message.
    Binary(Vec<u8>),
    /// A text message, which the nes client refuses.
    Text,
    /// A WebSocket ping frame, with its payload.
    Ping(Vec<u8>),
    /// A WebSocket pong frame.
    Pong,
    /// The server's close frame with its code (1005 when it had none), or `None` when the stream
    /// ended or failed.
    Closed(Option<u16>),
    /// The server broke the WebSocket protocol, or sent a message above the limit: the
    /// connection fails with this close code.
    Failed(u16),
}

/// Open the WebSocket on `stream`: the upgrade request for `ws://<address>/`, then the server's
/// response, which must accept the upgrade with no extension and no subprotocol.
pub async fn open(
    mut stream: Box<dyn Stream>,
    address: SocketAddr,
) -> io::Result<(Reader, Writer)> {
    let request = format!("ws://{address}/").into_client_request();
    let (request, key) =
        generate_request(request.map_err(io::Error::other)?).map_err(io::Error::other)?;
    stream.write_all(&request).await?;
    let mut buffer = Vec::new();
    let response = loop {
        if let Some((size, response)) = Response::try_parse(&buffer).map_err(io::Error::other)? {
            buffer.drain(..size);
            break response;
        }
        if buffer.len() > MAX_RESPONSE || stream.read_buf(&mut buffer).await? == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
    };
    let header = |name: &str| response.headers().get(name).map(|value| value.as_bytes());
    let upgraded = response.status().as_u16() == 101
        && header("Upgrade").is_some_and(|value| value.eq_ignore_ascii_case(b"websocket"))
        && header("Sec-WebSocket-Accept") == Some(derive_accept_key(key.as_bytes()).as_bytes())
        && header("Sec-WebSocket-Extensions").is_none()
        && header("Sec-WebSocket-Protocol").is_none();
    if !upgraded {
        return Err(io::Error::other("the server refused the WebSocket upgrade"));
    }
    let (half, write) = tokio::io::split(stream);
    let (frame, message, fragmented, total) = (None, Vec::new(), None, 0);
    let reader = Reader {
        half,
        buffer,
        frame,
        message,
        fragmented,
        total,
    };
    Ok((reader, Writer::spawn(write)))
}

/// The most capacity the read buffer keeps between messages: one large message grew it to twice
/// its size, which the connection then held for its life.
const KEPT_BUFFER: usize = 64 * 1024;

/// The receiving side of a connection.
pub struct Reader {
    half: ReadHalf<Box<dyn Stream>>,
    buffer: Vec<u8>,
    /// The frame whose header was read and checked, and its payload length.
    frame: Option<(FrameHeader, usize)>,
    /// The payloads of the current message's frames so far.
    message: Vec<u8>,
    /// The kind of a fragmented message in progress.
    fragmented: Option<Data>,
    /// The declared lengths of the current message's frames so far.
    total: u64,
}

impl Reader {
    /// The next message or control frame. A data frame is checked against `limit` when its header
    /// has been read. Cancel safe: what was read and checked is kept for the next call.
    pub async fn next(&mut self, limit: usize) -> Received {
        loop {
            if self.frame.is_none() {
                let mut cursor = Cursor::new(self.buffer.as_slice());
                match FrameHeader::parse(&mut cursor) {
                    Err(_) => return PROTOCOL,
                    Ok(None) => {}
                    Ok(Some((header, length))) => {
                        let used = usize::try_from(cursor.position()).unwrap_or(usize::MAX);
                        self.buffer.drain(..used.min(self.buffer.len()));
                        match self.check(&header, length, limit) {
                            Ok(length) => self.frame = Some((header, length)),
                            Err(failed) => return failed,
                        }
                    }
                }
            }
            if let Some((header, length)) = self.frame.take_if(|(_, n)| *n <= self.buffer.len()) {
                let payload: Vec<u8> = self.buffer.drain(..length).collect();
                self.buffer.shrink_to(KEPT_BUFFER);
                if let Some(received) = self.complete(&header, payload) {
                    return received;
                }
                continue;
            }
            if matches!(self.half.read_buf(&mut self.buffer).await, Ok(0) | Err(_)) {
                return Received::Closed(None);
            }
        }
    }

    /// The payload length of an acceptable frame, checked in the reference's order.
    fn check(
        &mut self,
        header: &FrameHeader,
        length: u64,
        limit: usize,
    ) -> Result<usize, Received> {
        let allowed = match header.opcode {
            OpCode::Data(Data::Continue) => self.fragmented.is_some(),
            OpCode::Data(_) => self.fragmented.is_none(),
            OpCode::Control(_) => header.is_final && length <= 125,
        };
        if !allowed || header.rsv1 || header.rsv2 || header.rsv3 || header.mask.is_some() {
            return Err(PROTOCOL);
        }
        if length > MAX_LENGTH {
            return Err(TOO_BIG);
        }
        if let OpCode::Data(data) = header.opcode {
            self.total = self.total.saturating_add(length);
            if self.total > u64::try_from(limit).unwrap_or(u64::MAX) {
                return Err(TOO_BIG);
            }
            if !header.is_final && self.fragmented.is_none() {
                self.fragmented = Some(data);
            }
        }
        usize::try_from(length).map_err(|_| TOO_BIG)
    }

    /// A frame's payload has been read: a control frame, or the end of a message.
    fn complete(&mut self, header: &FrameHeader, payload: Vec<u8>) -> Option<Received> {
        let data = match header.opcode {
            OpCode::Control(Control::Close) => return Some(close(&payload)),
            OpCode::Control(Control::Ping) => return Some(Received::Ping(payload)),
            OpCode::Control(Control::Pong) => return Some(Received::Pong),
            OpCode::Control(Control::Reserved(_)) => return Some(PROTOCOL),
            OpCode::Data(data) => data,
        };
        self.message.extend_from_slice(&payload);
        if !header.is_final {
            return None;
        }
        self.total = 0;
        let message = std::mem::take(&mut self.message);
        Some(match self.fragmented.take().unwrap_or(data) {
            Data::Binary => Received::Binary(message),
            Data::Text if std::str::from_utf8(&message).is_ok() => Received::Text,
            Data::Text => NOT_UTF8,
            Data::Continue | Data::Reserved(_) => PROTOCOL,
        })
    }
}

/// A close frame, whose code must be one a close frame may carry, with a UTF-8 reason.
fn close(payload: &[u8]) -> Received {
    let Some((code, reason)) = payload.split_first_chunk::<2>() else {
        return if payload.is_empty() {
            Received::Closed(Some(1005))
        } else {
            PROTOCOL
        };
    };
    let code = u16::from_be_bytes(*code);
    if !matches!(code, 1000..=1003 | 1007..=1014 | 3000..=4999) {
        return PROTOCOL;
    }
    match std::str::from_utf8(reason) {
        Ok(_) => Received::Closed(Some(code)),
        Err(_) => NOT_UTF8,
    }
}

/// The sending side of a connection: frames are masked and queued to a task that writes them in
/// order, so a sender never waits for the network. Dropping the writer ends the connection's
/// writes after the queued frames.
#[derive(Debug)]
pub struct Writer {
    queue: mpsc::UnboundedSender<Vec<u8>>,
    task: AbortHandle,
}

impl Writer {
    fn spawn(mut half: WriteHalf<Box<dyn Stream>>) -> Writer {
        let (queue, mut frames) = mpsc::unbounded_channel::<Vec<u8>>();
        let task = tokio::spawn(async move {
            while let Some(frame) = frames.recv().await {
                if half.write_all(&frame).await.is_err() {
                    return;
                }
            }
            let _ = half.shutdown().await;
        });
        let task = task.abort_handle();
        Writer { queue, task }
    }

    /// Send `bytes` as one binary frame.
    pub fn binary(&self, bytes: Vec<u8>) {
        self.send(OpCode::Data(Data::Binary), bytes);
    }

    /// Answer a WebSocket ping with a pong carrying its payload.
    pub fn pong(&self, payload: Vec<u8>) {
        self.send(OpCode::Control(Control::Pong), payload);
    }

    /// Send a close frame, with `code` when given, and end the connection's writes.
    pub fn close(self, code: Option<u16>) {
        let payload = code.map(|code| code.to_be_bytes().to_vec());
        self.send(OpCode::Control(Control::Close), payload.unwrap_or_default());
    }

    /// Drop the connection at once, with whatever is still queued.
    pub fn terminate(self) {
        self.task.abort();
    }

    fn send(&self, opcode: OpCode, payload: Vec<u8>) {
        let mask = Some(rand::random());
        let header = FrameHeader {
            opcode,
            mask,
            ..FrameHeader::default()
        };
        let mut frame = Vec::with_capacity(payload.len().saturating_add(14));
        let _ = Frame::from_payload(header, payload.into()).format(&mut frame);
        let _ = self.queue.send(frame);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_large_message_leaves_no_large_buffer_behind() {
        let (client, mut server) = tokio::io::duplex(64 * 1024);
        let (half, _write) = tokio::io::split(Box::new(client) as Box<dyn Stream>);
        let mut reader = Reader {
            half,
            buffer: Vec::new(),
            frame: None,
            message: Vec::new(),
            fragmented: None,
            total: 0,
        };
        let length = 4 * 1024 * 1024;
        tokio::spawn(async move {
            let mut header = vec![0x82, 127];
            header.extend_from_slice(&u64::try_from(length).unwrap().to_be_bytes());
            server.write_all(&header).await.unwrap();
            server.write_all(&vec![7; length]).await.unwrap();
            server.write_all(&[0x82, 1, 9]).await.unwrap();
            // Keep the stream open until the reader is done.
            std::future::pending::<()>().await;
        });
        assert_eq!(
            reader.next(8 * 1024 * 1024).await,
            Received::Binary(vec![7; length])
        );
        assert!(
            reader.buffer.capacity() <= KEPT_BUFFER,
            "{}",
            reader.buffer.capacity()
        );
        assert_eq!(
            reader.next(8 * 1024 * 1024).await,
            Received::Binary(vec![9])
        );
    }
}
