//! A blocking HTTP/1.1 GET to a loopback address, over `std::net`, for the devnet's reads of its
//! relays' APIs: no HTTP client crate, no TLS, no redirects. A body is read up to
//! [`MAX_RESPONSE`] bytes, by its `Content-Length` or to the end of the connection.

use std::net::SocketAddr;
use std::time::Duration;

use crate::errors::HttpError;
use crate::seams::{ApiBlock, ApiNodeStatus, ApiPeer, NodeApi};

/// The longest response read.
pub const MAX_RESPONSE: usize = 16 * 1024 * 1024;

/// An HTTP response: its status and body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    /// The status code.
    pub status: u16,
    /// The body.
    pub body: Vec<u8>,
}

/// `GET <path>` from the loopback `address`, within `timeout`.
pub fn get(address: SocketAddr, path: &str, timeout: Duration) -> Result<Response, HttpError> {
    let _ = (address, path, timeout);
    todo!("the loopback GET")
}

/// The [`NodeApi`] over [`get`], each read within `timeout`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoopbackApi {
    /// The time each read may take.
    pub timeout: Duration,
}

impl NodeApi for LoopbackApi {
    fn node_status(&self, api: SocketAddr) -> Result<ApiNodeStatus, HttpError> {
        let _ = (api, self.timeout);
        todo!("GET /api/node/status")
    }

    fn last_block(&self, api: SocketAddr) -> Result<ApiBlock, HttpError> {
        let _ = api;
        todo!("GET /api/blocks/last")
    }

    fn block_at(&self, api: SocketAddr, height: u32) -> Result<Option<ApiBlock>, HttpError> {
        let _ = (api, height);
        todo!("GET /api/blocks/{height}")
    }

    fn peers(&self, api: SocketAddr) -> Result<Vec<ApiPeer>, HttpError> {
        let _ = api;
        todo!("GET /api/peers")
    }
}
