//! The listener's connections: the WebSocket upgrade on `/` only (any other path is a 404, which
//! bans the caller with "Not a websocket connection"), the 2 s socket timeout of the HTTP
//! exchange, and messages of at most 20 MiB.

use std::time::Duration;

use tokio::net::TcpStream;
use tokio_tungstenite::WebSocketStream;

use super::seams::ServerRefusal;

/// The largest message a socket takes: 20 MiB.
pub const MAX_MESSAGE: usize = 20 * 1024 * 1024;
/// The HTTP socket timeout before the upgrade.
pub const SOCKET_TIMEOUT: Duration = Duration::from_secs(2);

/// The upgrade of `stream` to a WebSocket on `/`, or the refusal that bans its address.
pub async fn upgrade(stream: TcpStream) -> Result<WebSocketStream<TcpStream>, ServerRefusal> {
    let _ = stream;
    todo!("the WebSocket upgrade")
}
