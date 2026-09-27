"use strict";
// Oracle operations of the peer verifier, over the reference's own classes:
//   nsect.find            {n, low, high, common} -> {probes: [[height, ...]], result}: the
//                         kernel's NSect with a probe that answers from `common`
//   verifier.checkState   {session, peer, claimed, fast} -> {result, probes, fetched}: the
//                         reference's PeerVerifier with a stub communicator serving the peer's
//                         chain (a corpus prefix and an optional branch)
// Loaded by lib/ops.js. Until an operation is written it throws, and the harness reports its
// records as having no Rust implementation.

function notWritten(name) {
    return () => {
        throw new Error(`${name} is not written yet`);
    };
}

module.exports = function p2pVerifierOps() {
    return {
        "nsect.find": notWritten("nsect.find"),
        "verifier.checkState": notWritten("verifier.checkState"),
    };
};
