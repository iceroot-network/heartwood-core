//! `heartwood-p2p`: the peer-to-peer network of Heartwood Core, client and server side.
//!
//! A node follows the network as the reference's relay does: it connects to peers, verifies
//! their chains, downloads and broadcasts blocks and transactions, and keeps its peer list, with
//! the reference's requests, timeouts, limits, cadences and decisions; and it serves the peer
//! routes and its forger's internal routes.
//!
//! The client side:
//!
//! - [`nes`], [`transport`] and [`connector`]: the nes client session on a WebSocket, one per peer
//!   IP.
//! - [`peer`] and [`repository`]: peers with the reference's fields and rules, keyed by IP in
//!   insertion order.
//! - [`communicator`]: every request, the outgoing rate limiter, the reply checks, the error
//!   counting and the disconnect rules; [`attestation`]: the status attestations of a ping.
//! - [`processor`]: the acceptance of candidate peers; [`scoring`]: the protocol violations.
//! - [`verifier`] and [`nsect`]: the verification of a peer's claimed chain.
//! - [`monitor`]: discovery, cleansing, the network height, fork health, downloads and
//!   broadcasts, with [`chunk_cache`], [`download`], [`broadcaster`], [`seeds`] and
//!   [`diversity`].
//! - [`rate_limiter`] and [`utils`]: the rate limiter, the address rules, the version gate and
//!   the subnet test.
//! - [`view`]: what the client reads of the node and tells it; [`seams`]: the traits between the
//!   parts; [`options`] and [`errors`].
//!
//! The server side:
//!
//! - [`server`]: the listener, the nes server session, the ban hammer, the plugin chain, the
//!   inbound limits and caps, the request checks, and the route handlers.
//! - [`status_server`]: the served status and its cache; [`attestation_session`]: the forger's
//!   attestation session and the delegates on this node; [`network_state`]: the network state
//!   the forger reads.
//! - [`node_services`]: what the server reads of the node and the peer side, as traits.
//! - [`devnet`]: a devnet node's addressing.
//!
//! Both sides reach the node only through traits ([`view`], [`node_services`]), so the crate
//! depends on no crate of the chain, the store or the pool. Every reply and request is untrusted
//! input: it never causes a panic, and arithmetic on it is checked.
//!
//! Features: `node` (on by default) builds everything but [`nes`], [`transport`] and [`errors`],
//! which a forger's link to its relay needs and which compile without it; `heartwood-forger`
//! depends on the crate without `node`. `devnet` (on by default) reads a devnet node's addressing
//! from the options. `difftest` adds the `testing` module (the mock peer) and a fixed sampling
//! seed, test seams that only test code enables; a release build never has them. The `testing`
//! module compiles without `node`.

#![forbid(unsafe_code)]
#![warn(missing_docs)]
// The crate's documentation names the `node` modules, which a build without it does not have.
#![cfg_attr(not(feature = "node"), allow(rustdoc::broken_intra_doc_links))]
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::arithmetic_side_effects
    )
)]

#[cfg(feature = "node")]
pub mod attestation;
#[cfg(feature = "node")]
pub mod attestation_session;
#[cfg(feature = "node")]
pub mod broadcaster;
#[cfg(feature = "node")]
pub mod chunk_cache;
#[cfg(feature = "node")]
pub mod communicator;
#[cfg(feature = "node")]
pub mod connector;
#[cfg(feature = "node")]
pub mod devnet;
#[cfg(feature = "node")]
pub mod diversity;
#[cfg(feature = "node")]
pub mod download;
pub mod errors;
#[cfg(feature = "node")]
pub mod monitor;
pub mod nes;
#[cfg(feature = "node")]
pub mod network_state;
#[cfg(feature = "node")]
pub mod node_services;
#[cfg(feature = "node")]
pub mod nsect;
#[cfg(feature = "node")]
pub mod options;
#[cfg(feature = "node")]
pub mod peer;
#[cfg(feature = "node")]
pub mod processor;
#[cfg(feature = "node")]
pub mod rate_limiter;
#[cfg(feature = "node")]
pub mod repository;
#[cfg(feature = "node")]
pub mod scoring;
#[cfg(feature = "node")]
pub mod seams;
#[cfg(feature = "node")]
pub mod seeds;
#[cfg(feature = "node")]
pub mod server;
#[cfg(feature = "node")]
pub mod status_server;
#[cfg(feature = "difftest")]
pub mod testing;
pub mod transport;
#[cfg(feature = "node")]
pub mod utils;
#[cfg(feature = "node")]
pub mod verifier;
#[cfg(feature = "node")]
pub mod view;

#[cfg(feature = "node")]
pub use crate::communicator::{BlocksRequest, Communicator, CommunicatorParts};
#[cfg(feature = "node")]
pub use crate::connector::Connector;
#[cfg(feature = "node")]
pub use crate::download::{DownloadedBlock, to_block};
#[cfg(feature = "node")]
pub use crate::errors::PingError;
pub use crate::errors::SessionError;
#[cfg(feature = "node")]
pub use crate::monitor::{MonitorParts, NetworkMonitor};
#[cfg(feature = "node")]
pub use crate::node_services::{AttestedKeys, NoAttestedKeys, NodeServices, PeerSide};
#[cfg(feature = "node")]
pub use crate::options::P2pOptions;
#[cfg(feature = "node")]
pub use crate::peer::{FastVerification, Peer, PeerState, SharedPeer, Verification};
#[cfg(feature = "node")]
pub use crate::processor::{AcceptOptions, PeerProcessor, ProcessorParts};
#[cfg(feature = "node")]
pub use crate::repository::PeerRepository;
#[cfg(feature = "node")]
pub use crate::server::{P2pServer, ServerOptions, ServerParts};
#[cfg(feature = "node")]
pub use crate::verifier::PeerVerifier;
#[cfg(feature = "node")]
pub use crate::view::{BlockLookup, ChainView, OnlineSink, PeerEvents};
