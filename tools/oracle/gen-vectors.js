#!/usr/bin/env node
"use strict";
// gen-vectors.js: write the golden vectors of level L0 of the differential tests (fixed cases,
// compared byte for byte) from the reference implementation.
//
//   node gen-vectors.js --out DIR [--solar-dir DIR] [--devnet-generator FILE] [--external DIR]
//                       [--no-backend-check]
//
// Output: one JSONL file per vector class (lib/vectors/, listed in CLASSES below) and
// MANIFEST.sha256 (sha256sum format). Each file starts with a meta record; every other line is
//   {"op", "network", "height", "name"?, "input", "output"}   or   {..., "error": {"class", "message"}}
// Records hold only values the reference computed, so they are bit-identical across bcrypto
// backends and across image builds; everything that legitimately differs between runs (backend,
// image id, Node version) is in the meta record.
//
// --devnet-generator  the devnet generator, gen-network.js, which class V10 runs: a separate tool
//                     of the maintainers, not published; without it V10 is not written, and
//                     the committed V10 stays as it is [$DEVNET_GENERATOR]
// --external DIR      the published vector files of class V11 and their SOURCES.json
//                     [external/ next to this script, else packages/crypto/tests/vectors/external]
//
// Backend check: after writing, the script runs itself again under NODE_BACKEND=js and fails if
// any record differs from the native run.
//
// Every record is made on the devnet network of lib/network.js (network byte 90), at an explicit
// rule height, or under milestones the record itself gives as JSON text. Records carry no WIF or
// BLS fields.

const fs = require("fs");
const path = require("path");
const { spawnSync } = require("child_process");

const { assertNode18, loadSolar, sha256Hex } = require("./lib/solar");
const { buildDevnet } = require("./lib/network");
const { FIXED_AUX, makeOps } = require("./lib/ops");
const { makeCrafter } = require("./lib/craft");
const genesis = require("./lib/vectors/genesis");
const kernel = require("./lib/vectors/kernel");
const state = require("./lib/vectors/state");

const FORMAT = "heartwood-vectors/1";
const HEIGHT = 2; // rule height for records that are not blocks (heights 1 and 2 differ only in donations)
const MANIFEST = "MANIFEST.sha256";

// The classes, in generation order (later classes use values of earlier ones). A class whose
// builder returns null is not written.
const CLASSES = [
    ["V01-keys-addresses", require("./lib/vectors/keys")],
    ["V02-transactions", require("./lib/vectors/transactions")],
    ["V03-noncanonical-removed", require("./lib/vectors/noncanonical")],
    ["V04-signatures", require("./lib/vectors/signatures")],
    ["V05-blocks", require("./lib/vectors/blocks")],
    ["V06-arithmetic", require("./lib/vectors/arithmetic")],
    ["V07-vote-rules", require("./lib/vectors/votes")],
    ["V08-ranking", state.ranking],
    ["V09-rounds", kernel.rounds],
    ["V09-shuffle", state.shuffle],
    ["V10-genesis", genesis],
    ["V11-external", require("./lib/vectors/external")],
    ["V12-milestone-config", require("./lib/vectors/config")],
    ["V15-vote-amounts", state.voteAmounts],
    ["V16-kernel", kernel.kernel],
    ["V17-handlers", require("./lib/vectors/handlers-state")],
    ["V18-genesis-state", require("./lib/vectors/genesis-state")],
    ["V19-fee-floor", require("./lib/vectors/fees")],
    ["V20-logger", kernel.logger],
    ["V21-pool", require("./lib/vectors/pool")],
    ["V22-text", require("./lib/vectors/text")],
    ["V23-config", require("./lib/vectors/config-schema")],
];
// The peer-to-peer wire (heartwood-p2p-wire), generated last; its builder returns null while it
// has no records, and the class is then not written.
CLASSES.push(["V13-p2p-wire", require("./lib/vectors/wire")]);
// The peer-to-peer client (heartwood-p2p and its wire additions); a builder returns null while it
// has no records, and the class is then not written.
const p2pRules = require("./lib/vectors/p2p-rules");
CLASSES.push(
    ["V24-p2p-exchange", p2pRules.exchange],
    ["V25-peer-rules", p2pRules.rules],
    ["V26-network", require("./lib/vectors/p2p-network")],
);

