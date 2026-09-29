//! The nes server session of one socket, as the reference's nes socket serves it.
//!
//! - Every message is parsed first: a text message ends the socket "Invalid message received",
//!   one that does not parse "Invalid payload received". Then an id 0 ends it "Message missing
//!   id"; a ping within 1 s of the last one "Exceeded ping limit"; a second hello "Connection
//!   already initialised", a hello of another version than "2" "Incorrect protocol version
//!   (expected 2 but received `v`)", a hello with a subscription payload other than `[]` "Invalid
//!   subscription payload"; anything before the hello "Connection is not initialised"; an error
//!   message "Unknown message type" ("Exceeded socket rate limit" after more than ten messages
//!   within a second).
//! - A request names its route by id (`/p2p.peer.getStatus`; an unknown id ends the socket "Not
//!   Found", no path "Message missing path") or by its HTTP path (`//p2p/peer/getStatus`, one that
//!   names no route answering 404). Each request is handled on its own task, so a pending long
//!   poll or AwaitBlock wait never delays another request on the socket; the answers go out as
//!   they come. A 499 is answered and the socket kept; any other refusal ends the socket.
//! - The server pings every socket every 15 s from its start, and 5 s later closes each socket
//!   that has neither received a message, nor been sent anything but a ping, since the check
//!   before, nor a request in progress, nor a message being written to a peer that reads (bytes
//!   written since the check before). Dropped without a reason, as peers that do not read: a
//!   socket whose unwritten bytes pass [`super::listener::MAX_BACKLOG`]; one whose bytes that
//!   waited at the check before are still unwritten, whatever it sends. The reference keeps such
//!   sockets, and their unwritten messages, without a bound.
//! - A WebSocket ping or pong frame ends the socket ("Malicious ping frame received", "Malicious
//!   pong frame received"), and so does a frame fault, with the WebSocket library's text.
//!
//! A socket ended with a reason is banned with it when it closes (see [`super::ban_hammer`]). The
//! transport faults no honest client causes (a WebSocket control frame, a text message, a frame or
//! message that does not parse or is over 20 MiB, a bad or repeated hello) are also recorded as
//! the caller's protocol violations, the forger's addresses excepted.

use std::collections::VecDeque;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use heartwood_kernel::debug;
use heartwood_p2p_wire::RouteId;
use heartwood_p2p_wire::nes::{Heartbeat, Kind, Message};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio::time::{Instant, sleep_until, timeout};

use super::listener::{Frame, FrameReader, FrameWriter, MAX_BACKLOG, Writes};
use super::plugins::{Answer, Plugins};
use super::seams::SessionId;
use crate::scoring::Violation;

/// The nes protocol version of the hello.
pub const PROTOCOL: &str = "2";
/// How often the server sends a heartbeat.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);
/// How long the client has to answer a heartbeat.
pub const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a socket the server closes waits for the client's close frame.
pub const CLOSE_TIMEOUT: Duration = Duration::from_secs(30);
/// Two client pings closer than this end the socket.
const PING_LIMIT: Duration = Duration::from_secs(1);
/// The largest hello subscription payload read.
const MAX_SUBSCRIPTIONS: usize = 102_400;

/// How a socket ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEnd {
    /// The socket closed before its WebSocket opened.
    Handshake,
    /// The WebSocket closed.
    Closed,
    /// The server ended the socket, with the reason the ban hammer bans with.
    Terminated {
        /// The reason.
        reason: String,
    },
}

/// What the server tells a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    /// Drop the socket at once: a newer socket from its address opened.
    Destroy,
    /// Close the WebSocket: the server stops, or the forger's sessions close.
    Close,
}

/// What a session is made of.
pub struct SessionParts {
    /// The session's number.
    pub id: SessionId,
    /// The caller's address, normalised.
    pub ip: IpAddr,
    /// The plugin chain and the handlers.
    pub plugins: Arc<Plugins>,
    /// When the server started, from which its heartbeats are counted.
    pub beats_from: Instant,
    /// The server's orders.
    pub control: mpsc::UnboundedReceiver<Control>,
}

/// What goes out for a request's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    /// The response's payload.
    Payload(Vec<u8>),
    /// A graceful error (499) with this JSON payload; the socket stays open.
    Graceful(Vec<u8>),
    /// The socket ends with this reason.
    Terminate(String),
}

/// The reply to `answer`, as the reference's socket writes it: its HTTP layer hands the socket
/// the refusal's message, or `null` after the close-connection plugin emptied the response, and
/// a 500's text is "An internal server error occurred".
pub fn reply(answer: Answer) -> Reply {
    let refusal = match answer.result {
        Ok(payload) => return Reply::Payload(payload),
        Err(refusal) => refusal,
    };
    let text = if answer.close {
        "null"
    } else {
        refusal.message.as_str()
    };
    match refusal.status {
        499 => {
            let message = Value::String(text.to_owned());
            Reply::Graceful(format!("{{\"error\":\"Unknown\",\"message\":{message}}}").into_bytes())
        }
        500 => Reply::Terminate("An internal server error occurred".to_owned()),
        _ => Reply::Terminate(text.to_owned()),
    }
}

