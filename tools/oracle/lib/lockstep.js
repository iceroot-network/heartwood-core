"use strict";
// The lockstep session: the reference's side of a chain the producer forges with Heartwood, a
// replay session (lib/state.js) and a reference pool (lib/pool.js) over its state, each block
// applied with its digest line and each transaction built once and submitted to the pool. Used by
// lib/ops/lockstep.js. Not built yet.

function openSession() {
    throw new Error("the lockstep session is not built");
}

module.exports = { openSession };
