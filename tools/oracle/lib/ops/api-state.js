"use strict";
// Oracle operations of the listings over the state, as the reference's DelegateSearchService and
// WalletSearchService build them with the kernel's search services (services/{delegate,wallet}-
// search-service.ts, resources-new/{delegate,wallet}.ts):
//   api.delegatesPage     {fixture, query} -> {status, page} or {status, error}
//   api.walletsPage       {fixture, query, active?, votingFor?} -> {status, page} or {status, error}
//   api.delegateResource  {wallet, supply} -> {resource}
// A fixture holds the wallets with their attributes and the index orders.
// Not built yet: each operation throws.
// Loaded by lib/ops.js.

module.exports = function apiStateOps() {
    return {
        "api.delegatesPage": async () => {
            throw new Error("api.delegatesPage is not built");
        },
        "api.walletsPage": async () => {
            throw new Error("api.walletsPage is not built");
        },
        "api.delegateResource": async () => {
            throw new Error("api.delegateResource is not built");
        },
    };
};
