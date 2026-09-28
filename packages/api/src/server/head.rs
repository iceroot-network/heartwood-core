//! A request's head as Node's parser measures it, read before hyper serves the connection.
//!
//! Node refuses a request whose head reaches 16 KiB (its `--max-http-header-size`). It counts the
//! bytes of the target and of the header names and values: a value from its first byte that is
//! not a space or a tab, its trailing white space included; not the method, the version, the
//! colons or the line ends. hapi answers that refusal, as every refusal of Node's parser, with a
//! bare `400 Bad Request` and ends the connection. [`read`] reads a connection to its head's end,
//! counting as Node counts, and [`Rewind`] then gives hyper the bytes read, followed by the rest.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::TcpStream;

/// Node's limit on the counted bytes of a request's head.
const MAX_HEAD: usize = 16_384;

/// The most bytes read without the head ending, counted or not: hyper's default read buffer, the
/// bound the server kept before it read heads itself. A head refused only by this bound (white
/// space or line ends Node does not count) is refused the same way.
const MAX_READ: usize = 8192 + 4096 * 100;

/// hapi's answer to a request that Node's parser refuses.
const BAD_REQUEST: &[u8] = b"HTTP/1.1 400 Bad Request\r\n\r\n";

/// Where a head's reading stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Method,
    Target,
    Version,
    /// A header's name; `start` before any byte of its line.
    Name {
        start: bool,
    },
    /// The white space before a header's value.
    Space,
    Value,
}

/// What the bytes read so far make of a head.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Progress {
    More,
    Ended,
    Refused,
}

/// Node's count of a request's head, byte by byte.
#[derive(Debug)]
struct Head {
    part: Part,
    counted: usize,
    read: usize,
}

impl Head {
    fn new() -> Head {
        Head {
            part: Part::Method,
            counted: 0,
            read: 0,
        }
    }

    /// Count the next bytes of the connection.
    fn feed(&mut self, bytes: &[u8]) -> Progress {
        let line = Part::Name { start: true };
        for &byte in bytes {
            let (part, counted) = match (self.part, byte) {
                (part, b'\r') => (part, false),
                (Part::Name { start: true }, b'\n') => return Progress::Ended,
                (_, b'\n') => (line, false),
                (Part::Method, b' ') => (Part::Target, false),
                (Part::Target, b' ') => (Part::Version, false),
                (Part::Target, _) => (Part::Target, true),
                (Part::Method | Part::Version, _) => (self.part, false),
                (Part::Name { .. }, b':') => (Part::Space, false),
                (Part::Name { .. }, _) => (Part::Name { start: false }, true),
                (Part::Space, b' ' | b'\t') => (Part::Space, false),
                (Part::Space | Part::Value, _) => (Part::Value, true),
            };
            self.part = part;
            self.counted += usize::from(counted);
            self.read += 1;
            if self.counted >= MAX_HEAD || self.read >= MAX_READ {
                return Progress::Refused;
            }
        }
        Progress::More
    }
}

/// Read `stream` to the end of its request's head: the bytes read (the head and what came with
/// it), or `None` when Node's parser refuses the head, which is then answered as hapi answers it.
/// A connection that ends before its head does leaves the bytes read to hyper.
pub(super) async fn read(stream: &mut TcpStream) -> io::Result<Option<Vec<u8>>> {
    let mut head = Head::new();
    let mut read = Vec::new();
    let mut chunk = [0; 4096];
    loop {
        let length = stream.read(&mut chunk).await?;
        let bytes = chunk.get(..length).unwrap_or_default();
        if bytes.is_empty() {
            return Ok(Some(read));
        }
        read.extend_from_slice(bytes);
        match head.feed(bytes) {
            Progress::More => {}
            Progress::Ended => return Ok(Some(read)),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The verdict on `head` fed at once, checked against the same bytes fed one by one.
    fn verdict(head: &str) -> Progress {
        let whole = Head::new().feed(head.as_bytes());
        let mut bytewise = Head::new();
        let mut progress = Progress::More;
        for byte in head.as_bytes() {
            progress = bytewise.feed(std::slice::from_ref(byte));
            if progress != Progress::More {
                break;
            }
        }
        assert_eq!(whole, progress, "{}", &head[..40.min(head.len())]);
        whole
    }

    #[test]
    fn heads_are_refused_where_node_refuses_them() {
        // Each shape with `@` repeated: the first count that Node 18 refuses, under hapi.
        let shapes = [
            ("GET /@ HTTP/1.1\r\n\r\n", "q", 16_383),
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
            assert_eq!(verdict(&head(refused - 1)), Progress::Ended, "{shape}");
            assert_eq!(verdict(&head(refused)), Progress::Refused, "{shape}");
        }
        assert_eq!(verdict("GET / HTTP/1.1\r\nhost: a"), Progress::More);
        assert_eq!(verdict(&" ".repeat(MAX_READ)), Progress::Refused);
    }
}
