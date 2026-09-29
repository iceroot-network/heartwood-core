//! `heartwood-p2p-wire`: the peer-to-peer wire codec of Heartwood Core.
//!
//! This crate writes and reads the bytes that nodes exchange on the peer-to-peer port exactly as
//! the reference implementation does, including how it reads malformed bytes, so that both
//! implementations accept the same inputs and decode them to the same values.
//!
//! - [`nes`]: the binary frames of the WebSocket protocol (hello, ping, request, response and
//!   error), with the reference's path normalisation.
//! - [`messages`]: the protobuf messages of the public routes, with field presence: an absent
//!   field and a field present with its default value are different values.
//! - [`codecs`]: the reference's encoder and decoder of each public route, request and response,
//!   with their post-processing (amounts, block and transaction lists, the exclude list), and
//!   [`Amount`].
//! - [`list`]: the length-prefixed list framing and its caps.
//! - [`requests`]: the requests a node's client writes, which cannot take a shape a reference
//!   server bans for.
//! - [`replies`]: the reply schemas of the routes other than getStatus.
//! - [`routes`]: the twelve routes of the reference and Heartwood's two attestation routes, with
//!   their ids, paths, payload limits and codec kinds, and the 14 protobuf codec directions.
//! - [`internal`]: the JSON bodies of the internal routes, the network state and the forging
//!   quorum's counts.
//! - [`status`]: the signed peer status: the tree a relay signs, its canonical JSON, the
//!   attestations, and the checks a node applies to a status it receives.
//! - [`errors`]: the typed errors, each with the JavaScript error class the reference raises.
//!
//! Rules for the whole crate:
//!
//! - Untrusted bytes never cause a panic, and no decoder recurses on the input's nesting.
//! - Every refusal names the JavaScript class of the reference's error where the reference raises
//!   one, so that tests compare classes, never messages.
//! - No I/O and no clock. The nes sessions and the WebSocket transport are not part of the codec:
//!   they are in `heartwood-p2p`.
//!
//! Three inputs, which only a crafted message contains, are read differently from the reference on
//! purpose: a status plugin named `__proto__` (the reference replaces an object's prototype),
//! protobuf groups nested deeper than the reference's stack (the reference overflows), and amounts
//! beyond 128 bits (the reference reads integers of any size, which is unbounded work).
//!
//! Status attestations are the reference's pre-BIP340 Schnorr signatures, through the
//! `legacy-schnorr` feature of `heartwood-crypto`.

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

pub mod codecs;
pub mod errors;
pub mod internal;
mod js;
pub mod list;
pub mod messages;
pub mod nes;
mod proto;
pub mod replies;
pub mod requests;
pub mod routes;
pub mod status;

pub use crate::codecs::Amount;
pub use crate::errors::{
    DecodeError, FrameError, InternalBodyError, PreimageError, ReplyRefused, RequestError,
};
pub use crate::messages::Proto;
pub use crate::routes::{Codec, CodecKind, Direction, RouteId};
