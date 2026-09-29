"use strict";
// Vector class V31-server: the peer-to-peer server, in two parts, each a builder of records:
//   server.js  the plugin chain's verdicts and the inbound limiter
//   relay.js   the served status (statusServe)
// A part returns its records, or null while it has none; the class is not written until a part
// has records.

const PARTS = [require("./server"), require("./relay").statusServe];

module.exports = async function servingVectors(ctx) {
    let records = null;
    for (const part of PARTS) {
        const partRecords = await part(ctx);
        if (partRecords !== null) records = [...(records ?? []), ...partRecords];
    }
    return records;
};
