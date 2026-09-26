#!/usr/bin/env node
"use strict";
// gen-vectors.js: write the golden vectors of level L0 of the differential tests (fixed cases,
// compared byte for byte) from the Solar TypeScript reference.
//
//   node gen-vectors.js --out DIR [--solar-dir DIR] [--no-backend-check]
//
// Output: one JSONL file per vector class (V01 to V05, below) and MANIFEST.sha256 (sha256sum
// format). Each file starts with a meta record; every other line is
//   {"op", "network", "height", "input", "output"}   or   {..., "error": {"class", "message"}}
// Records hold only values the reference computed, so they are bit-identical across bcrypto
// backends and across image builds; everything that legitimately differs between runs (backend,
// image id, Node version) is in the meta record.
//
// Backend check: after writing, the script runs itself again under NODE_BACKEND=js and fails if
// any record differs from the native run.
//
// Every record is made on the devnet network of lib/network.js (network byte 90), at an explicit
// rule height. Transactions cover the six types Heartwood keeps; 1/5 IPFS, which Heartwood drops,
// is in the removed-type class. Records carry no WIF or BLS fields, and there are no v2
// transactions.

const fs = require("fs");
const path = require("path");
const { spawnSync } = require("child_process");

const { assertNode18, loadSolar, sha256Hex } = require("./lib/solar");
const { buildDevnet } = require("./lib/network");
const { FIXED_AUX, makeOps } = require("./lib/ops");

const FORMAT = "heartwood-vectors/1";
const HEIGHT = 2; // rule height for records that are not blocks (heights 1 and 2 differ only in donations)
const MANIFEST = "MANIFEST.sha256";

function parseArgs(argv) {
    const opts = { out: undefined, solarDir: undefined, backendCheck: true, emit: false, help: false };
    for (let i = 0; i < argv.length; i++) {
        const a = argv[i];
        const value = () => {
            if (i + 1 >= argv.length) throw new Error(`${a} needs a value`);
            return argv[++i];
        };
        if (a === "--out") opts.out = value();
        else if (a === "--solar-dir") opts.solarDir = value();
        else if (a === "--no-backend-check") opts.backendCheck = false;
        else if (a === "--emit") opts.emit = true;
        else if (a === "--help" || a === "-h") opts.help = true;
        else throw new Error(`unknown argument ${a}`);
    }
    return opts;
}

// ---- cases -----------------------------------------------------------------------------------------

const PASSPHRASES = [
    "this is a top secret passphrase",
    "differential test sender",
    "delegate one",
    "",
    "ünïcödé pässphrase ☃ 日本語",
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
];
const SENDER = "differential test sender";
const SECOND = "differential test second passphrase";
const DOUBLE_SENDER = "second signature sender";
const GENERATOR = "delegate one";
const IPFS_HASH = "QmR45FmbVVrixReBwJkhEKde2qwHYaQzGxu4ZoDeswuF9w";

function bytesHex(length, seed) {
    const b = Buffer.alloc(length);
    for (let i = 0; i < length; i++) b[i] = (i * 31 + seed) & 0xff;
    return b.toString("hex");
}

function flipLastByte(hex) {
    const b = Buffer.from(hex, "hex");
    b[b.length - 1] ^= 0x01;
    return b.toString("hex");
}

