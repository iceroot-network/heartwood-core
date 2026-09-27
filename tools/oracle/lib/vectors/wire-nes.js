"use strict";
// V13, nes frames: the frames the reference sends on the peer-to-peer port, every type byte,
// short and overrunning lengths, the path normalisation, lossy UTF-8, the 255-byte spill, and
// the writer's defaults and limits.
//
// Record names starting with "sent: " are the frames the reference's P2P client and server write,
// built as the cited code builds the message object; each is followed by the parse of its bytes.
// Socket ids are synthetic. Random inputs come from SHA-256 in counter mode over a fixed label.

const crypto = require("crypto");
const path = require("path");

// The route ids of the peer-to-peer port (packages/p2p/src/socket-server/routes/*.ts).
const ROUTE_IDS = [
    "p2p.peer.getPeers",
    "p2p.peer.getCommonBlocks",
    "p2p.peer.getStatus",
    "p2p.blocks.getBlocks",
    "p2p.blocks.postBlock",
    "p2p.transactions.getUnconfirmedTransactions",
    "p2p.transactions.postTransactions",
    "p2p.internal.emitEvent",
    "p2p.internal.getCurrentRound",
    "p2p.internal.getSlotNumber",
    "p2p.internal.getNetworkState",
    "p2p.internal.syncBlockchain",
];

// The heartbeat of the reference's nes plugin (packages/nes/src/plugin.ts defaults), which the P2P
// server does not change.
const HEARTBEAT = { interval: 15000, timeout: 5000 };

// Paths for the URL normalisation: the examples of the frame parser's checks, the dot-segment and
// percent-encoding tests of the URL standard that apply after a fixed special-scheme prefix, and
// the delimiters, trimming and removed characters.
const URL_PATHS = [
    "a/../p2p.peer.getStatus",
    "p2p.peer.get\tStatus  ",
    "./p2p.peer.getStatus#x",
    "p2p\\peer?q=a b'",
    "%2e%2E/x",
    " ",
    "\u0000",
    "a? ",
    "foo\tbar",
    "a\n/b\r",
    "a/b\u0001 \u0002",
    "\u0000a",
    "foo\t\u0091%91",
    "a?",
    "a?#",
    "a?b?c",
    "a#b?c",
    "a?b#c#d",
    "?",
    "#",
    "?x",
    "q?\u0001x",
    "é?é",
    "sum?q=a\"b<c>d'e`f{g}h",
    "foo/bar/..",
    "foo/bar/../",
    "foo/./bar",
    "foo/.%2e/bar",
    "foo/%2E%2e",
    "foo/..bar",
    "foo/%2e.bar",
    "foo/bar/../ton/../../a",
    "foo/../../..",
    "foo/../../../ton",
    "foo/%2e",
    "foo/%2e%2",
    "foo/%2e./%2e%2e/.%2e/%2e.bar",
    "///../..",
    "foo/bar//../..",
    "foo/bar//..",
    "..",
    ".",
    "./",
    "../",
    "a/.",
    "a/./",
    "%2E",
    "%2e%2E%2f",
    ".%2E/b",
    "a/b/%2E%2E/c",
    "a\\..\\b",
    "%20foo",
    "foo%",
    "foo%2",
    "foo%2zbar",
    "foo%2Â©zbar",
    "foo%41%7a",
    "foo%00%51",
    "(%28:%3A%29)",
    "foo\\\\bar",
    "%7Ffp3>*i",
    "foo/\u{1F600}",
    "a b",
    "`{}^|[]",
    "a\u007f",
    "/a",
    "﻿p2p.peer.getPeers",
];

// Byte strings that are not UTF-8, or not all of it: Node's decoder replaces each maximal invalid
// subpart with U+FFFD.
const INVALID_UTF8 = [
    "ff",
    "fffe",
    "c3",
    "c328",
    "e282",
    "e28228",
    "f09f98",
    "eda080",
    "edbfbf",
    "c0af",
    "e080af",
    "f4908080",
    "f8888080af",
    "80",
    "bf80",
    "61ff62c3a9e2",
    "efbbbf61",
];

