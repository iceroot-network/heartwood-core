//! Test support, behind the `difftest` feature: a scripted mock peer that speaks nes through the
//! wire crate, for the client's tests and the node's end-to-end and kill tests, and fakes of the
//! server's neighbours (`server`, with `node`). A release build never has it.

#[cfg(feature = "node")]
pub mod attestation;
pub mod mock_peer;
#[cfg(feature = "node")]
pub mod served;
#[cfg(feature = "node")]
pub mod server;

pub use self::mock_peer::{
    Answer, MockFrame, MockPeer, MockPeerError, Reply, Script, ServedChain, upgrade,
};
