"use strict";
// Oracle operations of block assembly, over the reference's forger class:
//   forger.forge            {privateKey, aux, previousBlock: {id, height}, timestamp, reward,
//                           transactions, milestones?} -> {hex, id}: each transaction read with
//                           TransactionFactory.fromBytesUnsafe, then the forger's Delegate.forge
//                           with the given aux (delegate.ts)
//   forger.collateAndForge  {session, privateKey, aux, round} -> {collated, removed, hex, id}: the
//                           pool session's getBlockCandidateTransactions(true, []) (a forger's
//                           collation), then Delegate.forge on the collated transactions
// Loaded by lib/ops.js. Until an operation is written it throws, and the harness reports its
// records as having no Rust implementation.

function notWritten(name) {
    return () => {
        throw new Error(`${name} is not written yet`);
    };
}

module.exports = function forgerForgeOps() {
    return {
        "forger.forge": notWritten("forger.forge"),
        "forger.collateAndForge": notWritten("forger.collateAndForge"),
    };
};
