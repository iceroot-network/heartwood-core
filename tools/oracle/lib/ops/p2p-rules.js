"use strict";
// Oracle operations of the peer protocol's rules, over the reference's own modules:
//   wire.checkReply       {route, hex} or {hex} (the first byte selects the route, byte % 6 over
//                         getPeers, getCommonBlocks, getBlocks, postBlock,
//                         getUnconfirmedTransactions, postTransactions) -> {valid, value} after
//                         the reply schema's changes, or {valid: false, refused: part}
//   p2p.validateRequest   {route, hex} -> the server's decode, version check and request schema:
//                         {accepted, value} or {status: 400, message}
//   peer.isValid          {ip, allowPrivate, interfaces} -> {valid, processed} (isValidPeer)
//   peer.isValidVersion   {version, ranges, networkName} -> {valid} (isValidVersion)
//   peer.sameSubnet       {ip, peers} -> {same} (getSameSubnetPeers)
//   ratelimit.run         {limiter, events: [{t, ip, route, op}]} -> the answer to each event
//                         (rate-limiter-flexible under a patched Date.now)
// Loaded by lib/ops.js. Until an operation is written it throws, and the harness reports its
// records as having no Rust implementation.

function notWritten(name) {
    return () => {
        throw new Error(`${name} is not written yet`);
    };
}

module.exports = function p2pRulesOps() {
    return {
        "wire.checkReply": notWritten("wire.checkReply"),
        "p2p.validateRequest": notWritten("p2p.validateRequest"),
        "peer.isValid": notWritten("peer.isValid"),
        "peer.isValidVersion": notWritten("peer.isValidVersion"),
        "peer.sameSubnet": notWritten("peer.sameSubnet"),
        "ratelimit.run": notWritten("ratelimit.run"),
    };
};
