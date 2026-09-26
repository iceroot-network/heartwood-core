"use strict";
// Oracle operations over the reference's own state machine, run headless in this process
// (lib/state.js): state.digest and state.applyTransaction on wallet fixtures, and the chain
// sessions chain.open, chain.apply, chain.revert, chain.dump and chain.close. Loaded by
// lib/ops.js; none is defined yet.

module.exports = function chainOps(context) {
    void context;
    return {};
};
