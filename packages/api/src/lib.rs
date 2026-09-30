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
//! - [`search`]: the in-memory listings' criteria, pagination and search over the state;
//!   [`productivity`]: the delegates' productivity and the chain's totals over the store;
//!   [`spec`]: the OpenAPI document.
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
pub mod productivity;
pub mod resources;
pub mod schemas;
pub mod search;
pub mod server;
pub mod spec;

use std::sync::Arc;

use heartwood_kernel::errors::BootError;
use heartwood_kernel::lifecycle::Component;

pub use crate::context::{
    ApiContext, ApiReply, ApiRequest, ChainTotals, DelegateProduction, Hundredths, NodeInfo,
    NodeStatus, PeerList, Productivity, ProductivityTable, Route, RouteId, SubmitTransactions,
};
pub use crate::errors::{ApiError, ServeError};
pub use crate::productivity::StoreProductivity;
pub use crate::server::{ApiConfig, ServerHandle};

/// The API component: the server of [`ApiContext`], on a runtime of its own.
///
/// A failure to serve (the address taken, HTTPS asked for) stops the relay, as the reference's
/// server ends its application when it cannot start; the relay's profile may still leave the API
/// out, or `CORE_API_DISABLED` turn it off ([`Component::enabled`]).
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

    /// The reference's log line of the server's `event` (`started`, `stopped`).
    fn log(&self, event: &str) {
        let address = self.context.config.address;
        heartwood_kernel::info!("Public API (HTTP) Server {event} at http://{address}");
    }
}

impl Component for Api {
    const PACKAGE: &'static str = "@solar-network/api";
    const REQUIRED: bool = true;

    fn enabled(&self) -> bool {
        self.context.config.enabled
    }

    async fn boot(&mut self) -> Result<(), BootError> {
        match server::serve(Arc::clone(&self.context), &self.runtime) {
            Ok(server) => self.server = Some(server),
            Err(error) => {
                heartwood_kernel::error!("Failed to start Public API (HTTP) Server!");
                let (package, reason) = (Self::PACKAGE, error.to_string());
                return Err(BootError { package, reason });
            }
        }
        self.log("started");
        Ok(())
    }

    async fn dispose(&mut self) {
        if let Some(server) = self.server.take() {
            server.stop().await;
            self.log("stopped");
        }
    }
}
