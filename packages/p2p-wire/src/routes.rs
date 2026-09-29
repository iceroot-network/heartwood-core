//! The routes of the peer-to-peer port: their ids, paths, payload limits and codecs.
//!
//! A nes request names its route by id (`p2p.peer.getStatus`). The seven public routes encode
//! their payloads with protobuf, in both directions ([`Codec`]); the five internal routes, which the
//! reference serves only to its own forger and to the addresses it allows, use JSON. Heartwood adds
//! two internal JSON routes that a reference relay never serves: the status attestations its
//! forger signs on request ([`RouteId::is_reference`]).

/// A route of the peer-to-peer port.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RouteId {
    /// `p2p.peer.getPeers`: the peers a node knows.
    GetPeers,
    /// `p2p.peer.getCommonBlocks`: the highest block of a list of ids that the node has.
    GetCommonBlocks,
    /// `p2p.peer.getStatus`: the node's signed status.
    GetStatus,
    /// `p2p.blocks.getBlocks`: blocks after a height.
    GetBlocks,
    /// `p2p.blocks.postBlock`: a new block.
    PostBlock,
    /// `p2p.transactions.getUnconfirmedTransactions`: the pool's transactions, for a forger.
    GetUnconfirmedTransactions,
    /// `p2p.transactions.postTransactions`: transactions for the pool.
    PostTransactions,
    /// `p2p.internal.emitEvent`: an event from the forger.
    EmitEvent,
    /// `p2p.internal.getCurrentRound`: the round, for the forger.
    GetCurrentRound,
    /// `p2p.internal.getSlotNumber`: the slot at a time, for the forger.
    GetSlotNumber,
    /// `p2p.internal.getNetworkState`: the network's view of the chain, for the forger.
    GetNetworkState,
    /// `p2p.internal.syncBlockchain`: a request to download blocks.
    SyncBlockchain,
    /// `p2p.internal.getStatusToSign`: the forger's long poll for a status to attest (Heartwood
    /// only).
    GetStatusToSign,
    /// `p2p.internal.postStatusSignatures`: the forger's attestations of an offered status
    /// (Heartwood only).
    PostStatusSignatures,
}

/// How a route's payloads are encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CodecKind {
    /// Protobuf messages, with the post-processing of [`crate::codecs`].
    Protobuf,
    /// `JSON.stringify` and `JSON.parse` of the payload.
    Json,
}

/// The server's payload limit of `postBlock` and `postTransactions`: 20 MiB, the limit of the
/// server's WebSocket.
const MAX_PAYLOAD: u32 = 20 * 1024 * 1024;

/// The id, the HTTP path and the payload limit of each route, in the order of [`RouteId::ALL`].
const ROUTES: [(&str, &str, Option<u32>); 14] = [
    ("p2p.peer.getPeers", "/p2p/peer/getPeers", Some(1024)),
    (
        "p2p.peer.getCommonBlocks",
        "/p2p/peer/getCommonBlocks",
        Some(10 * 1024),
    ),
    ("p2p.peer.getStatus", "/p2p/peer/getStatus", Some(1024)),
    ("p2p.blocks.getBlocks", "/p2p/blocks/getBlocks", Some(1024)),
    (
        "p2p.blocks.postBlock",
        "/p2p/blocks/postBlock",
        Some(MAX_PAYLOAD),
    ),
    (
        "p2p.transactions.getUnconfirmedTransactions",
        "/p2p/transactions/getUnconfirmedTransactions",
        Some(512 * 1024),
    ),
    (
        "p2p.transactions.postTransactions",
        "/p2p/transactions/postTransactions",
        Some(MAX_PAYLOAD),
    ),
    ("p2p.internal.emitEvent", "/p2p/internal/emitEvent", None),
    (
        "p2p.internal.getCurrentRound",
        "/p2p/internal/getCurrentRound",
        None,
    ),
    (
        "p2p.internal.getSlotNumber",
        "/p2p/internal/getSlotNumber",
        None,
    ),
    (
        "p2p.internal.getNetworkState",
        "/p2p/internal/getNetworkState",
        None,
    ),
    (
        "p2p.internal.syncBlockchain",
        "/p2p/internal/syncBlockchain",
        None,
    ),
    (
        "p2p.internal.getStatusToSign",
        "/p2p/internal/getStatusToSign",
        None,
    ),
    (
        "p2p.internal.postStatusSignatures",
        "/p2p/internal/postStatusSignatures",
        None,
    ),
];

impl RouteId {
    /// Every route: the seven public ones, then the five internal ones of the reference, in the
    /// reference's order, then Heartwood's two attestation routes.
    pub const ALL: [RouteId; 14] = [
        RouteId::GetPeers,
        RouteId::GetCommonBlocks,
        RouteId::GetStatus,
        RouteId::GetBlocks,
        RouteId::PostBlock,
        RouteId::GetUnconfirmedTransactions,
        RouteId::PostTransactions,
        RouteId::EmitEvent,
        RouteId::GetCurrentRound,
        RouteId::GetSlotNumber,
        RouteId::GetNetworkState,
        RouteId::SyncBlockchain,
        RouteId::GetStatusToSign,
        RouteId::PostStatusSignatures,
    ];