function parseArgs(argv) {
    const opts = {
        out: undefined,
        solarDir: undefined,
        devnetGenerator: process.env.DEVNET_GENERATOR || undefined,
        externalDir: undefined,
        checkGenesis: undefined,
        backendCheck: true,
        emit: false,
        help: false,
    };
    for (let i = 0; i < argv.length; i++) {
        const a = argv[i];
        const value = () => {
            if (i + 1 >= argv.length) throw new Error(`${a} needs a value`);
            return argv[++i];
        };
        if (a === "--out") opts.out = value();
        else if (a === "--solar-dir") opts.solarDir = value();
        else if (a === "--devnet-generator") opts.devnetGenerator = value();
        else if (a === "--external") opts.externalDir = value();
        else if (a === "--check-genesis") opts.checkGenesis = value();
        else if (a === "--no-backend-check") opts.backendCheck = false;
        else if (a === "--emit") opts.emit = true;
        else if (a === "--help" || a === "-h") opts.help = true;
        else throw new Error(`unknown argument ${a}`);
    }
    return opts;
}

function defaultExternalDir() {
    const beside = path.join(__dirname, "external");
    if (fs.existsSync(beside)) return beside;
    return path.resolve(__dirname, "..", "..", "packages", "crypto", "tests", "vectors", "external");
}

/** Build every class's records. */
async function generate(solar, devnet, options) {
    const { run } = makeOps(solar, devnet);
    const shared = { height: HEIGHT };
    const rec = async (op, height, input) => ({ op, network: devnet.info.name, height, input, ...(await run(op, height, input)) });
    const must = (r) => {
        if (r.error) throw new Error(`${r.op} failed while generating: ${r.error.class}: ${r.error.message}`);
        return r;
    };
    const ctx = { solar, devnet, rec, must, shared, options, craft: makeCrafter(solar) };
    const classes = {};
    for (const [name, build] of CLASSES) {
        const records = await build(ctx);
        if (records !== null) classes[name] = records;
    }
    const classMeta = {
        "V04-signatures": { legacySchnorrRecords: shared.legacyCount },
        "V10-genesis": shared.devnetGenerator ? { devnetGenerator: shared.devnetGenerator } : undefined,
        "V11-external": { external: shared.external },
    };
    return { classes, classMeta };
}

/** One JSON line per record, with a fixed key order. */
function recordLine(r) {
    const ordered = { op: r.op, network: r.network, height: r.height };
    if (r.name !== undefined) ordered.name = r.name;
    ordered.input = r.input;
    if (r.error) ordered.error = r.error;
    else ordered.output = r.output;
    return JSON.stringify(ordered);
}

function metaLine(solar, devnet, className, count, extra) {
    return JSON.stringify({
        op: "meta",
        format: FORMAT,
        class: className,
        records: count,
        generator: "tools/oracle/gen-vectors.js",
        solar: { version: solar.version, ...solar.provenance, seriesSha256: solar.seriesSha256 },
        oracle: { mode: process.env.ORACLE_IMAGE_ID ? "container" : "local", image: process.env.ORACLE_IMAGE_ID || null },
        node: process.version,
        icu: process.versions.icu,
        bcrypto: solar.bcryptoBackend,
        network: devnet.info,
        aux: FIXED_AUX.toString("hex"),
        ...(extra ? { inputs: extra } : {}),
    });
}

function writeVectors(out, solar, devnet, lines, classMeta) {
    fs.mkdirSync(out, { recursive: true });
    for (const f of fs.readdirSync(out)) {
        if (f.endsWith(".jsonl") || f === MANIFEST) fs.unlinkSync(path.join(out, f));
    }
    const manifest = [];
    for (const [className, recordLines] of Object.entries(lines)) {
        const file = `${className}.jsonl`;
        const body = [metaLine(solar, devnet, className, recordLines.length, classMeta[className]), ...recordLines].join("\n") + "\n";
        fs.writeFileSync(path.join(out, file), body);
        manifest.push(`${sha256Hex(Buffer.from(body, "utf8"))}  ${file}`);
    }
    manifest.sort((a, b) => (a.slice(66) < b.slice(66) ? -1 : 1));
    fs.writeFileSync(path.join(out, MANIFEST), manifest.join("\n") + "\n");
}

