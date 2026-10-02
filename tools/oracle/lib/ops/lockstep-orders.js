"use strict";
// Oracle operation of a lockstep session's index orders (lib/lockstep.js), for the listings
// replay, which runs a node's pool traffic and blocks through the session and reads the orders
// after each step:
//
//   lockstep.indexOrders {session} -> {height, addresses, publicKeys, usernames}: the keys of the
//       address, public-key and username indexes of the session's state, in their order
//       (allByAddress, allByPublicKey, allByUsername), after the handlers' deferred re-indexing
//       has run (setImmediate)
//
// Loaded by lib/ops.js.

const { sessions: registry } = require("../pool");
const { orders } = require("./state-index");

module.exports = function lockstepOrdersOps() {
    return {
        "lockstep.indexOrders": async ({ session: id }) => {
            const s = registry.get(id);
            if (!s || s.kind !== "chain" || !s.replay.chain.pool) throw new Error(`session ${id} is not a lockstep session`);
            // The handlers re-index each party in a setImmediate of its own: let every one queued
            // so far run first.
            await new Promise((resolve) => setImmediate(resolve));
            const chain = s.replay.chain;
            return { height: chain.lastBlock().data.height, ...orders(chain) };
        },
    };
};