    /// The route's row of the table.
    fn row(self) -> (&'static str, &'static str, Option<u32>) {
        ROUTES.get(self as usize).copied().unwrap_or(("", "", None))
    }

    /// The route id, which a nes request carries as its path: `p2p.peer.getStatus`.
    pub fn id(self) -> &'static str {
        self.row().0
    }

    /// The HTTP path the reference registers the route under: `/p2p/peer/getStatus`.
    pub fn path(self) -> &'static str {
        self.row().1
    }

    /// The name of the route's codec in the reference, the last part of the id: `getStatus`.
    pub fn name(self) -> &'static str {
        let id = self.id();
        id.rsplit_once('.').map_or(id, |(_, name)| name)
    }

    /// The server's payload limit for requests of this route, in bytes; `None` where the route
    /// sets none (the internal routes) and only the socket's limit applies.
    pub fn max_bytes(self) -> Option<u32> {
        self.row().2
    }

    /// Whether the route is internal: the reference serves it only to its own forger and to the
    /// addresses it allows.
    pub fn is_internal(self) -> bool {
        self.max_bytes().is_none()
    }

    /// Whether a reference relay serves the route: every route but the two attestation routes,
    /// which only a Heartwood relay serves to its forger.
    pub fn is_reference(self) -> bool {
        !matches!(
            self,
            RouteId::GetStatusToSign | RouteId::PostStatusSignatures
        )
    }

    /// How the route's payloads are encoded.
    pub fn kind(self) -> CodecKind {
        if self.is_internal() {
            CodecKind::Json
        } else {
            CodecKind::Protobuf
        }
    }

    /// The route whose id is `id` (without a leading `/`).
    pub fn from_id(id: &str) -> Option<RouteId> {
        RouteId::ALL.into_iter().find(|route| route.id() == id)
    }
}

/// The direction of a payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    /// From the client to the server.
    Request,
    /// From the server back to the client.
    Response,
}

impl Direction {
    /// The reference's name of the direction in a codec: `request` or `response`.
    pub const fn name(self) -> &'static str {
        match self {
            Direction::Request => "request",
            Direction::Response => "response",
        }
    }
}

/// One direction of a public route's codec: the 14 protobuf payload formats of the port.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Codec {
    /// The getPeers request.
    GetPeersRequest,
    /// The getPeers response.
    GetPeersResponse,
    /// The getCommonBlocks request.
    GetCommonBlocksRequest,
    /// The getCommonBlocks response.
    GetCommonBlocksResponse,
    /// The getStatus request.
    GetStatusRequest,
    /// The getStatus response.
    GetStatusResponse,
    /// The getBlocks request.
    GetBlocksRequest,
    /// The getBlocks response.
    GetBlocksResponse,
    /// The postBlock request.
    PostBlockRequest,
    /// The postBlock response.
    PostBlockResponse,
    /// The getUnconfirmedTransactions request.
    GetUnconfirmedTransactionsRequest,
    /// The getUnconfirmedTransactions response.
    GetUnconfirmedTransactionsResponse,
    /// The postTransactions request.
    PostTransactionsRequest,
    /// The postTransactions response.
    PostTransactionsResponse,
}

impl Codec {
    /// Every codec, in route order with the request before the response, which is also the
    /// declaration order ([`Codec::route`] and [`Codec::direction`] read it). [`Codec::select`]
    /// picks from this list.
    pub const ALL: [Codec; 14] = [
        Codec::GetPeersRequest,
        Codec::GetPeersResponse,
        Codec::GetCommonBlocksRequest,
        Codec::GetCommonBlocksResponse,
        Codec::GetStatusRequest,
        Codec::GetStatusResponse,
        Codec::GetBlocksRequest,
        Codec::GetBlocksResponse,
        Codec::PostBlockRequest,
        Codec::PostBlockResponse,
        Codec::GetUnconfirmedTransactionsRequest,
        Codec::GetUnconfirmedTransactionsResponse,
        Codec::PostTransactionsRequest,
        Codec::PostTransactionsResponse,
    ];

    /// The route of this codec.
    pub fn route(self) -> RouteId {
        RouteId::ALL
            .get(self as usize / 2)
            .copied()
            .unwrap_or(RouteId::GetPeers)
    }

    /// The direction of this codec.
    pub fn direction(self) -> Direction {
        if (self as usize).is_multiple_of(2) {
            Direction::Request
        } else {
            Direction::Response
        }
    }

    /// The codec of `direction` of `route`; `None` for an internal route, which uses JSON.
    pub fn of(route: RouteId, direction: Direction) -> Option<Codec> {
        Codec::ALL
            .into_iter()
            .find(|codec| codec.route() == route && codec.direction() == direction)
    }

    /// The codec that a fuzz input's first byte selects: entry `byte % 14` of [`Codec::ALL`].
    pub fn select(byte: u8) -> Codec {
        Codec::ALL
            .into_iter()
            .cycle()
            .nth(usize::from(byte))
            .unwrap_or(Codec::GetPeersRequest)
    }
}

#[cfg(test)]
mod tests;
