"use strict";
// Oracle operations of the relay's side of its forger, over the reference's internal controller,
// NetworkState and peer controller with their collaborators stubbed:
//   forger.round    {session, now, timestamps?} -> {round, slotNumbers}: getCurrentRound's
//                   answer at `now` in a chain session (controllers/internal.ts), and
//                   getSlotNumber for each of `timestamps`
//   quorum.analyze  {peers, declared, forgerKeys, active, ranks, tip, now, coldStart,
//                   minimumDelegateReach?} -> the NetworkState JSON and canForge:
//                   NetworkState.analyze (network-state.ts) with getDelegatesOnThisNode over the
//                   stubbed delegates.json and forger.json
//   status.serve    {tip, now, peerConfig, registered, forgerKeys} -> {hex, preimage}: getStatus's
//                   answer (controllers/peer.ts) with getForgerDelegates and the key file stubbed
// Loaded by lib/ops.js. Until an operation is written it throws, and the harness reports its
// records as having no Rust implementation.

function notWritten(name) {
    return () => {
        throw new Error(`${name} is not written yet`);
    };
}

module.exports = function relayOps() {
    return {
        "forger.round": notWritten("forger.round"),
        "quorum.analyze": notWritten("quorum.analyze"),
        "status.serve": notWritten("status.serve"),
    };
};
