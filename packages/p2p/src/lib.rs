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
//! Features: `difftest` adds the `testing` module (the mock peer) and a fixed sampling seed, test
//! seams that only test code enables; a release build never has them.

#![forbid(unsafe_code)]
#![warn(missing_docs)]
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

pub mod attestation;
pub mod broadcaster;
pub mod chunk_cache;
pub mod communicator;
pub mod connector;
pub mod diversity;
pub mod download;
pub mod errors;
pub mod monitor;
pub mod nes;
pub mod nsect;
pub mod options;
pub mod peer;
pub mod processor;
pub mod rate_limiter;
pub mod repository;
pub mod scoring;
pub mod seams;
pub mod seeds;
#[cfg(feature = "difftest")]
pub mod testing;
pub mod transport;
pub mod utils;
pub mod verifier;
pub mod view;

pub use crate::communicator::{BlocksRequest, Communicator, CommunicatorParts};
pub use crate::connector::Connector;
pub use crate::download::{DownloadedBlock, to_block};
pub use crate::errors::{PingError, SessionError};
pub use crate::monitor::{MonitorParts, NetworkMonitor};
pub use crate::options::P2pOptions;
pub use crate::peer::{FastVerification, Peer, PeerState, SharedPeer, Verification};
pub use crate::processor::{AcceptOptions, PeerProcessor, ProcessorParts};
pub use crate::repository::PeerRepository;
pub use crate::verifier::PeerVerifier;
pub use crate::view::{BlockLookup, ChainView, OnlineSink, PeerEvents};
