//! Blocks: the header and block codec, ids, the verification rules and the block factory.
//!
//! Blocks are version 0 headers signed with BIP340. The transactions of block H are decoded and
//! checked under the merged milestone at H. A node accepts a block from another node with
//! [`Block::receive`], never with [`Block::decode`] alone (see [`deserialiser`]).

pub mod block;
pub mod deserialiser;
pub mod factory;
pub mod serialiser;

pub use self::block::{Block, BlockHeader, BlockId, Verification, VerifyFailure};
pub use self::factory::BlockDraft;
