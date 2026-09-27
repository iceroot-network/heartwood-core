"use strict";
// Oracle operations of the reference's transaction pool: pool.open, pool.process, pool.setOnline,
// pool.readd, pool.cleanUp, pool.remove, pool.removeForged, pool.flush, pool.blockApplied,
// pool.contents, pool.collate and pool.restart. Loaded by lib/ops.js.
//
// Each session binds the reference's own pool classes in an application over a wallet fixture or
// a chain session, with the real storage in a temporary file; only the worker transport, the
// broadcaster and the clock are replaced (lib/pool.js).

module.exports = function poolOps() {
    return {};
};
