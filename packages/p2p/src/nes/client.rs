//! The nes client session.
//!
//! A session is a task that owns the connection and the client's state, as the reference's
//! client object does; [`Session`] is its handle. The task handles one event at a time (a call,
//! a timer, a received frame) in the order they come, so the state changes in the reference's
//! order. The reference's errors map to [`SessionError`] by their nes type: timeouts to
//! [`SessionError::Timeout`], disconnects to [`SessionError::Disconnect`] (a request on a
//! connection that is not open fails with it at once), server errors to
//! [`SessionError::Server`], protocol errors to [`SessionError::Protocol`], and the socket's own
//! failures to [`SessionError::Ws`].

use std::collections::{BTreeMap, VecDeque};
use std::future::{Future, poll_fn};
use std::io;
use std::net::SocketAddr;
use std::pin::{Pin, pin};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::Poll;
use std::time::Duration;

use heartwood_p2p_wire::RouteId;
use heartwood_p2p_wire::nes::{Kind, Message};
use tokio::sync::{mpsc, oneshot};
use tokio::time::{Instant, sleep_until};

use crate::errors::{ProtocolError, SessionError};
use crate::transport::{self, Dial, Reader, Received, Writer};

/// The client's receive limit when no request raises it: 100 KiB.
pub const DEFAULT_MAX_PAYLOAD: usize = 100 * 1024;
/// The request timeout until a request sets one: the connector creates its clients with 10 s.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// The first reconnect's delay; each later one waits 1 s more, and at most 5 s.
const RECONNECT_DELAY: Duration = Duration::from_secs(1);
const MAX_RECONNECT_DELAY: Duration = Duration::from_secs(5);
/// Two server pings closer than this are a ping flood.
const PING_LIMIT: Duration = Duration::from_secs(1);
/// How much of an error response's payload becomes the error's text.
const MAX_ERROR_TEXT: usize = 512;
/// How many timed-out request ids a connection remembers, so that a late response to one is told
/// from a response to an unknown id.
const REMEMBERED_TIMEOUTS: usize = 1024;

/// How a session connects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectOptions {
    /// How long the WebSocket may take to open.
    pub timeout: Duration,
    /// How many automatic reconnects follow a close, in the session's whole life.
    pub retries: u32,
}

impl Default for ConnectOptions {
    /// The connector's options: 5 s and one reconnect.
    fn default() -> ConnectOptions {
        ConnectOptions {
            timeout: Duration::from_secs(5),
            retries: 1,
        }
    }
}

/// Called with the error that the reference's client reports through its `onError` handler: a
/// protocol error, a heartbeat timeout, a failed reconnect or a failed socket. The connector
/// then terminates and forgets the session; the session itself stays open after a protocol
/// error, as the reference's does.
pub type ErrorHook = Arc<dyn Fn(SessionError) + Send + Sync>;

/// A handle to one nes session. Clones share the session; when the last one is dropped, the
/// session ends.
#[derive(Debug, Clone)]
pub struct Session {
    commands: mpsc::UnboundedSender<Command>,
    /// Whether the WebSocket is open, as the session's task last set it.
    ready: Arc<AtomicBool>,
}

impl Session {
    /// Create a session to `address` on streams from `dial` (TCP in production) and start
    /// connecting (the reference's `new Client` and `connect`); the future gives the first
    /// attempt's end. The WebSocket must open within the options' timeout, then the hello is
    /// answered within the request timeout. A failed first attempt may still reconnect in the
    /// background, as the reference's does. The session must be created on a tokio runtime.
    pub fn connect(
        address: SocketAddr,
        options: ConnectOptions,
        on_error: ErrorHook,
        dial: Dial,
    ) -> (
        Session,
        impl Future<Output = Result<(), SessionError>> + Send + 'static,
    ) {
        let (commands, receiver) = mpsc::unbounded_channel();
        let (caller, answer) = oneshot::channel();
        let ready = Arc::new(AtomicBool::new(false));
        let mut actor = Actor {
            ready: Readiness(ready.clone()),
            address,
            dial,
            on_error,
            ids: 0,
            timeout: REQUEST_TIMEOUT,
            reconnection: Some(Reconnection {
                retries: options.retries,
                wait: Duration::ZERO,
                timeout: options.timeout,
            }),
            link: Link::Idle,
            caller: Some(caller),
            pending: BTreeMap::new(),
            timed_out: VecDeque::new(),
            heartbeat: None,
            beat_at: None,
            last_pinged: None,
        };
        actor.start(options.timeout);
        tokio::spawn(actor.run(receiver));
        let connected = async move { answer.await.unwrap_or(Err(SessionError::Disconnect)) };
        (Session { commands, ready }, connected)
    }

