"use strict";
// Vector class V28-node: the node's API grammar and configuration, in two parts, each a builder
// of records:
//   api.js         api.parseQuery and api.paginate records
//   config-app.js  config.pluginOptions and config.loadApp records
// A part returns its records, or null while it has none; the class is not written until a part
// has records.

const PARTS = [require("./api"), require("./config-app")];

module.exports = async function nodeVectors(ctx) {
    let records = null;
    for (const part of PARTS) {
        const partRecords = await part(ctx);
        if (partRecords !== null) records = [...(records ?? []), ...partRecords];
    }
    return records;
};