/// The route a request's parsed path names: `Ok(None)` for an HTTP path that names none, or the
/// reason the socket ends with.
pub fn route_of(path: &str) -> Result<Option<RouteId>, &'static str> {
    let Some(path) = path.get(1..).filter(|_| !path.is_empty()) else {
        return Err("Message missing path");
    };
    if !path.starts_with('/') {
        return RouteId::from_id(path).map(Some).ok_or("Not Found");
    }
    let path = path.split('?').next().unwrap_or_default();
    Ok(RouteId::ALL.into_iter().find(|route| route.path() == path))
}

/// Why the loop stopped.
enum Stop {
    /// Drop the socket, with a reason or without.
    Terminate(Option<String>),
    /// Close the WebSocket with this code and wait for the client's close frame.
    Close(u16),
    /// Answer the client's close frame with this code.
    Echo(u16),
    /// The stream ended.
    Ended,
}

/// The nes server session of one socket.
pub struct ServerSession {
    parts: SessionParts,
    helloed: bool,
    /// The socket was active since the last heartbeat check.
    pinged: bool,
    in_flight: usize,
    last_ping: Option<Instant>,
    /// When the messages after the hello of the last second came.
    recent: VecDeque<Instant>,
}

impl ServerSession {
    /// A session of `parts`.
    pub fn new(parts: SessionParts) -> ServerSession {
        ServerSession {
            parts,
            helloed: false,
            pinged: true,
            in_flight: 0,
            last_ping: None,
            recent: VecDeque::new(),
        }
    }

    /// Serve the WebSocket of `reader` and `writer` until it closes or the session ends it.
    pub async fn run(mut self, mut reader: FrameReader, writer: FrameWriter) -> SessionEnd {
        let (answers, mut answered) = mpsc::unbounded_channel::<(u32, Answer)>();
        let mut beat = next_beat(self.parts.beats_from, Instant::now());
        let mut check = None;
        let stop = loop {
            let check_at = check.unwrap_or(beat);
            tokio::select! {
                biased;
                control = self.parts.control.recv() => match control {
                    Some(Control::Close) => break Stop::Close(1005),
                    Some(Control::Destroy) | None => break Stop::Terminate(None),
                },
                () = sleep_until(beat) => {
                    send(&writer, &Message::ping(1));
                    check = beat.checked_add(HEARTBEAT_TIMEOUT);
                    beat = beat.checked_add(HEARTBEAT_INTERVAL).unwrap_or(beat);
                }
                () = sleep_until(check_at), if check.is_some() => {
                    check = None;
                    let writes = writer.check();
                    if writes == Writes::Stalled {
                        debug!("Dropped the socket of {}, which reads nothing", self.parts.ip);
                        break Stop::Terminate(None);
                    } else if writes == Writes::Idle && !self.pinged && self.in_flight == 0 {
                        break Stop::Close(1005);
                    }
                    self.pinged = false;
                }
                Some((id, answer)) = answered.recv() => {
                    self.in_flight = self.in_flight.saturating_sub(1);
                    match reply(answer) {
                        Reply::Payload(payload) => self.send(&writer, &Message::response(id, payload)),
                        Reply::Graceful(body) => self.send(&writer, &Message::graceful_error(id, &body)),
                        Reply::Terminate(reason) => break Stop::Terminate(Some(reason)),
                    }
                    let backlog = writer.backlog();
                    if backlog > MAX_BACKLOG {
                        let ip = self.parts.ip;
                        debug!("Dropped the socket of {ip}, which has {backlog} bytes to read");
                        break Stop::Terminate(None);
                    }
                }
                frame = reader.next() => {
                    if let Some(stop) = self.frame(frame, &writer, &answers) {
                        break stop;
                    }
                }
            }
        };
        match stop {
            Stop::Terminate(reason) => {
                writer.terminate();
                reason.map_or(SessionEnd::Closed, |reason| SessionEnd::Terminated {
                    reason,
                })
            }
            Stop::Close(code) => {
                writer.close(code);
                let _ = timeout(CLOSE_TIMEOUT, async {
                    while !matches!(
                        reader.next().await,
                        Frame::Close(_) | Frame::Ended | Frame::Failed(_)
                    ) {}
                })
                .await;
                SessionEnd::Closed
            }
            Stop::Echo(code) => {
                writer.close(code);
                SessionEnd::Closed
            }
            Stop::Ended => SessionEnd::Closed,
        }
    }

    /// A frame from the client; the stop it causes.
    fn frame(
        &mut self,
        frame: Frame,
        writer: &FrameWriter,
        answers: &mpsc::UnboundedSender<(u32, Answer)>,
    ) -> Option<Stop> {
        match frame {
            Frame::Binary(bytes) => self.message(&bytes, writer, answers),
            Frame::Text => self.fault("Invalid message received"),
            Frame::Ping => self.fault("Malicious ping frame received"),
            Frame::Pong => self.fault("Malicious pong frame received"),
            Frame::Failed(Some(reason)) => self.fault(&reason),
            Frame::Failed(None) => {
                self.violation();
                Some(Stop::Close(1007))
            }
            Frame::Close(code) => Some(Stop::Echo(code)),
            Frame::Ended => Some(Stop::Ended),
        }
    }

