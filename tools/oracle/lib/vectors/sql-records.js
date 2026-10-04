"use strict";
// The records of kinds sql.round2, sql.avgInt8 and sql.productivity in V34, and the meta record
// sql.meta. They are recorded from PostgreSQL itself: the devnet tooling runs the reference's
// query text over generated row sets on the reference image's PostgreSQL 14, runs it again on the
// toolchain's PostgreSQL 16, fails on any difference between the two, and writes
// sql-records.json beside this file. This builder imports that file; it does not run a database.
// The meta record names both versions, the recording script's SHA-256 and the inputs' SHA-256.

const fs = require("fs");
const path = require("path");

const DATA = path.join(__dirname, "sql-records.json");
const KINDS = new Set(["sql.round2", "sql.avgInt8", "sql.productivity"]);

module.exports = async function sqlRecordVectors(ctx) {
    const { meta, records } = JSON.parse(fs.readFileSync(DATA, "utf8"));
    const height = ctx.shared.height;
    const network = ctx.devnet.info.name;
    const out = [];
    for (const record of records) {
        if (!KINDS.has(record.op)) throw new Error(`sql-records.json: unknown kind ${record.op}`);
        // PostgreSQL's jsonb orders keys by length; the rows are written name first.
        const output = record.op === "sql.productivity" && record.output
            ? { rows: record.output.rows.map((r) => ({ username: r.username, missed: r.missed, productivity: r.productivity })) }
            : record.output;
        out.push({ op: record.op, network, height, input: record.input, ...(output ? { output } : { error: record.error }) });
    }
    const counts = {};
    for (const record of records) counts[record.op] = (counts[record.op] || 0) + 1;
    for (const [kind, count] of Object.entries(meta.inputs.records)) {
        if (counts[kind] !== count) throw new Error(`sql-records.json: ${kind} has ${counts[kind]} records, its meta says ${count}`);
    }
    out.push({ op: "sql.meta", network, height, input: meta, output: { agreed: true } });
    return out;
};
