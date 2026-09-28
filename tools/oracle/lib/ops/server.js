"use strict";
// Oracle operations of the server's transport, over the reference's socket server plugins:
//   p2p.serveRequest  {route, hex, ip, remoteAccess, booted, minimumVersions?, milestones?} ->
//                     {status, message, banReason, connectionClose}: the plugin chain's verdict
//                     on a request (whitelist-forger, rate-limit, codec, validate, version,
//                     is-app-ready, accept-peer, close-connection and ban-hammer), the handler
//                     stubbed
// The inbound rate limiter is `ratelimit.run` with `limiter.inbound` (lib/ops/p2p-rules.js).
// Loaded by lib/ops.js. Until an operation is written it throws, and the harness reports its
// records as having no Rust implementation.

function notWritten(name) {
    return () => {
        throw new Error(`${name} is not written yet`);
    };
}

module.exports = function serverOps() {
    return {
        "p2p.serveRequest": notWritten("p2p.serveRequest"),
    };
};
