"use strict";
// Vector class V13-p2p-wire: the peer-to-peer wire, in four parts, each a builder of records:
//   wire-nes.js       nes frames: parse and write
//   wire-messages.js  the protobuf messages of every codec, with presence and malformed bytes
//   wire-lists.js     the block and transaction lists, the exclude list and the amounts
//   wire-status.js    signed statuses, their verification, and the reply check's rules
// A part returns its records, or null while it has none; the class is not written until a part
// has records. The records' inputs are self-contained (a status record carries its own
// configuration), so the class needs no other class.

const PARTS = [require("./wire-nes"), require("./wire-messages"), require("./wire-lists"), require("./wire-status")];

module.exports = async function wireVectors(ctx) {
    let records = null;
    for (const part of PARTS) {
        const partRecords = await part(ctx);
        if (partRecords !== null) records = [...(records ?? []), ...partRecords];
    }
    return records;
};
