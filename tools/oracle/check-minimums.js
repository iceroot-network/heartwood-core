#!/usr/bin/env node
"use strict";
// check-minimums.js: hold a vector run to the minimum record counts of vector-minimums.json.
//
//   node tools/oracle/check-minimums.js DIR
//
// For each class the file names, DIR must hold the class's file, and each kind (an operation) the
// file lists must have at least its minimum of records. A missing class or a kind below its
// minimum fails the check (exit 1), so an install never takes a class that could pass its tests
// vacuously.

const fs = require("fs");
const path = require("path");

const MINIMUMS = path.join(__dirname, "vector-minimums.json");

function main(argv) {
    if (argv.length !== 1) {
        console.error("usage: node tools/oracle/check-minimums.js DIR");
        return 2;
    }
    const dir = argv[0];
    const minimums = JSON.parse(fs.readFileSync(MINIMUMS, "utf8"));
    let failed = false;
    for (const [className, kinds] of Object.entries(minimums)) {
        const file = path.join(dir, `${className}.jsonl`);
        if (!fs.existsSync(file)) {
            console.error(`check-minimums: ${dir} has no ${className}.jsonl`);
            failed = true;
            continue;
        }
        const counts = {};
        for (const line of fs.readFileSync(file, "utf8").split("\n")) {
            if (line === "") continue;
            const op = JSON.parse(line).op;
            if (op !== "meta") counts[op] = (counts[op] || 0) + 1;
        }
        for (const [kind, minimum] of Object.entries(kinds)) {
            const count = counts[kind] || 0;
            const verdict = count >= minimum ? "ok" : "BELOW ITS MINIMUM";
            console.log(`check-minimums: ${className} ${kind}: ${count} of at least ${minimum}, ${verdict}`);
            if (count < minimum) failed = true;
        }
    }
    return failed ? 1 : 0;
}

process.exitCode = main(process.argv.slice(2));
