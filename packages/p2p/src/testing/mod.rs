//! Test support, behind the `difftest` feature: a scripted mock peer that speaks nes through the
//! wire crate, for the client's tests and the node's end-to-end and kill tests. A release build
//! never has it.

pub mod mock_peer;

pub use self::mock_peer::{
    MockFrame, MockPeer, MockPeerError, Reply, Script, ServedChain, upgrade,
};
