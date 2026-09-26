#!/usr/bin/env node
"use strict";
// wire-fuzz-seeds.js: seed the corpora of the wire fuzz targets from the wire vector class.
//
//   node tools/wire-fuzz-seeds.js VECTOR_FILE CORPUS_DIR [MAX_BYTES]
//
// Each seed is written as CORPUS_DIR/<target>/<SHA-1 of its bytes>, the name libFuzzer gives the
// files of a corpus, so a seed that is already there is not written again. Seeds longer than
// MAX_BYTES (65536 by default) are left out: libFuzzer mutates inputs up to the longest one.
//
//   nes_frame      the frames of nes.parse, and the frames nes.stringify wrote
//   wire_message   a selector byte, then a payload: wire.decodeSelected inputs as they are, and the
//                  inputs of wire.decode and proto.decode and the outputs of wire.encode and
//                  proto.encode after the byte of their codec (its index in route order, the request
//                  before the response); a proto record counts when its message is the payload of
//                  a route's request or response, such as peer.GetStatusResponse (not
//                  blocks.GetBlocksResponse: that codec writes a list of block headers)
//   status_verify  getStatus responses: status.verify inputs, status.sign outputs, and the
//                  getStatus response payloads of the records above

const crypto = require("crypto");
const fs = require("fs");
const path = require("path");

const ROUTES = ["getPeers", "getCommonBlocks", "getStatus", "getBlocks", "postBlock", "getUnconfirmedTransactions", "postTransactions"];
const TARGETS = ["nes_frame", "wire_message", "status_verify"];

function bytes(hex) {
    return typeof hex === "string" && /^([0-9a-fA-F]{2})*$/.test(hex) ? Buffer.from(hex, "hex") : null;
}

/** The selector byte of a codec, or null for a route or direction that is not one of the 14. */
function selector(route, direction) {
    const index = ROUTES.indexOf(route);
    if (index < 0 || !["request", "response"].includes(direction)) return null;
    return index * 2 + (direction === "response" ? 1 : 0);
}

/** The route and direction whose payload is the protobuf message `name`, such as
 * peer.GetStatusResponse; null for a nested message such as peer.GetStatusResponse.State, and for
 * blocks.GetBlocksResponse, whose codec writes a list of headers instead. */
function codecOfMessage(name) {
    const match = /^(?:peer|blocks|transactions)\.([A-Z]\w*?)(Request|Response)$/.exec(name ?? "");
    if (!match || name === "blocks.GetBlocksResponse") return null;
    return { route: match[1][0].toLowerCase() + match[1].slice(1), direction: match[2].toLowerCase() };
}

/** The seeds of one record, as [target, bytes] pairs. */
function seeds(record) {
    const input = record.input ?? {};
    const output = record.output ?? {};
    const out = [];
    const add = (target, buffer) => buffer && out.push([target, buffer]);
    const codec = (payload, { route, direction } = input) => {
        const byte = selector(route, direction);
        if (!payload || byte === null) return;
        add("wire_message", Buffer.concat([Buffer.from([byte]), payload]));
        if (route === "getStatus" && direction === "response") add("status_verify", payload);
    };
    const message = codecOfMessage(input.message) ?? {};
    switch (record.op) {
        case "nes.parse":
            add("nes_frame", bytes(input.hex));
            break;
        case "nes.stringify":
            add("nes_frame", bytes(output.hex));
            break;
        case "wire.decodeSelected":
            add("wire_message", bytes(input.hex));
            break;
        case "wire.decode":
            codec(bytes(input.hex));
            break;
        case "wire.encode":
            codec(bytes(output.hex));
            break;
        case "proto.decode":
            codec(bytes(input.hex), message);
            break;
        case "proto.encode":
            codec(bytes(output.hex), message);
            break;
        case "status.verify":
            add("status_verify", bytes(input.hex));
            break;
        case "status.sign":
            add("status_verify", bytes(output.hex));
            break;
        default:
            break;
    }
    return out;
}

function main(argv) {
    const [file, corpus, max = "65536"] = argv;
    const maxBytes = Number(max);
    if (!file || !corpus || !Number.isSafeInteger(maxBytes) || maxBytes < 1) {
        throw new Error("usage: wire-fuzz-seeds.js VECTOR_FILE CORPUS_DIR [MAX_BYTES]");
    }
    const counts = Object.fromEntries(TARGETS.map((target) => [target, { seeds: 0, written: 0, tooLong: 0 }]));
    for (const target of TARGETS) fs.mkdirSync(path.join(corpus, target), { recursive: true });
    const lines = fs.readFileSync(file, "utf8").split("\n").filter((line) => line.trim() !== "");
    for (const [index, line] of lines.entries()) {
        let record;
        try {
            record = JSON.parse(line);
        } catch (error) {
            throw new Error(`${file}:${index + 1}: ${error.message}`);
        }
        for (const [target, buffer] of seeds(record)) {
            const count = counts[target];
            if (buffer.length > maxBytes) {
                count.tooLong += 1;
                continue;
            }
            count.seeds += 1;
            const name = path.join(corpus, target, crypto.createHash("sha1").update(buffer).digest("hex"));
            if (!fs.existsSync(name)) {
                fs.writeFileSync(name, buffer);
                count.written += 1;
            }
        }
    }
    for (const [target, count] of Object.entries(counts)) {
        console.log(
            `wire-fuzz-seeds: ${target}: ${count.seeds} seeds, ${count.written} new, ${count.tooLong} longer than ${maxBytes} bytes left out`,
        );
    }
}

try {
    main(process.argv.slice(2));
} catch (error) {
    console.error(`wire-fuzz-seeds: ${error.message}`);
    process.exit(1);
}