/** Deterministic bytes: SHA-256 of `${label}:${counter}` for counter = 0, 1, ... */
function byteStream(label) {
    let counter = 0;
    let pool = Buffer.alloc(0);
    return (n) => {
        while (pool.length < n) pool = Buffer.concat([pool, crypto.createHash("sha256").update(`${label}:${counter++}`).digest()]);
        const out = Buffer.from(pool.subarray(0, n));
        pool = pool.subarray(n);
        return out;
    };
}

/** A frame's bytes as hex; the lengths default to those of the path and the socket id. */
function frame({ version = 0, type = 3, id = 1, status = 200, path = "", socket = "", payload = "", pathLength, socketLength, interval = 0, timeout = 0 }) {
    const bytes = (value) => (Buffer.isBuffer(value) ? value : Buffer.from(value));
    const [p, s, body] = [bytes(path), bytes(socket), bytes(payload)];
    const header = Buffer.alloc(14);
    header.writeUInt8(version, 0);
    header.writeUInt8(type, 1);
    header.writeUInt32BE(id, 2);
    header.writeUInt16BE(status, 6);
    header.writeUInt8(pathLength ?? p.length, 8);
    header.writeUInt8(socketLength ?? s.length, 9);
    header.writeUInt16BE(interval, 10);
    header.writeUInt16BE(timeout, 12);
    return Buffer.concat([header, p, s, body]).toString("hex");
}

/** A socket id as the reference's listener makes one: `${Date.now()}:${server.info.id}:${counter}`. */
function socketId(serverId, counter) {
    return `1790000000000:${serverId}:${counter}`;
}