/** Run this script again under NODE_BACKEND=js and compare its records with ours. */
function backendCheck(solar, lines, opts) {
    const args = [__filename, "--emit", "--solar-dir", solar.solarDir, "--external", opts.externalDir];
    if (opts.devnetGenerator) args.push("--devnet-generator", opts.devnetGenerator);
    const child = spawnSync(process.execPath, args, {
        env: { ...process.env, NODE_BACKEND: "js" },
        encoding: "utf8",
        maxBuffer: 1024 * 1024 * 1024,
    });
    if (child.status !== 0) throw new Error(`backend check: the NODE_BACKEND=js run failed:\n${child.stderr}`);
    const other = JSON.parse(child.stdout);
    if (other.backend !== "js") throw new Error(`backend check: the child run used bcrypto backend ${other.backend}, not js`);
    let differences = 0;
    const classes = new Set([...Object.keys(lines), ...Object.keys(other.lines)]);
    for (const className of classes) {
        const ours = lines[className] || [];
        const theirs = other.lines[className] || [];
        const n = Math.max(ours.length, theirs.length);
        for (let i = 0; i < n; i++) {
            if (ours[i] !== theirs[i]) {
                differences++;
                if (differences <= 5) console.error(`backend check: ${className} record ${i + 1} differs between native and js`);
            }
        }
    }
    if (differences) throw new Error(`backend check: ${differences} records differ between the native and js backends`);
    return Object.values(lines).reduce((n, l) => n + l.length, 0);
}

async function main() {
    const opts = parseArgs(process.argv.slice(2));
    if (opts.help) {
        console.log("Usage: node gen-vectors.js --out DIR [--solar-dir DIR] [--devnet-generator FILE] [--external DIR] [--no-backend-check]");
        return;
    }
    assertNode18();
    const solar = loadSolar(opts.solarDir);
    if (opts.checkGenesis) {
        process.stdout.write(JSON.stringify(genesis.checkGenesis(solar, opts.checkGenesis)));
        return;
    }
    opts.externalDir = path.resolve(opts.externalDir || defaultExternalDir());
    if (opts.devnetGenerator) opts.devnetGenerator = path.resolve(opts.devnetGenerator);
    const devnet = buildDevnet(solar);
    const { classes, classMeta } = await generate(solar, devnet, { ...opts, self: __filename });
    const lines = Object.fromEntries(Object.entries(classes).map(([c, records]) => [c, records.map(recordLine)]));

    if (opts.emit) {
        process.stdout.write(JSON.stringify({ backend: solar.bcryptoBackend, lines }));
        return;
    }
    if (!opts.out) throw new Error("--out DIR is required");
    if (opts.backendCheck && solar.bcryptoBackend !== "native") {
        throw new Error("the backend check needs the native bcrypto backend (unset NODE_BACKEND, or pass --no-backend-check)");
    }
    writeVectors(opts.out, solar, devnet, lines, classMeta);
    const total = Object.values(lines).reduce((n, l) => n + l.length, 0);
    const where = `${solar.provenance.commit || "unknown commit"}${solar.provenance.dirty ? " (dirty)" : ""}`;
    console.log(`gen-vectors: ${total} records in ${Object.keys(lines).length} files at ${opts.out}`);
    for (const [c, l] of Object.entries(lines)) console.log(`gen-vectors:   ${c}: ${l.length}`);
    if (!opts.devnetGenerator) console.log("gen-vectors: V10-genesis not written (no --devnet-generator)");
    console.log(`gen-vectors: reference ${solar.version} ${where}, Node ${process.version}, bcrypto ${solar.bcryptoBackend}`);
    if (opts.backendCheck) console.log(`gen-vectors: backend check passed, ${backendCheck(solar, lines, opts)} records identical under NODE_BACKEND=js`);
}

main().catch((error) => {
    console.error(`gen-vectors: ${error.stack || error.message}`);
    process.exit(1);
});
