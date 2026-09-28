"use strict";
// V31, the server's transport: p2p.serveRequest over V24's request shapes and each refusal
// (versions, schemas, internal routes from outside, not ready, unknown route), with the ban
// reasons and `connection: close`; the inbound limiter's sequences (ratelimit.run with
// limiter.inbound). Returns its records, or null while it has none.

module.exports = async function serverVectors() {
    return null;
};
