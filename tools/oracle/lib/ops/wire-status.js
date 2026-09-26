"use strict";
// Oracle operations of the signed peer status, recomposed from the reference's own functions in
// the order of its getStatus controller and of its peer communicator's ping:
//   status.sign    {block, forgingAllowed, currentSlot, config, keys} -> {preimage, hex}: the tree
//                  of the block's getHeader(false) and the configuration, Utils.stringify, one
//                  Hash.signSchnorr per key (repeats allowed), and the codec's bytes
//   status.verify  {hex, maxDelegates?} -> {valid, value, preimage, preimageNegativeSlot,
//                  verified}: the codec's decode, the reply schema, both preimages (the verifier's
//                  own slot not negative, then negative), and verifySchnorr per pair
// An output may carry `notes` (not compared): pluginsPrototype when the decoded plugins object
// has a prototype other than Object.prototype, wideAmount and stackOverflow as for the codecs.
// Loaded by lib/ops.js; none is defined yet.

module.exports = function wireStatusOps(context) {
    void context;
    return {};
};
