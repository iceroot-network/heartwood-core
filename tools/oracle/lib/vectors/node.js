"use strict";
// Vector class V28-node: the node's API grammar and configuration, in two parts, each a builder
// of records:
//   api.js         api.parseQuery and api.paginate records
//   config-app.js  config.pluginOptions and config.loadApp records
// A part returns its records, or null while it has none; the class is not written until a part
// has records.
//
// Then, for each config.pluginOptions case, a config.pluginStart record of the same input: what
// the process does next with those options (it runs, it leaves the component out, or it refuses
// the start). The case that asks for HTTPS is left out: the reference serves HTTPS with the key
// and certificate, where Heartwood refuses the start and points to a reverse proxy, a difference
// the documentation lists.

const PARTS = [require("./api"), require("./config-app")];

/** The pluginOptions cases with no pluginStart record. */
const NO_START = new Set(["api HTTPS asked for"]);

module.exports = async function nodeVectors(ctx) {
    let records = null;
    for (const part of PARTS) {
        const partRecords = await part(ctx);
        if (partRecords !== null) records = [...(records ?? []), ...partRecords];
    }
    if (records === null) return null;
    const starts = [];
    for (const record of records) {
        if (record.op !== "config.pluginOptions" || NO_START.has(record.name)) continue;
        starts.push({ ...ctx.must(await ctx.rec("config.pluginStart", record.height, record.input)), name: record.name });
    }
    return [...records, ...starts];
};
