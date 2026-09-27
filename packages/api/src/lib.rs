//! `heartwood-api`: the public API of Heartwood Core.
//!
//! A relay serves the reference's routes that a follower answers: the blocks, the node, the peers,
//! the transactions (confirmed, in the pool, and `POST /transactions`) and the wallets, with the
//! reference's envelope, query grammar, pagination, headers, error bodies, rate limit and
//! semaphore.
//!
//! - [`context`]: what the routes read ([`ApiContext`]) and the route table's types.
//! - [`controllers`]: the routes' handlers; [`resources`]: the response shapes.
//! - [`schemas`]: the query grammar and the routes' validation; [`pagination`]: the list
//!   envelope.
//! - [`plugins`]: what wraps every route; [`server`]: the options and the accept loop.
//! - [`errors`]: [`ApiError`], Boom's error answer, and [`ServeError`].
//!
//! The API reads only published snapshots and read-only store connections, so a request never
//! waits for the node thread, except `POST /transactions` for its processor's answer.

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

pub mod context;
pub mod controllers;
pub mod errors;
pub mod pagination;
pub mod plugins;
pub mod resources;
pub mod schemas;
pub mod server;

use std::sync::Arc;

use heartwood_kernel::errors::BootError;
use heartwood_kernel::lifecycle::Component;

pub use crate::context::{
    ApiContext, ApiReply, ApiRequest, NodeInfo, NodeStatus, PeerList, Route, RouteId,
    SubmitTransactions,
};
pub use crate::errors::{ApiError, ServeError};
pub use crate::server::{ApiConfig, ServerHandle};

/// The API component.
pub struct Api {
    context: Arc<ApiContext>,
    runtime: tokio::runtime::Handle,
    server: Option<ServerHandle>,
}

impl Api {
    /// The API of `context`, to be served on `runtime` (a runtime of its own, which the node's
    /// binary builds).
    pub fn new(context: Arc<ApiContext>, runtime: tokio::runtime::Handle) -> Api {
        Api {
            context,
            runtime,
            server: None,
        }
    }
}

impl Component for Api {
    const PACKAGE: &'static str = "@solar-network/api";
    const REQUIRED: bool = false;

    fn enabled(&self) -> bool {
        self.context.config.enabled
    }

    async fn boot(&mut self) -> Result<(), BootError> {
        let _ = (&self.runtime, &mut self.server);
        todo!()
    }

    async fn dispose(&mut self) {
        todo!()
    }
}