/** Build every class's records. `rec` runs one operation and returns the finished record. */
function generate(solar, devnet) {
    const { run } = makeOps(solar, devnet);
    const { Identities } = solar.lib;
    const rec = (op, height, input) => ({ op, network: devnet.info.name, height, input, ...run(op, height, input) });
    const must = (r) => {
        if (r.error) throw new Error(`${r.op} failed while generating: ${r.error.class}: ${r.error.message}`);
        return r;
    };

    // V01 keys and addresses.
    const v01 = [];
    const keys = PASSPHRASES.map((passphrase) => must(rec("keys.fromPassphrase", HEIGHT, { passphrase })));
    v01.push(...keys);
    for (const k of keys) {
        for (const networkByte of [90, 63]) {
            v01.push(must(rec("address.fromPublicKey", HEIGHT, { publicKey: k.output.publicKey, networkByte })));
        }
    }
    const addr90 = v01.find((r) => r.op === "address.fromPublicKey" && r.input.networkByte === 90).output.address;
    const addr63 = v01.find((r) => r.op === "address.fromPublicKey" && r.input.networkByte === 63).output.address;
    const badChecksum = addr90.slice(0, -1) + (addr90.endsWith("1") ? "2" : "1");
    for (const [address, networkByte] of [
        [addr90, 90],
        [addr90, 63],
        [addr63, 63],
        [addr63, 90],
        [badChecksum, 90],
        [addr90.slice(0, -1), 90],
        [addr90 + "1", 90],
    ]) {
        v01.push(must(rec("address.validate", HEIGHT, { address, networkByte })));
    }

    // V04 signatures: BIP340 with fixed aux, and legacy Schnorr (deterministic, no aux).
    const v04 = [];
    const signers = [SENDER, GENERATOR].map((p) => keys.find((k) => k.input.passphrase === p).output);
    const other = keys.find((k) => k.input.passphrase === PASSPHRASES[0]).output;
    const messages = [
        sha256Hex(Buffer.from("heartwood difftest message", "utf8")), // 32 bytes: signed as given
        "",
        "00",
        bytesHex(33, 1),
        bytesHex(100, 2),
    ];
    const auxes = [FIXED_AUX.toString("hex"), "00".repeat(32)];
    for (const key of signers) {
        for (const message of messages) {
            for (const aux of auxes) {
                const s = must(rec("sig.bip340.sign", HEIGHT, { message, privateKey: key.privateKey, aux }));
                v04.push(s);
                if (aux !== auxes[0]) continue;
                const signature = s.output.signature;
                const pk = key.publicKey;
                const flippedPrefix = (pk.startsWith("02") ? "03" : "02") + pk.slice(2);
                for (const input of [
                    { message, signature, publicKey: pk },
                    { message, signature: flipLastByte(signature), publicKey: pk },
                    { message, signature, publicKey: other.publicKey },
                    { message, signature, publicKey: pk.slice(2) }, // 32-byte x-only key
                    { message, signature, publicKey: flippedPrefix }, // the key slice ignores the prefix
                ]) {
                    v04.push(must(rec("sig.bip340.verify", HEIGHT, input)));
                }
            }
        }
    }
    const legacyMessages = [messages[0], "", "00", bytesHex(51, 3), bytesHex(1024, 4), bytesHex(4096, 5)];
    for (const key of signers) {
        for (const message of legacyMessages) {
            const s = must(rec("sig.legacy.sign", HEIGHT, { message, privateKey: key.privateKey }));
            v04.push(s);
            for (const input of [
                { message, signature: s.output.signature, publicKey: key.publicKey },
                { message, signature: flipLastByte(s.output.signature), publicKey: key.publicKey },
                { message, signature: s.output.signature, publicKey: other.publicKey },
            ]) {
                v04.push(must(rec("sig.legacy.verify", HEIGHT, input)));
            }
        }
    }

    // V02 transactions: the six types Heartwood keeps, v3, standard header, fixed aux.
    const recipients = [];
    for (let i = 0; i < 256; i++) recipients.push(Identities.Address.fromPassphrase(`recipient ${i}`, 90));
    const memo255 = "m".repeat(253) + "é"; // 255 bytes, a 2-byte character at the boundary
    const tx = (name, builder, calls, extra = {}) => ({ name, input: { builder, calls, sign: SENDER, ...extra } });
    const txCases = [
        tx("transfer-1-memo", "transfer", [["recipientId", recipients[0]], ["amount", "123456789"], ["nonce", "1"], ["memo", "hello"]]),
        tx("transfer-1", "transfer", [["recipientId", recipients[1]], ["amount", "100000000"], ["nonce", "2"]]),
        tx("transfer-2", "transfer", [["addTransfer", recipients[2], "1"], ["addTransfer", recipients[3], "2"], ["nonce", "3"]]),
        tx("transfer-256", "transfer", [
            ...recipients.map((r, i) => ["addTransfer", r, String(1000 + i)]),
            ["nonce", "4"],
            ["fee", "100000000"],
            ["memo", "multipayment memo ☃ utf8"],
        ]),
        tx("transfer-memo-255", "transfer", [["recipientId", recipients[4]], ["amount", "1"], ["nonce", "5"], ["memo", memo255]]),
        tx("second-signature", "secondSignature", [["signatureAsset", SECOND], ["nonce", "6"]]),
        tx("delegate-registration", "delegateRegistration", [["usernameAsset", "difftest_del1"], ["nonce", "7"]]),
        tx("resignation-0", "delegateResignation", [["resignationTypeAsset", 0], ["nonce", "8"]]),
        tx("resignation-1", "delegateResignation", [["resignationTypeAsset", 1], ["nonce", "9"]]),
        tx("resignation-2", "delegateResignation", [["resignationTypeAsset", 2], ["nonce", "10"]]),
        tx("burn", "burn", [["amount", "2000000000"], ["nonce", "11"]]),
        tx("vote-2", "vote", [["votesAsset", { difftest_del1: 60.5, gym: 39.5 }], ["nonce", "12"]]),
        tx("vote-1", "vote", [["votesAsset", { difftest_del1: 100 }], ["nonce", "13"]]),
        tx("unvote", "vote", [["votesAsset", {}], ["nonce", "14"]]),
        tx("vote-array-3", "vote", [["votesAsset", ["a", "b", "c"]], ["nonce", "15"]]),
        {
            name: "transfer-second-signed",
            input: { builder: "transfer", calls: [["recipientId", recipients[5]], ["amount", "5"], ["nonce", "1"]], sign: DOUBLE_SENDER, secondSign: SECOND },
        },
    ];
    const v02 = txCases.map((c) => ({ ...must(rec("tx.build", HEIGHT, c.input)), name: c.name }));
    const txHex = (name) => v02.find((r) => r.name === name).output.hex;

    // V03 removed types: 1/5 IPFS is not carried over to Heartwood. The reference still builds it;
    // Heartwood must reject it at decode. The other removed types are to be added here.
    const v03 = [{ ...must(rec("tx.build", HEIGHT, tx("ipfs", "ipfs", [["ipfsAsset", IPFS_HASH], ["nonce", "16"]]).input)), name: "ipfs" }];

    // V05 blocks: BlockFactory.make with fixed aux, past timestamps only.
    const aux = FIXED_AUX.toString("hex");
    const block = (name, height, timestamp, transactions) => ({
        ...must(rec("block.make", height, { height, timestamp, previousBlock: "ab".repeat(32), reward: "180000000", passphrase: GENERATOR, aux, transactions })),
        name,
    });
    const v05 = [
        block("empty", 2, 8, []),
        block("one-transfer", 3, 16, [txHex("transfer-1-memo")]),
        block("mixed", 4, 24, ["transfer-1", "transfer-2", "second-signature", "delegate-registration", "resignation-1", "burn", "vote-2"].map(txHex)),
    ];
    v05.push({ ...must(rec("block.fromBytes", 4, { hex: v05[2].output.hex })), name: "mixed-decoded" });

    return {
        "V01-keys-addresses": v01,
        "V02-transactions": v02,
        "V03-removed-types": v03,
        "V04-signatures": v04,
        "V05-blocks": v05,
    };
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

function metaLine(solar, devnet, className, count) {
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
    });
}

