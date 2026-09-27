//! The requests a node's client writes, each with exactly the fields the reference's client
//! writes.
//!
//! A reference server refuses some request shapes and bans the caller's IP for them: no `headers`
//! on getStatus, getPeers, getCommonBlocks or getBlocks; `headers` without a version or with an
//! empty one; `countOnly` on getUnconfirmedTransactions; no ids on getCommonBlocks; a
//! `blockLimit` or `lastBlockHeight` that is absent or 0 on getBlocks. [`ClientRequest`] cannot
//! express any of them: every request carries its [`Headers`], whose version is never empty, a
//! getCommonBlocks request holds 1 to 10 ids ([`CommonBlockIds`]), getBlocks' height and limit
//! are non-zero, and getUnconfirmedTransactions has no `countOnly`. Two more shapes are refused
//! and banned, and the caller keeps them out: a `blockLimit` above 400, and more transactions
//! than the receiver's `maxTransactionsPerRequest` (the peer client asks for at most 400 blocks,
//! and its broadcaster sends batches of at most its own `maxTransactionsPerRequest`, as the
//! reference's does).
//!
//! | Route | Payload |
//! |---|---|
//! | getStatus, getPeers | `{headers}` |
//! | getCommonBlocks | `{ids, headers}`, the ids as lowercase hex |
//! | getBlocks | `{lastBlockHeight, blockLimit, headersOnly (only when true), serialised: true, headers}` |
//! | getUnconfirmedTransactions | `{exclude, headers}`, `exclude` written even when empty |
//! | postBlock | `{block, headers}`: the block's bytes with its transactions |
//! | postTransactions | `{transactions, headers}` |

use std::num::NonZeroU32;

use heartwood_crypto::{BlockId, TransactionId};

use crate::codecs::transactions::{
    PostTransactionsInput, UnconfirmedRequestInput, get_unconfirmed_transactions_encode_request,
    post_transactions_encode_request,
};
use crate::errors::RequestError;
use crate::messages::Proto;
use crate::messages::blocks::{GetBlocksRequest, PostBlockRequest};
use crate::messages::peer::{GetCommonBlocksRequest, GetPeersRequest, GetStatusRequest};
use crate::messages::shared;
use crate::routes::RouteId;

/// The headers of every request: the node's version (never empty) and its peer-to-peer port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Headers {
    version: String,
    port: u32,
}

impl Headers {
    /// The headers of a node of `version` listening on `port`; [`RequestError::EmptyVersion`]
    /// for an empty version.
    pub fn new(version: impl Into<String>, port: u32) -> Result<Headers, RequestError> {
        let version = version.into();
        if version.is_empty() {
            return Err(RequestError::EmptyVersion);
        }
        Ok(Headers { version, port })
    }

    /// The version.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// The peer-to-peer port.
    pub fn port(&self) -> u32 {
        self.port
    }
}

/// The ids of a getCommonBlocks request: 1 to 10 block ids, in the order they are asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommonBlockIds(Vec<BlockId>);

impl CommonBlockIds {
    /// The most ids a reference server accepts in one request.
    pub const MAX: usize = 10;

    /// `ids` as a request's list; [`RequestError::CommonBlockIds`] for none or more than
    /// [`CommonBlockIds::MAX`].
    pub fn new(ids: Vec<BlockId>) -> Result<CommonBlockIds, RequestError> {
        if ids.is_empty() || ids.len() > CommonBlockIds::MAX {
            return Err(RequestError::CommonBlockIds(ids.len()));
        }
        Ok(CommonBlockIds(ids))
    }

    /// The ids.
    pub fn ids(&self) -> &[BlockId] {
        &self.0
    }
}

