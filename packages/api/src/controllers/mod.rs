//! The controllers: each serves its routes (see each module), as handler functions of the context
//! and the request.

pub mod blocks;
pub mod node;
pub mod peers;
pub mod transactions;
pub mod transactions_pool;
pub mod wallets;

use crate::context::Route;

/// Every served route.
pub fn routes() -> Vec<Route> {
    let mut routes = Vec::new();
    routes.extend(blocks::routes());
    routes.extend(node::routes());
    routes.extend(peers::routes());
    routes.extend(transactions::routes());
    routes.extend(transactions_pool::routes());
    routes.extend(wallets::routes());
    routes
}
