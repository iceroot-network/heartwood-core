//! `GET /api.json`: the API's OpenAPI document, with the reference's patches at its start (the
//! network's address character, the pool's and the blockchain's options, the version), listing
//! the kept routes and the kept transaction types.

use crate::context::{ApiContext, ApiReply};

/// The document, patched for the node of `context`.
pub fn answer(context: &ApiContext) -> ApiReply {
    let _ = context;
    todo!("the OpenAPI document")
}
