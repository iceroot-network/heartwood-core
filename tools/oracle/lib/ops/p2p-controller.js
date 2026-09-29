"use strict";
// Oracle operation of the server's public controllers (socket-server/controllers/{peer,blocks,
// transactions}.ts) run headless over a chain session, a pool session and a peer repository:
//   p2p.controller  {route, request, chain, pool?, peers?, caller, remoteAccess} -> {status,
//                   answer, lines} or {thrown: {name, message}, lines}
// `answer` is the controller's answer written by the route's codec (hex) and `lines` the log lines
// it wrote; a controller that throws (postBlock above the milestone's maxTransactions) is reported
// as `thrown`, never as a status. Not built yet: the operation throws.
// Loaded by lib/ops.js.

module.exports = function p2pControllerOps() {
    return {
        "p2p.controller": async () => {
            throw new Error("p2p.controller is not built");
        },
    };
};
