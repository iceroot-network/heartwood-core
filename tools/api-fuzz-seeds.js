#!/usr/bin/env node
"use strict";
// api-fuzz-seeds.js: seed the corpus of the API's query fuzz target from a vector directory.
//
//   node tools/api-fuzz-seeds.js VECTOR_DIR CORPUS_DIR [MAX_BYTES] [CHOICES]
//
// The target reads a route's choice byte, then a raw query string. Every distinct query string a
// record of VECTOR_DIR's classes gives as `input.query` (the queries of the grammar and of the
// pagination, and the listings' queries) is written once for each choice byte from 0 to CHOICES - 1
// (27 by default, the target's routes), as CORPUS_DIR/api_query/<SHA-1 of its bytes>, the name
// libFuzzer gives the files of a corpus, so a seed that is already there is not written again.
// The fee statistics' `days` values are written for that route's choice byte (9) as well, and the
// inputs of the vectors' api.fuzzQuery records (`input.hex`, a choice byte and a query) as they are.
// Seeds longer than MAX_BYTES (65536 by default) are left out.

const crypto = require("crypto");
const fs = require("fs");
const path = require("path");

const TARGET = "api_query";

// The route of GET /node/fees in the target's table, and the values of its `days` that the
// vectors' queries do not reach.
const FEES_CHOICE = 9;
const DAYS = ["days=1", "days=30", "days=0", "days=31", "days=1.5", "days=abc", "days=1e1", "days=%2030", "days=1&days=2", "days=1,2", "days=", "days=9007199254740992", "days=-1", "days=+7", "days=07", "page=2"];

function main(argv) {
    if (argv.length < 2) {
        console.error("usage: node tools/api-fuzz-seeds.js VECTOR_DIR CORPUS_DIR [MAX_BYTES] [CHOICES]");
        return 2;
    }
    const [vectors, corpus] = argv;
    const max = Number(argv[2] || 65536);
    const choices = Number(argv[3] || 27);
    const queries = new Set();
    const inputs = new Set();
    for (const file of fs.readdirSync(vectors).filter((f) => f.endsWith(".jsonl")).sort()) {
        for (const line of fs.readFileSync(path.join(vectors, file), "utf8").split("\n")) {
            if (line === "") continue;
            const record = JSON.parse(line);
            const query = record.input && record.input.query;
            if (typeof query === "string") queries.add(query);
            const hex = record.op === "api.fuzzQuery" && record.input && record.input.hex;
            if (typeof hex === "string" && /^([0-9a-f]{2})+$/.test(hex)) inputs.add(hex);
        }
    }
    const dir = path.join(corpus, TARGET);
    fs.mkdirSync(dir, { recursive: true });
    let written = 0;
    const seed = (bytes) => {
        if (bytes.length > max) return;
        const name = crypto.createHash("sha1").update(bytes).digest("hex");
        const file = path.join(dir, name);
        if (fs.existsSync(file)) return;
        fs.writeFileSync(file, bytes);
        written++;
    };
    for (const query of queries) {
        for (let choice = 0; choice < choices; choice++) seed(Buffer.concat([Buffer.from([choice]), Buffer.from(query, "utf8")]));
    }
    for (const days of DAYS) seed(Buffer.concat([Buffer.from([FEES_CHOICE]), Buffer.from(days, "utf8")]));
    for (const hex of inputs) seed(Buffer.from(hex, "hex"));
    console.log(`api-fuzz-seeds: ${queries.size} queries from ${vectors}, ${written} new seeds in ${dir}`);
    return 0;
}

process.exitCode = main(process.argv.slice(2));
