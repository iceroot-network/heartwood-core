"use strict";
// Vector class V27-processor: the block processor and the job, in two parts, each a builder of
// records:
//   processor.js  chain.process records
//   job.js        chain.job records
// A part returns its records, or null while it has none; the class is not written until a part
// has records.

const PARTS = [require("./processor"), require("./job")];

module.exports = async function chainProcessVectors(ctx) {
    let records = null;
    for (const part of PARTS) {
        const partRecords = await part(ctx);
        if (partRecords !== null) records = [...(records ?? []), ...partRecords];
    }
    return records;
};
