"use strict";
// V29, the forger's decisions: forger.decide over every branch and line of forgeNewBlock and
// isForgingAllowed, an over-height header by the delegate's own key empty and not empty; and the
// internal bodies of NetworkState (every status, over-height headers, absent fields) and
// getCurrentRound. Null while it has no records.

module.exports = async function forgerDecideVectors() {
    return null;
};
