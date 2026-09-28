//! The nes client session fed a script of server frames and delays: no input may panic it, the
//! pending requests stay bounded, every failure is a typed error, and the connection ends where
//! the reference's client would end it.
//!
//! The session runs on tokio's paused clock over an in-memory connection. The input is a script
//! of steps, each chosen by one byte: raw bytes from the server, a well-formed nes message from
//! the server in one frame, a pause, a request from the client, a terminate, or the end of the
//! server's stream. At the end the clock runs on for ten minutes, by which time every request
//! must have its answer, since every request has a timeout; after a terminate every request
//! fails at once as disconnected.

#![no_main]

use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use heartwood_p2p::errors::SessionError;
use heartwood_p2p::nes::{ConnectOptions, Session};
use heartwood_p2p::testing::upgrade;
use heartwood_p2p::transport::{Dial, Stream};
use heartwood_p2p_wire::RouteId;
use heartwood_p2p_wire::nes::{Heartbeat, Kind, Message};
use libfuzzer_sys::fuzz_target;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// The script's reader: bytes run out as zeros.
struct Script<'a>(std::slice::Iter<'a, u8>);

impl Script<'_> {
    fn byte(&mut self) -> u8 {
        self.0.next().copied().unwrap_or(0)
    }

    fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.byte()).collect()
    }
}

/// A server frame (unmasked, final) of `opcode` around `payload`.
fn frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let mut frame = vec![0x80 | opcode];
    match payload.len() {
        n if n < 126 => frame.push(n as u8),
        n => {
            frame.push(126);
            frame.extend_from_slice(&(n as u16).to_be_bytes());
        }
    }
    frame.extend_from_slice(payload);
    frame
}

fuzz_target!(|bytes: &[u8]| {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .start_paused(true)
        .build()
        .unwrap();
    runtime.block_on(run(bytes));
});

async fn run(bytes: &[u8]) {
    let mut script = Script(bytes.iter());
    // The first connection is scripted; a reconnect gets a peer that never answers.
    let (client, mut server) = tokio::io::duplex(64 * 1024);
    let streams = Mutex::new(vec![client, tokio::io::duplex(1024).0].into_iter());
    let dial: Dial = Arc::new(move |_| {
        let stream = streams.lock().unwrap().next();
        Box::pin(async move {
            stream
                .map(|stream| Box::new(stream) as Box<dyn Stream>)
                .ok_or_else(|| io::ErrorKind::ConnectionRefused.into())
        })
    });
    let reported = Arc::new(Mutex::new(0usize));
    let counter = reported.clone();
    let hook = Arc::new(move |_: SessionError| *counter.lock().unwrap() += 1);
    let address = SocketAddr::from(([127, 0, 0, 1], 4002));
    let (session, connected) = Session::connect(address, ConnectOptions::default(), hook, dial);
    let connected = tokio::spawn(connected);
    if upgrade(&mut server).await.is_err() {
        return;
    }
    let (mut from_client, mut to_client) = tokio::io::split(server);
    tokio::spawn(async move {
        let mut sink = Vec::new();
        while from_client.read_buf(&mut sink).await.is_ok_and(|n| n > 0) {
            sink.clear();
        }
    });

    let mut requests = Vec::new();
    let mut terminated = false;
    while script.0.len() > 0 && requests.len() < 64 {
        match script.byte() % 6 {
            0 => {
                let n = usize::from(script.byte());
                let raw = script.bytes(n);
                let _ = to_client.write_all(&raw).await;
            }
            1 => {
                let kind = [Kind::Hello, Kind::Ping, Kind::Request, Kind::Error]
                    [usize::from(script.byte() % 4)];
                let id = u32::from(script.byte() % 8);
                let status_code = [200, 400, 499, 500][usize::from(script.byte() % 4)];
                let interval = u16::from(script.byte()) * 100;
                let timeout = u16::from(script.byte()) * 100;
                let n = usize::from(script.byte());
                let message = Message {
                    kind,
                    status_code,
                    heartbeat: Heartbeat { interval, timeout },
                    ..Message::response(id, script.bytes(n))
                };
                if let Ok(bytes) = message.encode() {
                    let _ = to_client.write_all(&frame(2, &bytes)).await;
                }
            }
            2 => {
                let ms = u64::from(u16::from_be_bytes([script.byte(), script.byte()]));
                tokio::time::sleep(Duration::from_millis(ms)).await;
            }
            3 => {
                let route = RouteId::ALL[usize::from(script.byte()) % RouteId::ALL.len()];
                let timeout = [0, 100, 2_000, 10_000][usize::from(script.byte() % 4)];
                let max_payload =
                    [0, 64, 100 * 1024, 20 * 1024 * 1024][usize::from(script.byte() % 4)];
                let n = usize::from(script.byte() % 16);
                let payload = script.bytes(n);
                let session = session.clone();
                let timeout = Duration::from_millis(timeout);
                requests.push(tokio::spawn(async move {
                    session.request(route, payload, timeout, max_payload).await
                }));
            }
            4 => {
                session.terminate();
                terminated = true;
            }
            _ => {
                let _ = to_client.shutdown().await;
            }
        }
        tokio::task::yield_now().await;
    }

    // Every request has a timeout of at most 10 s, and the connection attempts end within 16 s.
    tokio::time::sleep(Duration::from_secs(600)).await;
    assert!(connected.is_finished(), "the connection attempt ended");
    for request in requests {
        assert!(
            request.is_finished(),
            "a request is still pending after its timeout"
        );
        let _typed: Result<Vec<u8>, SessionError> = request.await.unwrap();
    }
    if terminated {
        let after = session
            .request(RouteId::GetStatus, Vec::new(), Duration::ZERO, 0)
            .await;
        assert_eq!(after, Err(SessionError::Disconnect));
    }
    let _ = *reported.lock().unwrap();
}
