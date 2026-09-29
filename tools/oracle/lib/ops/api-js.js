"use strict";
// Oracle operations of the wallet search and its collation:
//   api.walletsLike     {wallets, input} -> {wallets}  (the reference's getWalletsLike, sorted)
//   js.localeCompare    {a, b} -> {order}  (a.localeCompare(b, "en", {numeric: true}))
// Not built yet: each operation throws.
// Loaded by lib/ops.js.

module.exports = function apiJsOps() {
    return {
        "api.walletsLike": async () => {
            throw new Error("api.walletsLike is not built");
        },
        "js.localeCompare": async () => {
            throw new Error("js.localeCompare is not built");
        },
    };
};
