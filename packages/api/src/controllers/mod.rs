//! The controllers: each serves its routes (see each module), as handler functions of the context
//! and the request.
//!
//! The state's listings (`delegates`, `wallets_list`), the store's history (`delegate_history`,
//! `rounds`, `votes`, and the listing routes of `blocks` and `wallets`) and the chain's routes
//! (`blockchain`, and `/node/fees` and `/transactions/schemas`) are dispatched by
//! [`listing_routes`], one handler per route.

pub mod blockchain;
pub mod blocks;
pub mod delegate_history;
pub mod delegates;
pub mod node;
pub mod peers;
pub mod rounds;
pub mod transactions;
pub mod transactions_pool;
pub mod votes;
pub mod wallets;
pub mod wallets_list;

use crate::context::{Route, RouteId};

/// Every served route.
pub fn routes() -> Vec<Route> {
    let mut routes = Vec::new();
    routes.extend(blocks::routes());
    routes.extend(node::routes());
    routes.extend(peers::routes());
    routes.extend(transactions::routes());
    routes.extend(transactions_pool::routes());
    routes.extend(wallets::routes());
    routes.extend(listing_routes());
    routes
}

/// The routes of the state's listings, the store's history and the chain: each route's handler in
/// its controller.
pub fn listing_routes() -> [Route; 20] {
    [
        Route {
            id: RouteId::BlockchainIndex,
            handler: |ctx, request| Box::pin(blockchain::index(ctx, request)),
        },
        Route {
            id: RouteId::BlockchainSearch,
            handler: |ctx, request| Box::pin(blockchain::search(ctx, request)),
        },
        Route {
            id: RouteId::BlocksIndex,
            handler: |ctx, request| Box::pin(blocks::index(ctx, request)),
        },
        Route {
            id: RouteId::BlocksMissed,
            handler: |ctx, request| Box::pin(blocks::missed(ctx, request)),
        },
        Route {
            id: RouteId::BlocksFirst,
            handler: |ctx, request| Box::pin(blocks::first(ctx, request)),
        },
        Route {
            id: RouteId::DelegatesIndex,
            handler: |ctx, request| Box::pin(delegates::index(ctx, request)),
        },
        Route {
            id: RouteId::DelegatesShow,
            handler: |ctx, request| Box::pin(delegates::show(ctx, request)),
        },
        Route {
            id: RouteId::DelegatesVoters,
            handler: |ctx, request| Box::pin(delegates::voters(ctx, request)),
        },
        Route {
            id: RouteId::DelegatesBlocks,
            handler: |ctx, request| Box::pin(delegate_history::blocks(ctx, request)),
        },
        Route {
            id: RouteId::DelegatesMissedBlocks,
            handler: |ctx, request| Box::pin(delegate_history::missed_blocks(ctx, request)),
        },
        Route {
            id: RouteId::NodeFees,
            handler: |ctx, request| Box::pin(node::fees(ctx, request)),
        },
        Route {
            id: RouteId::RoundsDelegates,
            handler: |ctx, request| Box::pin(rounds::delegates(ctx, request)),
        },
        Route {
            id: RouteId::TransactionsSchemas,
            handler: |ctx, request| Box::pin(transactions::schemas(ctx, request)),
        },
        Route {
            id: RouteId::VotesIndex,
            handler: |ctx, request| Box::pin(votes::index(ctx, request)),
        },
        Route {
            id: RouteId::VotesShow,
            handler: |ctx, request| Box::pin(votes::show(ctx, request)),
        },
        Route {
            id: RouteId::WalletsIndex,
            handler: |ctx, request| Box::pin(wallets_list::index(ctx, request)),
        },
        Route {
            id: RouteId::WalletsTop,
            handler: |ctx, request| Box::pin(wallets_list::top(ctx, request)),
        },
        Route {
            id: RouteId::WalletsTransactionsSent,
            handler: |ctx, request| Box::pin(wallets::sent(ctx, request)),
        },
        Route {
            id: RouteId::WalletsTransactionsReceived,
            handler: |ctx, request| Box::pin(wallets::received(ctx, request)),
        },
        Route {
            id: RouteId::WalletsVotes,
            handler: |ctx, request| Box::pin(wallets::votes(ctx, request)),
        },
    ]
}
