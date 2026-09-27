"use strict";
// Oracle operation of the process-blocks job and the enqueueing (the reference's ProcessBlocksJob
// and Blockchain.enqueueBlocks, headless on a chain session):
//   chain.job {session, hexes, now, machineState} -> each block's result, the accepted count, the
//             end (broadcast, fork, clear, fork check), the missed rows written; or
//             {blocks, lastDownloadedHeight} -> the enqueueing's jobs
// and the judge's processor mode (processorJudge): the corpus replayed with one block per job,
// each result written beside its digest line to ts-processor.jsonl.
// Loaded by lib/ops.js. Until they are written they throw.

function notWritten(name) {
    return () => {
        throw new Error(`${name} is not written yet`);
    };
}

module.exports = function jobOps() {
    return {
        "chain.job": notWritten("chain.job"),
    };
};

// replay-chain.js --processor: {solar, corpus, out} -> {ok, blocks, lines}
module.exports.processorJudge = notWritten("the judge's processor mode");
