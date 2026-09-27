"use strict";
// Oracle operation of the block processor (lib/process.js):
//   chain.process {session, hex, now} -> {result, unchained?, line?}: the reference's
//                 BlockProcessor.process of the block on the session's chain at `now`, its
//                 unchained status when the block did not chain, and the digest line after an
//                 accepted block
// Loaded by lib/ops.js. Until the operation is written it throws, and the harness reports its
// records as having no Rust implementation.

function notWritten(name) {
    return () => {
        throw new Error(`${name} is not written yet`);
    };
}

module.exports = function processOps() {
    return {
        "chain.process": notWritten("chain.process"),
    };
};