    /// Whether the session's WebSocket is open: the reference's `_isReady`, whose socket's
    /// `readyState` is `OPEN`. It holds from the upgrade on, while the hello may still be
    /// unanswered (a request sent then follows the hello on the connection, as in the reference),
    /// and not while the session connects, waits to reconnect, closes or has ended.
    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }

    /// Send a request on `route` with `payload` and wait for its response's payload. Before it is
    /// sent, the connection's request timeout becomes `timeout` and its receive limit
    /// `max_payload`; a zero leaves the setting as it is, as the reference's connector does. The
    /// request keeps the timeout it was sent with.
    pub async fn request(
        &self,
        route: RouteId,
        payload: Vec<u8>,
        timeout: Duration,
        max_payload: usize,
    ) -> Result<Vec<u8>, SessionError> {
        let (reply, answer) = oneshot::channel();
        let request = Request {
            route,
            payload,
            timeout,
            max_payload,
            reply,
        };
        let sent = self.commands.send(Command::Request(request));
        match sent {
            Ok(()) => answer.await.unwrap_or(Err(SessionError::Disconnect)),
            Err(_) => Err(SessionError::Disconnect),
        }
    }

    /// Close the connection at once and give up reconnecting; every pending request fails.
    pub fn terminate(&self) {
        let _ = self.commands.send(Command::Terminate(None));
    }

    /// Terminate as [`Session::terminate`] does, failing the pending requests with `error`
    /// instead of a disconnect. The connector does this after a reported protocol error, so the
    /// requests that were waiting carry it; every session error is of the same class.
    pub fn terminate_for(&self, error: SessionError) {
        let _ = self.commands.send(Command::Terminate(Some(error)));
    }
}

type Reply<T> = oneshot::Sender<Result<T, SessionError>>;

#[derive(Debug)]
struct Request {
    route: RouteId,
    payload: Vec<u8>,
    timeout: Duration,
    max_payload: usize,
    reply: Reply<Vec<u8>>,
}

#[derive(Debug)]
enum Command {
    Request(Request),
    /// Close at once; the pending requests fail with the error, or with a disconnect.
    Terminate(Option<SessionError>),
}

/// The reconnects left, the delay of the last one, and the attempts' timeout.
struct Reconnection {
    retries: u32,
    wait: Duration,
    timeout: Duration,
}

/// A request waiting for its response; the hello has no caller.
struct Pending {
    deadline: Instant,
    reply: Option<Reply<Vec<u8>>>,
}

type Opening = Pin<Box<dyn Future<Output = io::Result<(Reader, Writer)>> + Send>>;

/// The WebSocket: none, a reconnect's wait, an attempt with its deadline, or open.
enum Link {
    Idle,
    Waiting(Instant),
    Connecting(Opening, Option<Instant>),
    Open(Reader, Writer, usize),
}

enum Event {
    Command(Option<Command>),
    Timer,
    Opened(io::Result<(Reader, Writer)>),
    Received(Received),
}

impl Link {
    async fn event(&mut self) -> Event {
        match self {
            Link::Connecting(opening, _) => Event::Opened(opening.as_mut().await),
            Link::Open(reader, _, limit) => Event::Received(reader.next(*limit).await),
            Link::Idle | Link::Waiting(_) => std::future::pending().await,
        }
    }
}

/// The session's readiness as its handles see it; false again when the task ends.
struct Readiness(Arc<AtomicBool>);

impl Readiness {
    fn set(&self, open: bool) {
        self.0.store(open, Ordering::Release);
    }
}

impl Drop for Readiness {
    fn drop(&mut self) {
        self.set(false);
    }
}

