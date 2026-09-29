"use strict";
// Oracle operations of the lockstep producer: a session over the chain session's replay
// (lib/state.js's ReplaySession) with a reference pool built as the pool sessions build it
// (lib/pool.js), over the session's state (lib/lockstep.js):
//   lockstep.open    {seed, profile} -> {session}
//   lockstep.build   {session, height, build} -> {transactions: [{id, hex}]}  (built once, with
//                    the fixed aux and nonces from the session's state)
//   lockstep.apply   {session, hex} -> {verdict, digest}
//   lockstep.submit  {session, transactions} -> {accept, errors}
//   lockstep.online  {session, round, delegates} -> {online}
//   lockstep.close   {session} -> {}
// Not built yet: each operation throws.
// Loaded by lib/ops.js.

module.exports = function lockstepOps() {
    return {
        "lockstep.open": async () => {
            throw new Error("lockstep.open is not built");
        },
        "lockstep.build": async () => {
            throw new Error("lockstep.build is not built");
        },
        "lockstep.apply": async () => {
            throw new Error("lockstep.apply is not built");
        },
        "lockstep.submit": async () => {
            throw new Error("lockstep.submit is not built");
        },
        "lockstep.online": async () => {
            throw new Error("lockstep.online is not built");
        },
        "lockstep.close": async () => {
            throw new Error("lockstep.close is not built");
        },
    };
};
