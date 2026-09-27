"use strict";
// Vector class V29-forging: the forger, in two parts, each a builder of records:
//   forger-forge.js   block assembly from its inputs and from a pool session's collation
//   forger-decide.js  the forger's decisions in a slot, and the internal routes' bodies
// A part returns its records, or null while it has none; the class is not written until a part
// has records.

const PARTS = [require("./forger-forge"), require("./forger-decide")];

module.exports = async function forgingVectors(ctx) {
    let records = null;
    for (const part of PARTS) {
        const partRecords = await part(ctx);
        if (partRecords !== null) records = [...(records ?? []), ...partRecords];
    }
    return records;
};
