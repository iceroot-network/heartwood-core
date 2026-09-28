#!/usr/bin/env node
"use strict";
// p2p-fuzz-seeds.js: seed the corpora of the p2p fuzz targets from a vector run.
//
//   node tools/p2p-fuzz-seeds.js VECTOR_DIR CORPUS_DIR [MAX_BYTES]
//
// Reads the classes V24-p2p-exchange and V26-network of VECTOR_DIR, when present. Each seed is
// written as CORPUS_DIR/<target>/<SHA-1 of its bytes>, the name libFuzzer gives the files of a
// corpus, so a seed that is already there is not written again. Seeds longer than MAX_BYTES
// (65536 by default) are left out.
//
//   reply_check   the inputs of wire.checkReply: a selector byte (the route's index in ROUTES,
//                 the order of heartwood_p2p_wire::replies::ROUTES) and the reply, or the selected
//                 input as it is when the record names no route
//   listed_block  the getBlocks replies of block.fromListed
//
// The session target (nes_session) reads a script, not a payload, and has no seeds here.

const crypto = require("crypto");
const fs = require("fs");
const path = require("path");

const ROUTES = ["getPeers", "getCommonBlocks", "getBlocks", "postBlock", "getUnconfirmedTransactions", "postTransactions"];
const CLASSES = ["V24-p2p-exchange.jsonl", "V26-network.jsonl"];

function bytes(hex) {
    return typeof hex === "string" && /^([0-9a-fA-F]{2})*$/.test(hex) ? Buffer.from(hex, "hex") : null;
}

/** The seed of a record for its target, or null. */
function seed(record) {
    const input = record.input || {};
    const payload = bytes(input.hex);
    if (payload === null) return null;
    if (record.op === "wire.checkReply") {
        if (input.route === undefined) return { target: "reply_check", data: payload };
        const index = ROUTES.indexOf(input.route);
        return index < 0 ? null : { target: "reply_check", data: Buffer.concat([Buffer.from([index]), payload]) };
    }
    if (record.op === "block.fromListed") return { target: "listed_block", data: payload };
    return null;
}

function main() {
    const [vectorDir, corpusDir, maxArg] = process.argv.slice(2);
    if (!vectorDir || !corpusDir) {
        console.error("usage: p2p-fuzz-seeds.js VECTOR_DIR CORPUS_DIR [MAX_BYTES]");
        process.exit(2);
    }
    const max = maxArg ? Number(maxArg) : 65536;
    const written = { reply_check: 0, listed_block: 0 };
    let tooLong = 0;
    for (const name of CLASSES) {
        const file = path.join(vectorDir, name);
        if (!fs.existsSync(file)) continue;
        for (const line of fs.readFileSync(file, "utf8").split("\n")) {
            if (!line) continue;
            const record = JSON.parse(line);
            if (record.op === "meta") continue;
            const found = seed(record);
            if (found === null) continue;
            if (found.data.length > max) {
                tooLong++;
                continue;
            }
            const dir = path.join(corpusDir, found.target);
            fs.mkdirSync(dir, { recursive: true });
            const target = path.join(dir, crypto.createHash("sha1").update(found.data).digest("hex"));
            if (!fs.existsSync(target)) {
                fs.writeFileSync(target, found.data);
                written[found.target]++;
            }
        }
    }
    console.log(`p2p-fuzz-seeds: new seeds ${JSON.stringify(written)}, ${tooLong} longer than ${max} bytes left out`);
}

main();