struct Actor {
    ready: Readiness,
    address: SocketAddr,
    dial: Dial,
    on_error: ErrorHook,
    ids: u32,
    /// The connection-wide request timeout.
    timeout: Duration,
    reconnection: Option<Reconnection>,
    link: Link,
    /// The first attempt's caller, until the attempt ends.
    caller: Option<Reply<()>>,
    pending: BTreeMap<u32, Pending>,
    /// The ids of the connection's requests that timed out, oldest first.
    timed_out: VecDeque<u32>,
    heartbeat: Option<Duration>,
    beat_at: Option<Instant>,
    last_pinged: Option<Instant>,
}

/// `delay` from now.
fn after(delay: Duration) -> Instant {
    let now = Instant::now();
    now.checked_add(delay).unwrap_or(now)
}

fn ws(text: &str) -> SessionError {
    SessionError::Ws(text.to_owned())
}

impl Actor {
    async fn run(mut self, mut commands: mpsc::UnboundedReceiver<Command>) {
        loop {
            // Calls first, then timers (the reference's event loop runs timers before I/O), then
            // the connection.
            let event = {
                let deadline = self.deadline();
                let mut timer = pin!(async move {
                    match deadline {
                        Some(at) => sleep_until(at).await,
                        None => std::future::pending().await,
                    }
                });
                let mut link = pin!(self.link.event());
                poll_fn(|context| {
                    if let Poll::Ready(command) = commands.poll_recv(context) {
                        return Poll::Ready(Event::Command(command));
                    }
                    if timer.as_mut().poll(context).is_ready() {
                        return Poll::Ready(Event::Timer);
                    }
                    link.as_mut().poll(context)
                })
                .await
            };
            match event {
                Event::Command(None) => return,
                Event::Command(Some(Command::Request(request))) => self.request(request),
                Event::Command(Some(Command::Terminate(error))) => self.terminate(error),
                Event::Timer => self.timers(),
                Event::Opened(opened) => self.opened(opened),
                Event::Received(received) => self.received(received),
            }
            self.ready.set(matches!(self.link, Link::Open(..)));
        }
    }

    fn deadline(&self) -> Option<Instant> {
        let link = match self.link {
            Link::Waiting(at) => Some(at),
            Link::Connecting(_, at) => at,
            _ => None,
        };
        let requests = self.pending.values().map(|pending| pending.deadline);
        requests.chain(self.beat_at).chain(link).min()
    }

    /// A connection attempt (`_connect`).
    fn start(&mut self, timeout: Duration) {
        let (dial, address) = (self.dial.clone(), self.address);
        let opening = Box::pin(async move { transport::open(dial(address).await?, address).await });
        self.link = Link::Connecting(opening, (!timeout.is_zero()).then(|| after(timeout)));
    }

    fn request(&mut self, request: Request) {
        if !request.timeout.is_zero() {
            self.timeout = request.timeout;
        }
        let Link::Open(_, _, limit) = &mut self.link else {
            let _ = request.reply.send(Err(SessionError::Disconnect));
            return;
        };
        if request.max_payload != 0 {
            *limit = request.max_payload;
        }
        let id = self.track(Some(request.reply));
        self.write(&Message::request(id, request.route, request.payload));
    }

    fn terminate(&mut self, error: Option<SessionError>) {
        self.reconnection = None;
        match std::mem::replace(&mut self.link, Link::Idle) {
            Link::Idle | Link::Waiting(_) => {}
            Link::Connecting(..) => self.fail(ws("Socket error")),
            Link::Open(_, writer, _) => {
                writer.terminate();
                self.cleanup_with(error.unwrap_or(SessionError::Disconnect));
            }
        }
    }

    fn opened(&mut self, opened: io::Result<(Reader, Writer)>) {
        match opened {
            Ok((reader, writer)) => {
                self.link = Link::Open(reader, writer, DEFAULT_MAX_PAYLOAD);
                let id = self.track(None);
                self.write(&Message::client_hello(id));
            }
            Err(_) if self.will_reconnect() => {
                self.finalise(Some(ws("Connection terminated while waiting to connect")));
                self.cleanup();
                self.reconnect();
            }
            Err(_) => self.fail(ws("Socket error")),
        }
    }

