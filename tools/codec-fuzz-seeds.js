#!/usr/bin/env node
"use strict";
// codec-fuzz-seeds.js: seed the corpora of the codec and head fuzz targets from a vector directory.
//
//   node tools/codec-fuzz-seeds.js VECTOR_DIR[:VECTOR_DIR...] CORPUS_DIR [MAX_BYTES]
//
// A list of vector directories, separated by colons, is read as one.
//
// address_decode takes text: every address that the records of the vector classes V01 to V03 hold
// (an input's `address`, a transaction's `senderId` and `recipientId`, the transfers' recipients),
// each as it is and made wrong in the ways an address goes wrong (a character changed, dropped or
// added, a lower-case copy, a leading 1, white space around it).
//
// transaction_json takes JSON text: the JSON of every transaction of the transaction classes V02
// and V03 (the `json` of a record's input or output), as it is; with the old names the reference's
// factory renames (`asset.payments` for a transfer's `asset.transfers`, `vendorField` for `memo`);
// without its `id`; with a null `id`; with a number for `fee`; and made short.
//
// api_head takes a byte that says how many bytes the head is fed at a time, then the connection's
// bytes: the query of every record of the API classes (V28 and V34) as the target of a GET head,
// each fed one byte at a time and at once, and a set of heads of the shapes the head reader's tests
// measure against Node 18.
//
// Each seed is written as CORPUS_DIR/<target>/<SHA-1 of its bytes>, the name libFuzzer gives the
// files of a corpus, so a seed that is already there is not written again. Seeds longer than
// MAX_BYTES (65536 by default) are left out.

const crypto = require("crypto");
const fs = require("fs");
const path = require("path");

const ADDRESS_KEYS = new Set(["address", "recipientId", "senderId"]);

function usage() {
    console.error("usage: node tools/codec-fuzz-seeds.js VECTOR_DIR[:VECTOR_DIR...] CORPUS_DIR [MAX_BYTES]");
    return 2;
}

/** The records of every file of the classes whose file name starts with one of `prefixes`. */
function records(vectors, prefixes) {
    const out = [];
    for (const dir of vectors.split(":")) {
        for (const file of fs.readdirSync(dir).filter((f) => f.endsWith(".jsonl")).sort()) {
            if (!prefixes.some((p) => file.startsWith(p))) continue;
            for (const line of fs.readFileSync(path.join(dir, file), "utf8").split("\n")) {
                if (line === "") continue;
                const record = JSON.parse(line);
                if (record.op !== "meta") out.push(record);
            }
        }
    }
    return out;
}

/** Every string under an address key, anywhere in `value`. */
function addressesIn(value, found) {
    if (Array.isArray(value)) {
        for (const item of value) addressesIn(item, found);
    } else if (value !== null && typeof value === "object") {
        for (const [key, item] of Object.entries(value)) {
            if (ADDRESS_KEYS.has(key) && typeof item === "string") found.add(item);
            else addressesIn(item, found);
        }
    }
}

/** The ways an address text goes wrong, each a new text. */
function broken(address) {
    const out = [address.toLowerCase(), `1${address}`, ` ${address}\n`, address.slice(1), address.slice(0, -1), `${address}1`];
    const at = Math.floor(address.length / 2);
    const other = address[at] === "z" ? "y" : "z";
    out.push(address.slice(0, at) + other + address.slice(at + 1));
    out.push(address.slice(0, at) + address.slice(at + 1));
    return out;
}

/** The JSON of every transaction of `records`, in the shapes a record holds it. */
function transactionsIn(list) {
    const out = [];
    for (const record of list) {
        for (const side of [record.input, record.output]) {
            if (side && side.json && typeof side.json === "object" && !Array.isArray(side.json) && side.json.type !== undefined) out.push(side.json);
        }
    }
    return out;
}

/** The variants of a transaction's JSON a client may send. */
function variants(json) {
    const out = [json];
    const copy = () => JSON.parse(JSON.stringify(json));
    const bare = copy();
    delete bare.id;
    out.push(bare);
    const nulled = copy();
    nulled.id = null;
    out.push(nulled);
    if (json.asset && json.asset.transfers) {
        const old = copy();
        old.typeGroup = 1;
        old.asset.payments = old.asset.transfers;
        delete old.asset.transfers;
        out.push(old);
    }
    if (json.memo) {
        const old = copy();
        old.vendorField = old.memo;
        delete old.memo;
        out.push(old);
    }
    if (typeof json.fee === "string" && /^[0-9]{1,15}$/.test(json.fee)) {
        const number = copy();
        number.fee = Number(json.fee);
        out.push(number);
    }
    const short = copy();
    delete short.signature;
    out.push(short);
    return out;
}