function writeVectors(out, solar, devnet, lines) {
    fs.mkdirSync(out, { recursive: true });
    for (const f of fs.readdirSync(out)) {
        if (f.endsWith(".jsonl") || f === MANIFEST) fs.unlinkSync(path.join(out, f));
    }
    const manifest = [];
    for (const [className, recordLines] of Object.entries(lines)) {
        const file = `${className}.jsonl`;
        const body = [metaLine(solar, devnet, className, recordLines.length), ...recordLines].join("\n") + "\n";
        fs.writeFileSync(path.join(out, file), body);
        manifest.push(`${sha256Hex(Buffer.from(body, "utf8"))}  ${file}`);
    }
    manifest.sort((a, b) => (a.slice(66) < b.slice(66) ? -1 : 1));
    fs.writeFileSync(path.join(out, MANIFEST), manifest.join("\n") + "\n");
}

/** Run this script again under NODE_BACKEND=js and compare its records with ours. */
function backendCheck(solar, lines) {
    const child = spawnSync(process.execPath, [__filename, "--emit", "--solar-dir", solar.solarDir], {
        env: { ...process.env, NODE_BACKEND: "js" },
        encoding: "utf8",
        maxBuffer: 512 * 1024 * 1024,
    });
    if (child.status !== 0) throw new Error(`backend check: the NODE_BACKEND=js run failed:\n${child.stderr}`);
    const other = JSON.parse(child.stdout);
    if (other.backend !== "js") throw new Error(`backend check: the child run used bcrypto backend ${other.backend}, not js`);
    let differences = 0;
    for (const [className, recordLines] of Object.entries(lines)) {
        const theirs = other.lines[className] || [];
        const n = Math.max(recordLines.length, theirs.length);
        for (let i = 0; i < n; i++) {
            if (recordLines[i] !== theirs[i]) {
                differences++;
                if (differences <= 5) console.error(`backend check: ${className} record ${i + 1} differs between native and js`);
            }
        }
    }
    if (differences) throw new Error(`backend check: ${differences} records differ between the native and js backends`);
    return Object.values(lines).reduce((n, l) => n + l.length, 0);
}

function main() {
    const opts = parseArgs(process.argv.slice(2));
    if (opts.help) {
        console.log("Usage: node gen-vectors.js --out DIR [--solar-dir DIR] [--no-backend-check]");
        return;
    }
    assertNode18();
    const solar = loadSolar(opts.solarDir);
    const devnet = buildDevnet(solar);
    const classes = generate(solar, devnet);
    const lines = Object.fromEntries(Object.entries(classes).map(([c, records]) => [c, records.map(recordLine)]));

    if (opts.emit) {
        process.stdout.write(JSON.stringify({ backend: solar.bcryptoBackend, lines }));
        return;
    }
    if (!opts.out) throw new Error("--out DIR is required");
    if (opts.backendCheck && solar.bcryptoBackend !== "native") {
        throw new Error("the backend check needs the native bcrypto backend (unset NODE_BACKEND, or pass --no-backend-check)");
    }
    writeVectors(opts.out, solar, devnet, lines);
    const total = Object.values(lines).reduce((n, l) => n + l.length, 0);
    const where = `${solar.provenance.commit || "unknown commit"}${solar.provenance.dirty ? " (dirty)" : ""}`;
    console.log(`gen-vectors: ${total} records in ${Object.keys(lines).length} files at ${opts.out}`);
    console.log(`gen-vectors: Solar ${solar.version} ${where}, Node ${process.version}, bcrypto ${solar.bcryptoBackend}`);
    if (opts.backendCheck) console.log(`gen-vectors: backend check passed, ${backendCheck(solar, lines)} records identical under NODE_BACKEND=js`);
}

try {
    main();
} catch (error) {
    console.error(`gen-vectors: ${error.message}`);
    process.exit(1);
}
