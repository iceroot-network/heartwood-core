"use strict";
// Oracle operations of the reference's route codecs (packages/p2p/dist/socket-server/codecs):
//   wire.encode          {route, direction, value} -> {hex}; route is the codec's name
//                        (getStatus), direction is request or response, value has present keys
//                        only, with bytes and transactions as hex and amounts as decimal text
//   wire.decode          {route, direction, hex}   -> {value}: own keys in their order, Buffers
//                        as hex, BigNumbers as decimal text; or the reference's error
//   wire.decodeSelected  {hex}                     -> {route, direction, value}: the first byte
//                        modulo 14 selects the codec, in route order with the request first
//                        (getPeers, getCommonBlocks, getStatus, getBlocks, postBlock,
//                        getUnconfirmedTransactions, postTransactions), 0 for an empty input, and
//                        the rest is the payload; for the fuzz replay
// An output may carry `notes` (not compared) with what the value cannot show: wideAmount when a
// decoded amount lies outside the signed 128-bit range, stackOverflow for the Node stack's
// RangeError. Loaded by lib/ops.js; none is defined yet.

module.exports = function wireCodecOps(context) {
    void context;
    return {};
};