/** Heads of the shapes the head reader's tests measure against Node 18. */
const HEADS = [
    "GET /api/blocks HTTP/1.1\r\nHost: localhost\r\n\r\n",
    "GET /api/blocks HTTP/1.1\r\nHost: localhost\r\n",
    "\r\n\r\nGET /api HTTP/1.1\r\n\r\n",
    "GET /a b HTTP/1.1\r\n\r\n",
    "GET /api/wallets?limit=2&orderBy=balance:desc HTTP/1.0\r\nAccept: */*\r\nUser-Agent: x\r\n\r\n",
    "GET /api/node/status HTTP/1.1\nHost: a\n\n",
    "GET / HTTP/1.1\r\nx:   v   \r\nx: w\r\n\r\n",
    "GET / HTTP/1.1\r\nx\ty: z\r\n\r\n",
    "POST /api/transactions HTTP/1.1\r\nContent-Type: application/json\r\nContent-Length: 5\r\n\r\nhello",
    "POST /api/transactions HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n",
    "POST / HTTP/1.1\r\nTransfer-Encoding: \r\nContent-Length: 2\r\n\r\nab",
    "GET / HTTP/1.1\r\nExpect: 100-continue\r\n\r\n",
    "GET / HTTP/1.0\r\nExpect: 100-continue\r\n\r\n",
    "CONNECT a:1 HTTP/1.1\r\n\r\n",
    "OPTIONS * HTTP/1.1\r\n\r\n",
    "PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n",
    "FOO / HTTP/1.1\r\n\r\n",
    "get / HTTP/1.1\r\n\r\n",
    "GET http://a/b?c=d#e HTTP/1.1\r\nHost: a\r\n\r\n",
    "GET /é HTTP/1.1\r\n\r\n",
    "GET /api/%zz HTTP/1.1\r\n\r\n",
    "GET / HTTP/1.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n",
    "GET / HTTP/1.1\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\n",
    "GET / HTTP/1.1\rx: y\r\r",
    "GET /api HTTP/1.1\r\n" + "x: y\r\n".repeat(200) + "\r\n",
    "GET /" + "q".repeat(16380) + " HTTP/1.1\r\n\r\n",
];

function main(argv) {
    if (argv.length < 2) return usage();
    const [vectors, corpus] = argv;
    const max = Number(argv[2] || 65536);
    const counts = {};
    const writer = (target) => {
        const dir = path.join(corpus, target);
        fs.mkdirSync(dir, { recursive: true });
        counts[target] = 0;
        return (bytes) => {
            if (bytes.length > max) return;
            const file = path.join(dir, crypto.createHash("sha1").update(bytes).digest("hex"));
            if (fs.existsSync(file)) return;
            fs.writeFileSync(file, bytes);
            counts[target]++;
        };
    };

    const transactions = records(vectors, ["V02", "V03"]);
    const addresses = new Set();
    for (const record of [...records(vectors, ["V01"]), ...transactions]) addressesIn(record, addresses);
    const address = writer("address_decode");
    for (const text of addresses) {
        address(Buffer.from(text, "utf8"));
        for (const wrong of broken(text)) address(Buffer.from(wrong, "utf8"));
    }
    address(Buffer.from("", "utf8"));

    const json = writer("transaction_json");
    const seen = new Set();
    for (const tx of transactionsIn(transactions)) {
        for (const variant of variants(tx)) {
            const text = JSON.stringify(variant);
            if (seen.has(text)) continue;
            seen.add(text);
            json(Buffer.from(text, "utf8"));
        }
    }

    const head = writer("api_head");
    const queries = new Set();
    for (const record of records(vectors, ["V28", "V34"])) {
        const query = record.input && record.input.query;
        if (typeof query === "string") queries.add(query);
    }
    const steps = [0, 7, 255];
    const seedHead = (text) => {
        for (const step of steps) head(Buffer.concat([Buffer.from([step]), Buffer.from(text, "latin1")]));
    };
    for (const text of HEADS) seedHead(Buffer.from(text, "utf8").toString("latin1"));
    for (const query of queries) seedHead(`GET /api/blocks?${Buffer.from(query, "utf8").toString("latin1")} HTTP/1.1\r\nHost: localhost\r\n\r\n`);

    console.log(
        `codec-fuzz-seeds: ${addresses.size} addresses, ${seen.size} transactions and ${queries.size} queries from ${vectors}; new seeds in ${corpus}: ` +
            Object.entries(counts)
                .map(([target, n]) => `${target} ${n}`)
                .join(", "),
    );
    return 0;
}

process.exitCode = main(process.argv.slice(2));
