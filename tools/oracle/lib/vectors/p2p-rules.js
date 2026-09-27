"use strict";
// Vector classes of the peer protocol's rules, each a builder of records (null while it has none,
// and the class is then not written):
//   exchange  V24-p2p-exchange: every request shape the client writes accepted by
//             p2p.validateRequest; the six ban-trap shapes refused; the version floor at, below
//             and above; the replies of the six routes: valid, each schema rule broken once, and
//             the edges of their caps
//   rules     V25-peer-rules: peer.isValid over every ipaddr.js range, peer.isValidVersion over
//             the devnet's and the default ranges, peer.sameSubnet, and ratelimit.run sequences
//             of each outgoing rule across window edges

module.exports = {
    exchange: async function exchangeVectors() {
        return null;
    },
    rules: async function peerRuleVectors() {
        return null;
    },
};
