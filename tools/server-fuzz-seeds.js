#!/usr/bin/env node
"use strict";
// server-fuzz-seeds.js: seed the corpus of the server's nes_server_session fuzz target from a
// vector directory.
//
//   node tools/server-fuzz-seeds.js VECTOR_DIR CORPUS_DIR [MAX_BYTES [PROBE_DIR]]
//
// Reads the class V31-server of VECTOR_DIR, when present, and the reference-client probe's records
// under PROBE_DIR (each NODE/cases.jsonl: every request the probe sent, once from the forger's
// address and once from another). Each p2p.serveRequest record becomes
// one script of the target (fuzz/fuzz_targets/nes_server_session.rs):
//
//   byte 0   0 when the record's caller is in its remoteAccess (the target's forger address),
//            else 1 (another address)
//   byte 1   1 when the record's node has booted, else 0
//   step     the client's hello as one masked frame
//   steps    the record's request (id 2, its route as the path, its bytes as the payload) as one
//            binary frame: one frame step when the message fits in one (255 bytes), else the whole
//            masked frame in raw steps of at most 255 bytes
//
// A request whose frame needs more steps than a script runs (64) is left out, as is a seed longer
// than MAX_BYTES (65536 by default). Each seed is written as CORPUS_DIR/nes_server_session/<SHA-1
// of its bytes>, the name libFuzzer gives the files of a corpus, so a seed that is already there
// is not written again.

const crypto = require("crypto");
const fs = require("fs");
const path = require("path");

const CLASS = "V31-server.jsonl";
const TARGET = "nes_server_session";
// The target's step selectors (the script byte modulo 5) and its limits.
const RAW = 0;
const FRAME = 1;
const MAX_STEPS = 64;
const MAX_STEP_BYTES = 255;
// A final binary frame, and the mask of the frames this script writes itself.
const BINARY = 0x82;
const MASK = [1, 2, 3, 4];

/** A nes message: version, type, id, status 200, path, no socket id or heartbeat, payload. */
function message(version, type, id, pathText, payload) {
    const route = Buffer.from(pathText, "utf8");
    if (route.length > 255) return null;
    const header = Buffer.alloc(14);
    header[0] = version;
    header[1] = type;
    header.writeUInt32BE(id, 2);
    header.writeUInt16BE(200, 6);
    header[8] = route.length;
    return Buffer.concat([header, route, payload]);
}

/** The client's hello: version 2, type 0, id 1. */
const HELLO = message(2, 0, 1, "", Buffer.alloc(0));

/** A frame step: the target masks `payload` with MASK and frames it with the first byte BINARY. */
function frameStep(payload) {
    return Buffer.concat([Buffer.from([FRAME, BINARY, payload.length, ...MASK]), payload]);
}

/** A masked binary frame around `payload`, as a client writes it. */
function maskedFrame(payload) {
    let length;
    if (payload.length < 126) {
        length = Buffer.from([0x80 | payload.length]);
    } else if (payload.length < 65536) {
        length = Buffer.alloc(3);
        length[0] = 0x80 | 126;
        length.writeUInt16BE(payload.length, 1);
    } else {
        length = Buffer.alloc(9);
        length[0] = 0x80 | 127;
        length.writeBigUInt64BE(BigInt(payload.length), 1);
    }
    const masked = Buffer.from(payload.map((byte, i) => byte ^ MASK[i % 4]));
    return Buffer.concat([Buffer.from([BINARY]), length, Buffer.from(MASK), masked]);
}

/** The steps that write `payload` as one binary frame. */
function requestSteps(payload) {
    if (payload.length <= MAX_STEP_BYTES) return [frameStep(payload)];
    const frame = maskedFrame(payload);
    const steps = [];
    for (let at = 0; at < frame.length; at += MAX_STEP_BYTES) {
        const piece = frame.subarray(at, at + MAX_STEP_BYTES);
        steps.push(Buffer.concat([Buffer.from([RAW, piece.length]), piece]));
    }
    return steps;
}

/** The script of a p2p.serveRequest record, or null when it cannot be one. */
function script(record) {
    const input = record.input || {};
    if (typeof input.route !== "string" || typeof input.hex !== "string") return null;
    const request = message(0, 3, 2, input.route, Buffer.from(input.hex, "hex"));
    if (request === null) return null;
    const steps = [frameStep(HELLO), ...requestSteps(request)];
    if (steps.length > MAX_STEPS) return null;
    const remote = (input.remoteAccess || []).includes(input.ip);
    return Buffer.concat([Buffer.from([remote ? 0 : 1, input.booted ? 1 : 0]), ...steps]);
}

/** The scripts of the requests a probe's records hold: each from the forger's address and not. */
function probeScripts(probeDir) {
    const scripts = [];
    const files = [path.join(probeDir, "cases.jsonl")];
    for (const entry of fs.existsSync(probeDir) ? fs.readdirSync(probeDir) : []) {
        files.push(path.join(probeDir, entry, "cases.jsonl"));
    }
    for (const file of files.filter((f) => fs.existsSync(f))) {
        for (const line of fs.readFileSync(file, "utf8").split("\n")) {
            if (!line) continue;
            for (const call of JSON.parse(line).calls || []) {
                if (typeof call.endpoint !== "string" || typeof call.requestHex !== "string") continue;
                for (const remote of [true, false]) {
                    const input = { route: call.endpoint, hex: call.requestHex, ip: "a", booted: true };
                    input.remoteAccess = remote ? ["a"] : [];
                    scripts.push(script({ input }));
                }
            }
        }
    }
    return scripts;
}

function main() {
    const [vectorDir, corpusDir, maxArg, probeDir] = process.argv.slice(2);
    if (!vectorDir || !corpusDir) {
        console.error("usage: server-fuzz-seeds.js VECTOR_DIR CORPUS_DIR [MAX_BYTES [PROBE_DIR]]");
        process.exit(2);
    }
    const max = maxArg ? Number(maxArg) : 65536;
    const file = path.join(vectorDir, CLASS);
    const scripts = [];
    if (fs.existsSync(file)) {
        for (const line of fs.readFileSync(file, "utf8").split("\n")) {
            if (!line) continue;
            const record = JSON.parse(line);
            if (record.op === "p2p.serveRequest") scripts.push(script(record));
        }
    } else {
        console.log(`server-fuzz-seeds: no ${CLASS} in ${vectorDir}`);
    }
    if (probeDir) scripts.push(...probeScripts(probeDir));
    const dir = path.join(corpusDir, TARGET);
    fs.mkdirSync(dir, { recursive: true });
    let seeds = 0;
    let written = 0;
    let leftOut = 0;
    for (const data of scripts) {
        if (data === null || data.length > max) {
            leftOut++;
            continue;
        }
        seeds++;
        const target = path.join(dir, crypto.createHash("sha1").update(data).digest("hex"));
        if (!fs.existsSync(target)) {
            fs.writeFileSync(target, data);
            written++;
        }
    }
    console.log(
        `server-fuzz-seeds: ${TARGET}: ${seeds} seeds, ${written} new, ${leftOut} left out ` +
            `(a frame of more than ${MAX_STEPS} steps or a seed longer than ${max} bytes)`,
    );
}

main();
