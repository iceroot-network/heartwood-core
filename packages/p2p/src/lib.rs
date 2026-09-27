//! `heartwood-p2p`: the peer-to-peer network of Heartwood Core, client side.
//!
//! A node follows the network as the reference's relay does: it connects to peers, verifies
//! their chains, downloads and broadcasts blocks and transactions, and keeps its peer list, with
//! the reference's requests, timeouts, limits, cadences and decisions. It opens no peer-to-peer
//! listening socket.
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
//! The client reaches the node only through [`view`]'s traits, so it depends on no crate of the
//! chain, the store or the pool. Every reply is untrusted input: it never causes a panic, and
//! arithmetic on it is checked.
//!
//! Features: `node` (on by default) builds everything but [`nes`], [`transport`] and [`errors`],
//! which a forger's link to its relay needs and which compile without it; `heartwood-forger`
//! depends on the crate without `node`. `difftest` adds the `testing` module (the mock peer) and a
//! fixed sampling seed, test seams that only test code enables; a release build never has them.
//! The `testing` module compiles without `node`.

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
pub mod broadcaster;
#[cfg(feature = "node")]
pub mod chunk_cache;
#[cfg(feature = "node")]
pub mod communicator;
#[cfg(feature = "node")]
pub mod connector;
#[cfg(feature = "node")]
pub mod diversity;
#[cfg(feature = "node")]
pub mod download;
pub mod errors;
#[cfg(feature = "node")]
pub mod monitor;
pub mod nes;
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
pub use crate::options::P2pOptions;
#[cfg(feature = "node")]
pub use crate::peer::{FastVerification, Peer, PeerState, SharedPeer, Verification};
#[cfg(feature = "node")]
pub use crate::processor::{AcceptOptions, PeerProcessor, ProcessorParts};
#[cfg(feature = "node")]
pub use crate::repository::PeerRepository;
#[cfg(feature = "node")]
pub use crate::verifier::PeerVerifier;
#[cfg(feature = "node")]
pub use crate::view::{BlockLookup, ChainView, OnlineSink, PeerEvents};
