"use strict";
// Vector class V34-api-tiers: the API's listings over the state and the store, joined from the
// builders of each part: the state's index orders, the listings, the pagination and its tree, the
// wallet search, the SQL records and the chain routes' documents. Each kind has a minimum record
// count in tools/oracle/vector-minimums.json, which the install step and the crates' vector tests
// hold. The builder returns null while no part has records; the class is not written until then.

const PARTS = [
    require("./state-index"),
    require("./api-state"),
    require("./api-page"),
    require("./api-js"),
    require("./sql-records"),
    require("./api-spec"),
];

module.exports = async function apiTierVectors(ctx) {
    const records = [];
    for (const build of PARTS) records.push(...(await build(ctx)));
    return records.length > 0 ? records : null;
};