/// A request of the reference's client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientRequest {
    /// getStatus.
    GetStatus,
    /// getPeers.
    GetPeers,
    /// getCommonBlocks: our block ids at the heights probed.
    GetCommonBlocks {
        /// The ids, written as lowercase hex.
        ids: CommonBlockIds,
    },
    /// getBlocks: the blocks after `last_block_height`, at most `block_limit` of them, always
    /// serialised.
    GetBlocks {
        /// The height after which blocks are wanted.
        last_block_height: NonZeroU32,
        /// The most blocks wanted. A reference server accepts at most 400 (its request schema
        /// refuses more, and bans the caller); the caller keeps to that, as the reference's
        /// client does.
        block_limit: NonZeroU32,
        /// Whether only the headers are wanted; `headersOnly` is written only when true.
        headers_only: bool,
    },
    /// getUnconfirmedTransactions: the pool, without the transactions of `exclude`.
    GetUnconfirmedTransactions {
        /// The ids of transactions the client has.
        exclude: Vec<TransactionId>,
    },
    /// postBlock: a block with its transactions.
    PostBlock {
        /// The block's bytes with its transactions.
        block: Vec<u8>,
    },
    /// postTransactions: a batch of transactions.
    PostTransactions {
        /// The serialised transactions, at most the receiver's `maxTransactionsPerRequest` (40 by
        /// default; its request schema refuses more, and bans the caller).
        transactions: Vec<Vec<u8>>,
    },
}

impl ClientRequest {
    /// The route of the request.
    pub fn route(&self) -> RouteId {
        match self {
            ClientRequest::GetStatus => RouteId::GetStatus,
            ClientRequest::GetPeers => RouteId::GetPeers,
            ClientRequest::GetCommonBlocks { .. } => RouteId::GetCommonBlocks,
            ClientRequest::GetBlocks { .. } => RouteId::GetBlocks,
            ClientRequest::GetUnconfirmedTransactions { .. } => RouteId::GetUnconfirmedTransactions,
            ClientRequest::PostBlock { .. } => RouteId::PostBlock,
            ClientRequest::PostTransactions { .. } => RouteId::PostTransactions,
        }
    }

    /// The request's payload with `headers`, as the reference's client encodes it: the headers'
    /// version and port are always written, and so are the lists, even when they are empty.
    pub fn encode(&self, headers: &Headers) -> Vec<u8> {
        let headers = Some(shared::Headers {
            version: Some(headers.version.clone()),
            port: Some(headers.port),
            ..shared::Headers::default()
        });
        match self {
            ClientRequest::GetStatus => GetStatusRequest {
                headers,
                ..GetStatusRequest::default()
            }
            .encode(),
            ClientRequest::GetPeers => GetPeersRequest {
                headers,
                ..GetPeersRequest::default()
            }
            .encode(),
            ClientRequest::GetCommonBlocks { ids } => GetCommonBlocksRequest {
                ids: ids.ids().iter().map(BlockId::to_hex).collect(),
                headers,
                ..GetCommonBlocksRequest::default()
            }
            .encode(),
            ClientRequest::GetBlocks {
                last_block_height,
                block_limit,
                headers_only,
            } => GetBlocksRequest {
                last_block_height: Some(last_block_height.get()),
                block_limit: Some(block_limit.get()),
                headers_only: headers_only.then_some(true),
                serialised: Some(true),
                headers,
                ..GetBlocksRequest::default()
            }
            .encode(),
            ClientRequest::GetUnconfirmedTransactions { exclude } => {
                get_unconfirmed_transactions_encode_request(&UnconfirmedRequestInput {
                    count_only: None,
                    exclude: exclude.iter().map(|id| *id.as_bytes()).collect(),
                    headers,
                })
            }
            ClientRequest::PostBlock { block } => PostBlockRequest {
                block: Some(block.clone()),
                headers,
                ..PostBlockRequest::default()
            }
            .encode(),
            ClientRequest::PostTransactions { transactions } => {
                post_transactions_encode_request(&PostTransactionsInput {
                    transactions: transactions.clone(),
                    headers,
                })
            }
        }
    }
}