    fn received(&mut self, received: Received) {
        match received {
            Received::Binary(bytes) => self.message(Some(bytes)),
            Received::Text => self.message(None),
            // The WebSocket library answers a ping; then the client drops the connection for good,
            // without a report. The pending requests fail as disconnected in the reference; here
            // they carry the frame, of the same class, so the peer's violation can be recorded.
            Received::Ping(payload) => {
                if let Link::Open(_, writer, _) = &self.link {
                    writer.pong(payload);
                }
                self.control_frame();
            }
            Received::Pong => self.control_frame(),
            Received::Closed(None) => self.closed(),
            Received::Closed(Some(code)) => {
                self.end(Some(code).filter(|code| *code != 1005));
                self.closed();
            }
            Received::Failed(code) => {
                self.end(Some(code));
                if self.will_reconnect() {
                    self.closed();
                } else {
                    self.fail(ws("Socket error"));
                }
            }
        }
    }

    /// A received message (`_onMessage`); `None` for a text frame.
    fn message(&mut self, bytes: Option<Vec<u8>>) {
        self.beat();
        let Some(bytes) = bytes else {
            return self.report(ProtocolError::TextFrame);
        };
        let update = match Message::parse(&bytes) {
            Ok(update) => update,
            Err(error) => return self.report(ProtocolError::Frame(error)),
        };
        if update.kind == Kind::Ping {
            let now = Instant::now();
            let last = self.last_pinged.replace(now);
            if last
                .and_then(|at| at.checked_add(PING_LIMIT))
                .is_some_and(|end| now < end)
            {
                return self.report(ProtocolError::PingFlood);
            }
            let id = self.next_id();
            return self.write(&Message::ping(id));
        }
        if let Link::Open(_, _, limit) = &mut self.link {
            *limit = DEFAULT_MAX_PAYLOAD;
        }
        let error = (update.status_code >= 400).then(|| {
            let text = update
                .payload
                .get(..MAX_ERROR_TEXT)
                .unwrap_or(&update.payload);
            let payload = String::from_utf8_lossy(text).into_owned();
            SessionError::Server {
                status: update.status_code,
                payload,
            }
        });
        let Some(pending) = self.pending.remove(&update.id) else {
            let late = self.timed_out.iter().position(|id| *id == update.id);
            return match late.and_then(|index| self.timed_out.remove(index)) {
                Some(_) => self.report(ProtocolError::LateResponse),
                None => self.report(ProtocolError::UnknownRequest),
            };
        };
        let answer = match update.kind {
            Kind::Request => Ok(update.payload),
            Kind::Hello => {
                let (interval, timeout) = (update.heartbeat.interval, update.heartbeat.timeout);
                let beat = u64::from(interval).saturating_add(u64::from(timeout));
                self.heartbeat = (beat > 0).then(|| Duration::from_millis(beat));
                self.beat();
                // A hello answering a request resolves it with nothing in the reference, which
                // its communicator then fails to read.
                match pending.reply {
                    None => Ok(Vec::new()),
                    Some(_) => Err(SessionError::NoPayload),
                }
            }
            Kind::Error | Kind::Ping => {
                self.settle(
                    pending,
                    Err(SessionError::Protocol(ProtocolError::UnknownType)),
                );
                return self.report(ProtocolError::UnknownType);
            }
        };
        self.settle(pending, error.map_or(answer, Err));
    }

    fn timers(&mut self) {
        let now = Instant::now();
        match self.link {
            // The attempt timed out; only the first attempt reconnects afterwards.
            Link::Connecting(_, Some(at)) if at <= now => {
                let first = self.caller.is_some();
                self.cleanup();
                self.finalise(Some(SessionError::Timeout));
                if first {
                    self.reconnect();
                }
            }
            Link::Waiting(at) if at <= now => {
                let timeout = self
                    .reconnection
                    .as_ref()
                    .map_or(Duration::ZERO, |r| r.timeout);
                self.start(timeout);
            }
            _ => {}
        }
        let expired: Vec<u32> = (self.pending.iter())
            .filter(|(_, pending)| pending.deadline <= now)
            .map(|(id, _)| *id)
            .collect();
        for id in expired {
            if let Some(pending) = self.pending.remove(&id) {
                if pending.reply.is_some() {
                    if self.timed_out.len() == REMEMBERED_TIMEOUTS {
                        self.timed_out.pop_front();
                    }
                    self.timed_out.push_back(id);
                }
                self.settle(pending, Err(SessionError::Timeout));
            }
        }
        if self.beat_at.is_some_and(|at| at <= now) {
            self.beat_at = None;
            (self.on_error)(SessionError::Protocol(ProtocolError::HeartbeatTimeout));
            self.end(None);
            self.closed();
        }
    }