    /// A nes message (`_onMessage` and `_lifecycle`).
    fn message(
        &mut self,
        bytes: &[u8],
        writer: &FrameWriter,
        answers: &mpsc::UnboundedSender<(u32, Answer)>,
    ) -> Option<Stop> {
        let Ok(message) = Message::parse(bytes) else {
            return self.fault("Invalid payload received");
        };
        self.pinged = true;
        let now = Instant::now();
        if message.id == 0 {
            return end("Message missing id");
        }
        match message.kind {
            Kind::Ping => {
                let last = self.last_ping.replace(now);
                let limit = last.and_then(|last| last.checked_add(PING_LIMIT));
                limit
                    .filter(|limit| now < *limit)
                    .and(end("Exceeded ping limit"))
            }
            Kind::Hello => self.hello(&message, writer),
            _ if !self.helloed => end("Connection is not initialised"),
            kind => {
                self.recent.push_back(now);
                self.recent.retain(|at| {
                    at.checked_add(Duration::from_secs(1))
                        .is_some_and(|end| now < end)
                });
                if kind == Kind::Error {
                    return match self.recent.len() {
                        11.. => end("Exceeded socket rate limit"),
                        _ => end("Unknown message type"),
                    };
                }
                let route = match route_of(&message.path) {
                    Ok(route) => route,
                    Err(reason) => return end(reason),
                };
                self.in_flight = self.in_flight.saturating_add(1);
                let plugins = Arc::clone(&self.parts.plugins);
                let (ip, session, answers) = (self.parts.ip, self.parts.id, answers.clone());
                let (id, payload) = (message.id, message.payload);
                tokio::spawn(async move {
                    let answer = plugins.handle(route, payload, ip, session).await;
                    let _ = answers.send((id, answer));
                });
                None
            }
        }
    }

    /// The hello (`_processHello`).
    fn hello(&mut self, message: &Message, writer: &FrameWriter) -> Option<Stop> {
        if self.helloed {
            return self.fault("Connection already initialised");
        }
        if message.version.to_string() != PROTOCOL {
            let version = message.version;
            return self.fault(&format!(
                "Incorrect protocol version (expected {PROTOCOL} but received {version})"
            ));
        }
        self.helloed = true;
        // No subscription exists, so only an empty list subscribes to nothing.
        let payload = &message.payload;
        let subscriptions = || serde_json::from_str(&String::from_utf8_lossy(payload)).ok();
        if !payload.is_empty()
            && (payload.len() > MAX_SUBSCRIPTIONS
                || subscriptions() != Some(Value::Array(Vec::new())))
        {
            return self.fault("Invalid subscription payload");
        }
        let heartbeat = Heartbeat {
            interval: millis(HEARTBEAT_INTERVAL),
            timeout: millis(HEARTBEAT_TIMEOUT),
        };
        let socket = format!("heartwood:{}", self.parts.id.0);
        self.send(
            writer,
            &Message::server_hello(message.id, heartbeat, socket),
        );
        None
    }

    /// A transport fault: a violation of the caller, and the end of the socket with `reason`.
    fn fault(&self, reason: &str) -> Option<Stop> {
        self.violation();
        end(reason)
    }

    fn violation(&self) {
        let (plugins, ip) = (&self.parts.plugins, self.parts.ip);
        if !plugins.is_forger(ip) {
            plugins.peers().record_violation(ip, Violation::Protocol);
        }
    }

    /// Send `message`, which counts as activity.
    fn send(&mut self, writer: &FrameWriter, message: &Message) {
        self.pinged = true;
        send(writer, message);
    }
}

fn end(reason: &str) -> Option<Stop> {
    Some(Stop::Terminate(Some(reason.to_owned())))
}

fn send(writer: &FrameWriter, message: &Message) {
    if let Ok(bytes) = message.encode() {
        writer.binary(&bytes, message.kind != Kind::Ping);
    }
}

fn millis(duration: Duration) -> u16 {
    u16::try_from(duration.as_millis()).unwrap_or(u16::MAX)
}

/// The first heartbeat after `now` of a server whose heartbeats count from `from`.
fn next_beat(from: Instant, now: Instant) -> Instant {
    let interval = u64::from(millis(HEARTBEAT_INTERVAL));
    let elapsed =
        u64::try_from(now.saturating_duration_since(from).as_millis()).unwrap_or(u64::MAX);
    let beats = elapsed
        .checked_div(interval)
        .unwrap_or_default()
        .saturating_add(1);
    let offset = Duration::from_millis(beats.saturating_mul(interval));
    from.checked_add(offset).unwrap_or(now)
}
