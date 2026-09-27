"use strict";
// The reference's block processor, its process-blocks job and its enqueueing, run headless on a
// chain session (lib/corpus.js's ReplaySession), as the patch tests run the reference's classes:
// Object.create of the real BlockProcessor, ProcessBlocksJob and Blockchain with stubbed
// collaborators. Only storage, time and the peer transport are stubbed; every rule the processor
// and the job apply is the reference's code.
//
//   makeProcessor(solar, session)  -> {process(block, now) -> {result, unchained?, line?}}
//
// Each function cites the reference lines it follows. Until it is written it throws, and the
// harness reports the records as having no Rust implementation.

function notWritten(name) {
    return () => {
        throw new Error(`${name} is not written yet`);
    };
}

module.exports = { makeProcessor: notWritten("makeProcessor") };
