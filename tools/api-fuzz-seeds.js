#!/usr/bin/env node
"use strict";
// api-fuzz-seeds.js: seed the corpus of the API's query fuzz target from a vector directory.
//
//   node tools/api-fuzz-seeds.js VECTOR_DIR CORPUS_DIR [MAX_BYTES] [CHOICES]
//
// The target reads a route's choice byte, then a raw query string. Every distinct query string a
// record of VECTOR_DIR's classes gives as `input.query` (the queries of the grammar and of the
// pagination, and the listings' queries) is written once for each choice byte from 0 to CHOICES - 1
// (7 by default, the target's routes), as CORPUS_DIR/api_query/<SHA-1 of its bytes>, the name
// libFuzzer gives the files of a corpus, so a seed that is already there is not written again.
// Seeds longer than MAX_BYTES (65536 by default) are left out.

const crypto = require("crypto");
const fs = require("fs");
const path = require("path");

const TARGET = "api_query";

function main(argv) {
    if (argv.length < 2) {
        console.error("usage: node tools/api-fuzz-seeds.js VECTOR_DIR CORPUS_DIR [MAX_BYTES] [CHOICES]");
        return 2;
    }
    const [vectors, corpus] = argv;
    const max = Number(argv[2] || 65536);
    const choices = Number(argv[3] || 7);
    const queries = new Set();
    for (const file of fs.readdirSync(vectors).filter((f) => f.endsWith(".jsonl")).sort()) {
        for (const line of fs.readFileSync(path.join(vectors, file), "utf8").split("\n")) {
            if (line === "") continue;
            const record = JSON.parse(line);
            const query = record.input && record.input.query;
            if (typeof query === "string") queries.add(query);
        }
    }
    const dir = path.join(corpus, TARGET);
    fs.mkdirSync(dir, { recursive: true });
    let written = 0;
    for (const query of queries) {
        for (let choice = 0; choice < choices; choice++) {
            const bytes = Buffer.concat([Buffer.from([choice]), Buffer.from(query, "utf8")]);
            if (bytes.length > max) continue;
            const name = crypto.createHash("sha1").update(bytes).digest("hex");
            const file = path.join(dir, name);
            if (fs.existsSync(file)) continue;
            fs.writeFileSync(file, bytes);
            written++;
        }
    }
    console.log(`api-fuzz-seeds: ${queries.size} queries from ${vectors}, ${written} new seeds in ${dir}`);
    return 0;
}

process.exitCode = main(process.argv.slice(2));