    /// Answer a pending request; for the hello, end the connection attempt.
    fn settle(&mut self, pending: Pending, result: Result<Vec<u8>, SessionError>) {
        match (pending.reply, result) {
            (Some(reply), result) => {
                let _ = reply.send(result);
            }
            (None, Ok(_)) => self.finalise(None),
            // A failed hello gives the connection up for good.
            (None, Err(error)) => {
                self.reconnection = None;
                if matches!(self.link, Link::Waiting(_)) {
                    self.link = Link::Idle;
                }
                self.end(None);
                self.fail(error);
            }
        }
    }

    /// The connection closed (the reference's close handler): fail the pending requests and
    /// reconnect if one is left.
    fn closed(&mut self) {
        self.cleanup();
        self.reconnect();
    }

    /// The connection failed with no reconnect left, or the attempt was given up.
    fn fail(&mut self, error: SessionError) {
        self.cleanup();
        self.finalise(Some(error));
    }

    /// End the open connection gracefully: a close frame with `code`, then the end of writes.
    fn end(&mut self, code: Option<u16>) {
        if let Link::Open(_, writer, _) = std::mem::replace(&mut self.link, Link::Idle) {
            writer.close(code);
        }
    }

    /// Drop the connection, stop the heartbeat, and fail every pending request (`_cleanup`).
    fn cleanup(&mut self) {
        self.cleanup_with(SessionError::Disconnect);
    }

    /// [`Actor::cleanup`], failing the pending requests with `error`.
    fn cleanup_with(&mut self, error: SessionError) {
        if matches!(self.link, Link::Open(..) | Link::Connecting(..)) {
            self.link = Link::Idle;
        }
        self.beat_at = None;
        self.timed_out.clear();
        for pending in std::mem::take(&mut self.pending).into_values() {
            self.settle(pending, Err(error.clone()));
        }
    }

    /// A WebSocket ping or pong: the connection ends for good.
    fn control_frame(&mut self) {
        self.reconnection = None;
        self.cleanup_with(SessionError::Protocol(ProtocolError::ControlFrame));
    }

    /// Wait before the next reconnect, if one is left (`_reconnect`).
    fn reconnect(&mut self) {
        let Some(reconnection) = self.reconnection.as_mut() else {
            return;
        };
        if reconnection.retries < 1 {
            self.reconnection = None;
            return;
        }
        reconnection.retries = reconnection.retries.saturating_sub(1);
        reconnection.wait = reconnection.wait.saturating_add(RECONNECT_DELAY);
        self.link = Link::Waiting(after(reconnection.wait.min(MAX_RECONNECT_DELAY)));
    }

    fn will_reconnect(&self) -> bool {
        self.reconnection.as_ref().is_some_and(|r| r.retries >= 1)
    }

    /// End the connection attempt (`finalise`): the first attempt's caller gets the result; a
    /// later error goes to the error hook, and then a reconnect follows if one is left.
    fn finalise(&mut self, error: Option<SessionError>) {
        if let Some(caller) = self.caller.take() {
            let _ = caller.send(error.map_or(Ok(()), Err));
        } else if let Some(error) = error {
            (self.on_error)(error);
            self.reconnect();
        }
    }

    fn report(&self, error: ProtocolError) {
        (self.on_error)(SessionError::Protocol(error));
    }

    fn beat(&mut self) {
        if let Some(heartbeat) = self.heartbeat {
            self.beat_at = Some(after(heartbeat));
        }
    }

    fn next_id(&mut self) -> u32 {
        self.ids = self.ids.wrapping_add(1);
        self.ids
    }

    /// A request with the next id, waiting for its response until the current timeout.
    fn track(&mut self, reply: Option<Reply<Vec<u8>>>) -> u32 {
        let id = self.next_id();
        let deadline = after(self.timeout);
        self.pending.insert(id, Pending { deadline, reply });
        id
    }

    fn write(&self, message: &Message) {
        if let (Link::Open(_, writer, _), Ok(bytes)) = (&self.link, message.encode()) {
            writer.binary(bytes);
        }
    }
}
