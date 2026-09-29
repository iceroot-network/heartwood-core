"use strict";
// Oracle operation of the state's index orders, through the reference's own state classes
// (wallets/wallet-index.ts, state-saver.ts, state-loader.ts, state-builder.ts) over a chain session:
//   state.indexOrders  {corpus, heights} -> {orders: [{height, live, saved, rebuild}], notes}
// Each of `live`, `saved` and `rebuild` holds the address, public-key and username index orders
// (allByAddress, allByPublicKey, allByUsername): after the replay session applies the chain to the
// height, after the saved-state round trip at it, and after a StateBuilder rebuild at it; the note
// `rebuildOrder` says whether the rebuild's orders differ from the live ones. It runs the chain
// session's restarts through lib/ops/chain.js and lib/state.js without changing them.
// Not built yet: each operation throws.
// Loaded by lib/ops.js.

module.exports = function stateIndexOps() {
    return {
        "state.indexOrders": async () => {
            throw new Error("state.indexOrders is not built");
        },
    };
};