module.exports = async function wireNesVectors({ solar, rec, shared }) {
    const H = shared.height;
    const out = [];
    const parse = async (name, hex) => out.push({ ...(await rec("nes.parse", H, { hex })), name });
    const stringify = async (name, message, parseBack = false) => {
        const r = await rec("nes.stringify", H, { message });
        out.push({ ...r, name });
        if (parseBack && r.output) await parse(`${name}, parsed`, r.output.hex);
    };
    const payloadOf = byteStream("nes payload");

    // ---- The frames the reference sends on the peer-to-peer port ----

    // The client's hello (client.ts _hello; _send then sets id = ++ids, 1 on a new client).
    await stringify("sent: client hello", { type: "hello", version: "2", id: 1 }, true);
    // A request (client.ts request; _send sets the id) for every route, with a payload of codec
    // bytes (random here: the frame does not read them).
    for (const [i, routeId] of ROUTE_IDS.entries()) {
        const payload = payloadOf([2, 0, 9, 40, 300, 1, 77, 5000, 3, 12, 64, 128][i]).toString("hex");
        await stringify(`sent: request ${routeId}`, { type: "request", method: "post", path: routeId, payload, id: i + 2 }, true);
    }
    for (const id of [255, 256, 65536, 2 ** 31, 2 ** 32 - 1]) {
        await stringify(`sent: request id ${id}`, { type: "request", method: "post", path: "p2p.peer.getStatus", payload: "0a00", id }, true);
    }
    // The server's reply to a hello (socket.ts _processHello): the hello's id, the heartbeat and the
    // socket id. The P2P client refuses a socket id longer than 100 bytes, which a long host name
    // in server.info.id produces.
    const serverId = "relay:4242:mfz1q8k0";
    const long = (length) => socketId(serverId, 10000).padEnd(length, "0");
    for (const [name, socket] of [
        ["socket id", socketId(serverId, 10000)],
        ["socket id of 100 bytes", long(100)],
        ["socket id of 101 bytes", long(101)],
    ]) {
        await stringify(`sent: server hello, ${name}`, { type: "hello", id: 1, heartbeat: HEARTBEAT, socket }, true);
    }
    // A response (socket.ts _processRequest): the route's method, the request's id, the status
    // code and the route's result.
    for (const [id, size] of [
        [2, 0],
        [3, 2],
        [4, 200],
        [4294967295, 17],
    ]) {
        await stringify(`sent: response id ${id}`, { type: "request", method: "post", id, statusCode: 200, payload: payloadOf(size).toString("hex") }, true);
    }
    // The server's heartbeat (listener.ts _beat) and the client's reply (client.ts _onMessage;
    // _send sets id = ++ids).
    await stringify("sent: server ping", { type: "ping" }, true);
    await stringify("sent: client ping reply", { type: "ping", id: 14 }, true);
    // The graceful error to a peer outside remoteAccess: the close-connection plugin answers 499
    // with no result, socket.ts _processRequest turns it into a Boom error of JSON.stringify(null),
    // and _error sends the Boom payload without statusCode as a request-type message.
    {
        const nesDir = path.join(solar.solarDir, "packages", "nes");
        const Boom = require(require.resolve("@hapi/boom", { paths: [nesDir] }));
        const Hoek = require(require.resolve("@hapi/hoek", { paths: [nesDir] }));
        const result = null;
        const error = Boom.boomify(Boom.boomify(new Error(typeof result === "object" ? JSON.stringify(result) : result), { statusCode: 499 }));
        const message = Hoek.clone(error.output);
        delete message.payload.statusCode;
        const request = { type: "request", id: 3 };
        const body = Buffer.from(JSON.stringify(message.payload));
        if (body.toString() !== '{"error":"Unknown","message":"null"}') throw new Error(`unexpected graceful error body ${body}`);
        await stringify("sent: graceful error", { type: request.type, id: request.id, statusCode: message.statusCode, payload: body.toString("hex") }, true);
    }

    // ---- The parser ----

    for (let type = 0; type < 256; type++) await parse(`type byte ${type}`, frame({ type }));
    const hello = frame({ version: 2, type: 0 });
    for (let length = 0; length <= 14; length++) await parse(`${length} bytes of a hello`, hello.slice(0, 2 * length));
    await parse("header fields at their maximum", frame({ version: 255, type: 9, id: 2 ** 32 - 1, status: 65535, interval: 65535, timeout: 65535, payload: "\u0001\u0002" }));
    await parse("path runs past the end", frame({ path: "abcd", pathLength: 5 }));
    await parse("path of 255 bytes, none present", frame({ type: 0, pathLength: 255 }));
    await parse("socket id runs past the end", frame({ path: "a", socket: "ab", socketLength: 3 }));
    await parse("socket id of 100 bytes", frame({ type: 0, socket: "s".repeat(100), payload: "x" }));
    await parse("socket id of 101 bytes", frame({ type: 0, socket: "s".repeat(101) }));
    await parse("socket id of 255 bytes", frame({ type: 0, socket: "s".repeat(255) }));
    await parse("socket id of 100 invalid bytes", frame({ type: 0, socket: Buffer.alloc(100, 0xff) }));
    // A frame that is not a post and has a 255-byte path takes the payload's text into the path.
    const path255 = "a/../" + "p".repeat(250);
    for (const type of [0, 1, 9, 3]) {
        await parse(`255-byte path, type ${type}`, frame({ type, path: path255, payload: "xyz" }));
    }
    await parse("255-byte path, hello, invalid payload bytes", frame({ type: 0, path: path255, payload: Buffer.from("61ff2fc3", "hex") }));
    await parse("255-byte path, hello, no payload", frame({ type: 0, path: path255 }));
    await parse("255-byte path, ping, socket id", frame({ type: 1, path: "é".repeat(127) + "a", socket: "s:1", payload: " ./#?" }));
    await parse("254-byte path, hello", frame({ type: 0, path: "q".repeat(254), payload: "xyz" }));
    for (const p of URL_PATHS) await parse(`path ${JSON.stringify(p)}`, frame({ path: p, payload: "\u0001" }));
    const printable = byteStream("nes printable path");
    const alphabet = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789 !\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~..//%2e%2E";
    for (let i = 0; i < 40; i++) {
        const length = 1 + (printable(1)[0] % 80);
        const p = [...printable(length)].map((b) => alphabet[b % alphabet.length]).join("");
        await parse(`random printable path ${i}`, frame({ path: p }));
    }
    const random = byteStream("nes random path");
    for (let i = 0; i < 20; i++) {
        const length = 1 + (random(1)[0] % 64);
        await parse(`random byte path ${i}`, frame({ path: random(length), socket: random(random(1)[0] % 8) }));
    }
    for (const hex of INVALID_UTF8) {
        await parse(`path bytes ${hex}`, frame({ path: Buffer.from(hex, "hex") }));
        await parse(`socket id bytes ${hex}`, frame({ type: 0, socket: Buffer.from(hex, "hex") }));
    }

    // ---- The writer ----

    await stringify("writer defaults: hello without id or version", { type: "hello" }, true);
    await stringify("writer defaults: id 0 and status 0", { type: "error", id: 0, statusCode: 0 }, true);
    await stringify("writer defaults: request with id 0", { type: "request", method: "post", path: "p2p.peer.getPeers", id: 0 }, true);
    await stringify("method in capitals", { type: "request", method: "POST", path: "p2p.peer.getPeers", id: 9 }, true);
    await stringify("version 255", { type: "ping", version: "255", id: 2 }, true);
    await stringify("heartbeat with one value", { type: "hello", id: 1, heartbeat: { interval: 15000 } }, true);
    await stringify("heartbeat at its maximum", { type: "hello", id: 1, heartbeat: { interval: 65535, timeout: 65535 } }, true);
    await stringify("error with a path and a payload", { type: "error", id: 4, statusCode: 500, path: "p2p/x y", payload: "7b7d" }, true);
    // The cut of a long path into the payload, for a message without the post method.
    await stringify("hello, path of 255 UTF-16 units", { type: "hello", id: 1, path: "h".repeat(255) }, true);
    await stringify("hello, path of 256 UTF-16 units, payload replaced", { type: "hello", id: 1, path: "h".repeat(256), payload: "ff" }, true);
    await stringify("ping, path of 300 UTF-16 units", { type: "ping", id: 1, path: "p".repeat(299) + "/" }, true);
    await stringify("error, path cut before non-ASCII text", { type: "error", id: 1, path: "e".repeat(255) + "é\u{1F600}./" }, true);
    await stringify("request, path of 256 UTF-16 units", { type: "request", method: "post", id: 1, path: "r".repeat(256) });
    await stringify("hello, cut through a surrogate pair", { type: "hello", id: 1, path: "a".repeat(254) + "\u{1F600}b" });
    await stringify("request, path of 254 bytes of UTF-8", { type: "request", method: "post", id: 1, path: "é".repeat(127) }, true);
    await stringify("request, path of 256 bytes of UTF-8", { type: "request", method: "post", id: 1, path: "é".repeat(128) });
    await stringify("hello, path of 400 bytes of UTF-8", { type: "hello", id: 1, path: "é".repeat(200) });
    await stringify("ping, path of 252 bytes in surrogate pairs", { type: "ping", id: 1, path: "\u{1F600}".repeat(63) }, true);
    await stringify("ping, path of 256 bytes in surrogate pairs", { type: "ping", id: 1, path: "\u{1F600}".repeat(64) });
    await stringify("socket id of 255 bytes", { type: "hello", id: 1, socket: "s".repeat(255) }, true);
    await stringify("socket id of 256 bytes", { type: "hello", id: 1, socket: "s".repeat(256) });
    await stringify("socket id of 100 bytes of UTF-8", { type: "hello", id: 1, socket: "é".repeat(50) }, true);
    await stringify("path and socket id both too long", { type: "request", method: "post", id: 1, path: "p".repeat(256), socket: "s".repeat(256) });

    return out;
};
