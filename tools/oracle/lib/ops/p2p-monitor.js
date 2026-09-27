"use strict";
// Oracle operations of the network monitor's decisions and the download path:
//   monitor.networkHeight  {heights} -> {height}
//   monitor.forkDecision   {peers, height} -> {forked, blocksToRollback}: checkNetworkHealth with
//                          the cleanse stubbed
//   monitor.downloadPlan   {peers, networkHeight, from, chunkSize, maxParallel, answers, seed} ->
//                          {requests, blocks, cached, nextChunkSize, order}:
//                          downloadBlocksFromHeight with a stub communicator and a seeded
//                          Math.random, the order it drew recorded
//   block.fromListed       {hex} -> {blocks: [{id, verdict}]}: the getBlocks codec, the reply
//                          check, fromBytesUnsafe and BlockFactory.fromData
// Loaded by lib/ops.js. Until an operation is written it throws, and the harness reports its
// records as having no Rust implementation.

function notWritten(name) {
    return () => {
        throw new Error(`${name} is not written yet`);
    };
}

module.exports = function p2pMonitorOps() {
    return {
        "monitor.networkHeight": notWritten("monitor.networkHeight"),
        "monitor.forkDecision": notWritten("monitor.forkDecision"),
        "monitor.downloadPlan": notWritten("monitor.downloadPlan"),
        "block.fromListed": notWritten("block.fromListed"),
    };
};
