"use strict";
// Oracle operations of the forger's decisions, over the reference's ForgerService with its client
// stubbed:
//   forger.decide  {round, networkState, status, slots, timeLeftInMs, delegate, firstAttempt,
//                  usernames, overHeightHeaders, milestones?} -> {action, reason, lines}: one
//                  attempt of forgeNewBlock and its isForgingAllowed, the relay's answers given by
//                  the input, with every line the service logs (forger-service.ts)
// The internal routes' bodies (the reference's codecs over NetworkState objects built by its own
// class, and getCurrentRound objects over a fixture) are operations of this file too.
// Loaded by lib/ops.js. Until an operation is written it throws, and the harness reports its
// records as having no Rust implementation.

function notWritten(name) {
    return () => {
        throw new Error(`${name} is not written yet`);
    };
}

module.exports = function forgerDecideOps() {
    return {
        "forger.decide": notWritten("forger.decide"),
    };
};
