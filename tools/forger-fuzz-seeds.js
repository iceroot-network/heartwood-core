#!/usr/bin/env node
"use strict";
// forger-fuzz-seeds.js: seed the corpus of the forger's internal_body fuzz target from a vector
// run.
//
//   node tools/forger-fuzz-seeds.js VECTOR_DIR CORPUS_DIR [MAX_BYTES]
//
// Reads the class V29-forging of VECTOR_DIR, when present. Each seed is written as
// CORPUS_DIR/internal_body/<SHA-1 of its bytes>, the name libFuzzer gives the files of a corpus,
// so a seed that is already there is not written again. Seeds longer than MAX_BYTES (65536 by
// default) are left out.
//
//   internal_body  a selector byte (0: getCurrentRound's body, 4: getNetworkState's, the order of
//                  fuzz/fuzz_targets/internal_body.rs) and the body's JSON text: the bodies of
//                  internal.currentRound and internal.networkState, and the round and network
//                  state that each forger.decide record's relay answers
//
// The attester's target (status_to_sign) builds its offers from a structured input, not from
// bytes, and has no seeds here.

const crypto = require("crypto");
const fs = require("fs");
const path = require("path");

const CLASS = "V29-forging.jsonl";
const TARGET = "internal_body";
const ROUND = 0;
const NETWORK_STATE = 4;

/** A body as its JSON text: text as it is, any other value written by JSON.stringify. */
function text(body) {
    if (body === undefined || body === null) return null;
    return typeof body === "string" ? body : JSON.stringify(body);
}

/** The seeds of a record: [selector, text] pairs. */
function seeds(record) {
    const input = record.input || {};
    const output = record.output || {};
    switch (record.op) {
        case "internal.currentRound":
            return [[ROUND, text(input.json)]];
        case "internal.networkState":
            return [[NETWORK_STATE, text(input.body !== undefined ? input.body : output.json)]];
        case "forger.decide":
            return [
                [ROUND, text(input.round)],
                [NETWORK_STATE, text(input.networkState)],
            ];
        default:
            return [];
    }
}

function main() {
    const [vectorDir, corpusDir, maxArg] = process.argv.slice(2);
    if (!vectorDir || !corpusDir) {
        console.error("usage: forger-fuzz-seeds.js VECTOR_DIR CORPUS_DIR [MAX_BYTES]");
        process.exit(2);
    }
    const max = maxArg ? Number(maxArg) : 65536;
    const file = path.join(vectorDir, CLASS);
    if (!fs.existsSync(file)) {
        console.log(`forger-fuzz-seeds: no ${CLASS} in ${vectorDir}`);
        return;
    }
    const dir = path.join(corpusDir, TARGET);
    fs.mkdirSync(dir, { recursive: true });
    let written = 0;
    let tooLong = 0;
    for (const line of fs.readFileSync(file, "utf8").split("\n")) {
        if (!line) continue;
        const record = JSON.parse(line);
        if (record.op === "meta") continue;
        for (const [selector, body] of seeds(record)) {
            if (body === null) continue;
            const data = Buffer.concat([Buffer.from([selector]), Buffer.from(body, "utf8")]);
            if (data.length > max) {
                tooLong++;
                continue;
            }
            const target = path.join(dir, crypto.createHash("sha1").update(data).digest("hex"));
            if (!fs.existsSync(target)) {
                fs.writeFileSync(target, data);
                written++;
            }
        }
    }
    console.log(`forger-fuzz-seeds: ${written} new ${TARGET} seeds, ${tooLong} longer than ${max} bytes left out`);
}

main();
