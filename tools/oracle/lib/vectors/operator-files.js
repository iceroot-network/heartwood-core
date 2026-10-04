"use strict";
// Vector class V35-operator: the operator commands' files, joined from the builders of each part:
// the environment file (`env.stringify`, as the reference's envfile writes it; `env.parseCli`, as
// its CLI reads it) and the forger's key file (`config.forgerKey`). Each kind has a minimum record
// count in tools/oracle/vector-minimums.json. The builder returns null while no part has records;
// the class is not written until then.

const PARTS = [require("./operator"), require("./operator-config")];

module.exports = async function operatorFileVectors(ctx) {
    const records = [];
    for (const build of PARTS) records.push(...(await build(ctx)));
    return records.length > 0 ? records : null;
};
