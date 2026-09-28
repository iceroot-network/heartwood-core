"use strict";
// Vector class V26-network: the peer network, in three parts, each a builder of records:
//   p2p-peers.js     the status attestations of a ping
//   p2p-verifier.js  the common-block search and the peer verifier
//   p2p-monitor.js   the monitor's decisions and a listed block's path to a block
// A part returns its records, or null while it has none; the class is not written until a part
// has records.

const PARTS = [require("./p2p-peers"), require("./p2p-verifier"), require("./p2p-monitor")];

module.exports = async function networkVectors(ctx) {
    let records = null;
    for (const part of PARTS) {
        const partRecords = await part(ctx);
        if (partRecords !== null) records = [...(records ?? []), ...partRecords];
    }
    return records;
};
