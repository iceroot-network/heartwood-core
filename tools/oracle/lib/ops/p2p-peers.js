"use strict";
// Oracle operation of a ping's status attestations, over the reference's peer communicator:
//   peer.attest   {status, peers, lastBlock, slot, delegates} -> {attributed, versions, moved}:
//                 the signature loop of ping, with the other peers' keys, forks and headers
// Loaded by lib/ops.js. Until it is written it throws, and the harness reports its records as
// having no Rust implementation.

module.exports = function p2pPeersOps() {
    return {
        "peer.attest": () => {
            throw new Error("peer.attest is not written yet");
        },
    };
};
