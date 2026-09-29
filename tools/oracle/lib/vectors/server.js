"use strict";
// V31, the server's transport: p2p.serveRequest over the request shapes of V24 and each refusal
// (the version gate, the schemas, internal routes from outside, the payload limits, requests the
// codecs refuse, not ready, unknown routes and paths), with the answers, the ban reasons,
// `connection: close`, the peer acceptance's port and a known peer's version; and the inbound
// limiter's sequences (ratelimit.run with limiter.inbound). Every record carries the devnet's
// milestones, so that a reader without the oracle's devnet builds the same version gate.
// Addresses are documentation ranges.

const path = require("path");

const HEARTWOOD = "4.3.1-heartwood.0.1.0";
const HEADERS = { version: HEARTWOOD, port: 4002 };
const OUTSIDER = "192.0.2.10";
const FORGER = "203.0.113.5";
const REMOTE_ACCESS = [FORGER];

module.exports = async function serverVectors({ solar, devnet, rec, shared }) {
    const out = [];
    const H = shared.height;
    const Codecs = require(path.join(solar.solarDir, "packages", "p2p", "dist", "socket-server", "codecs")).Codecs;
    const milestones = JSON.stringify(devnet.config.milestones);
    const hex = (bytes) => Buffer.from(bytes).toString("hex");
    const encode = (route, payload) => Codecs[route].request.serialise(payload);
    const serve = async (name, route, bytes, extra = {}) => {
        const input = { route, hex: hex(bytes), ip: OUTSIDER, remoteAccess: REMOTE_ACCESS, booted: true, milestones, ...extra };
        out.push({ ...(await rec("p2p.serveRequest", H, input)), name });
    };
    const id = (n) => n.toString(16).padStart(2, "0").repeat(32);
    const forger = { ip: FORGER };

    // ---- the public routes' requests, accepted, from a peer and from the forger ------------------
    const valid = {
        getStatus: { headers: HEADERS },
        getPeers: { headers: HEADERS },
        getCommonBlocks: { ids: [id(1), id(2)], headers: HEADERS },
        getBlocks: { lastBlockHeight: 5, blockLimit: 400, serialised: true, headers: HEADERS },
        postBlock: { block: Buffer.from("00010203", "hex"), headers: HEADERS },
        getUnconfirmedTransactions: { exclude: [id(3)], headers: HEADERS },
        postTransactions: { transactions: [Buffer.from("aa", "hex"), Buffer.from("bbcc", "hex")], headers: HEADERS },
    };
    const routeId = { getStatus: "peer", getPeers: "peer", getCommonBlocks: "peer", getBlocks: "blocks", postBlock: "blocks" };
    const idOf = (route) => `p2p.${routeId[route] || "transactions"}.${route}`;
    for (const [route, payload] of Object.entries(valid)) {
        await serve(`${route}: accepted`, idOf(route), encode(route, payload));
        await serve(`${route}: from the forger`, idOf(route), encode(route, payload), forger);
        await serve(`${route}: a known peer's version`, idOf(route), encode(route, payload), { known: true });
    }
    for (const port of [0, 1, 6002, 65535, 65536]) {
        await serve(`getStatus: headers port ${port}`, idOf("getStatus"), encode("getStatus", { headers: { version: HEARTWOOD, port } }));
    }
    await serve("postBlock: no headers", idOf("postBlock"), encode("postBlock", { block: Buffer.from("01", "hex") }));
    await serve("postTransactions: no headers", idOf("postTransactions"), encode("postTransactions", { transactions: [Buffer.from("01", "hex")] }));
    await serve("getUnconfirmedTransactions: no headers", idOf("getUnconfirmedTransactions"), encode("getUnconfirmedTransactions", { exclude: [] }));
    const transactions = (n) => Array.from({ length: n }, (_, i) => Buffer.from([i]));
    for (const n of [0, 40, 41]) {
        await serve(`postTransactions: ${n} transactions`, idOf("postTransactions"), encode("postTransactions", { transactions: transactions(n), headers: HEADERS }));
    }
    await serve("postTransactions: 3 transactions over a limit of 2", idOf("postTransactions"), encode("postTransactions", { transactions: transactions(3), headers: HEADERS }), { maxTransactionsPerRequest: 2 });

    // ---- the schemas' refusals -------------------------------------------------------------------
    for (const route of ["getStatus", "getPeers", "getCommonBlocks", "getBlocks"]) {
        const { headers, ...rest } = valid[route];
        // An empty message is no payload at all: an unknown field stands in for the headers.
        const bytes = encode(route, rest);
        await serve(`${route}: no headers`, idOf(route), bytes.length ? bytes : Buffer.from("7801", "hex"));
    }
    for (const route of Object.keys(valid)) {
        await serve(`${route}: headers without a version`, idOf(route), encode(route, { ...valid[route], headers: { port: 4002 } }));
        await serve(`${route}: an empty version`, idOf(route), encode(route, { ...valid[route], headers: { version: "", port: 4002 } }));
    }
    for (const countOnly of [false, true]) {
        await serve(`getUnconfirmedTransactions: countOnly ${countOnly}`, idOf("getUnconfirmedTransactions"), encode("getUnconfirmedTransactions", { countOnly, exclude: [], headers: HEADERS }));
    }
    const ids = {
        "no ids": [],
        "10 ids": Array.from({ length: 10 }, (_, i) => id(i)),
        "11 ids": Array.from({ length: 11 }, (_, i) => id(i)),
        "uppercase hex": [id(10).toUpperCase()],
        "63 characters": [id(1).slice(1)],
        "65 characters": [id(1) + "a"],
        "not hex": ["g".repeat(64)],
    };
    for (const [name, list] of Object.entries(ids)) {
        await serve(`getCommonBlocks: ${name}`, idOf("getCommonBlocks"), encode("getCommonBlocks", { ids: list, headers: HEADERS }));
    }
    for (const [lastBlockHeight, blockLimit] of [[0, 400], [1, 0], [1, 401], [4294967295, 1], [undefined, 400], [5, undefined]]) {
        const payload = { serialised: true, headers: HEADERS };
        if (lastBlockHeight !== undefined) payload.lastBlockHeight = lastBlockHeight;
        if (blockLimit !== undefined) payload.blockLimit = blockLimit;
        await serve(`getBlocks: after ${lastBlockHeight}, limit ${blockLimit}`, idOf("getBlocks"), encode("getBlocks", payload));
    }

    // ---- the version gate: the devnet's floor, then the option under a milestone without one -----
    for (const version of ["4.2.9", "4.3.0-next.9", "4.3.0", HEARTWOOD, "5.0.0", "not.a.version", "v4.3.1"]) {
        await serve(`getStatus: version ${version}`, idOf("getStatus"), encode("getStatus", { headers: { version, port: 4002 } }));
    }
    const unfloored = JSON.stringify(devnet.config.milestones.map(({ p2p, ...milestone }) => milestone));
    for (const minimumVersions of [undefined, [">=4.3.0"], [">=5.0.0"], []]) {
        const extra = { milestones: unfloored, ...(minimumVersions ? { minimumVersions } : {}) };
        await serve(`getPeers: option ${JSON.stringify(minimumVersions)}, no floor`, idOf("getPeers"), encode("getPeers", { headers: HEADERS }), extra);
    }
    await serve("getStatus: a bad version and no ids", idOf("getCommonBlocks"), encode("getCommonBlocks", { headers: { version: "1.0.0" } }));

    // ---- what the codecs refuse, and the payload limits -----------------------------------------
    for (const [name, bytes] of [["an empty payload", ""], ["a cut field", "0a05"], ["wire type 7", "0f"], ["an end-group tag", "0c"], ["a cut varint", "08ff"]]) {
        for (const route of ["getStatus", "postTransactions"]) await serve(`${route}: ${name}`, idOf(route), Buffer.from(bytes, "hex"));
    }
    // A valid request padded with an unknown bytes field (tag, two-byte length) to `total` bytes.
    const padded = (bytes, total) => {
        const length = total - bytes.length - 3;
        return Buffer.concat([bytes, Buffer.from([0x7a, (length & 0x7f) | 0x80, length >> 7]), Buffer.alloc(length, 0)]);
    };
    await serve("getStatus: 1024 bytes", idOf("getStatus"), padded(encode("getStatus", { headers: HEADERS }), 1024));
    await serve("getStatus: 1025 bytes", idOf("getStatus"), Buffer.alloc(1025, 0));
    await serve("getCommonBlocks: 10241 bytes", idOf("getCommonBlocks"), Buffer.alloc(10241, 0));
    await serve("getStatus: 1025 bytes from the forger", idOf("getStatus"), Buffer.alloc(1025, 0), forger);

    // ---- the internal routes ---------------------------------------------------------------------
    const internal = ["emitEvent", "getCurrentRound", "getSlotNumber", "getNetworkState", "syncBlockchain"];
    const json = (value) => Buffer.from(JSON.stringify(value));
    for (const route of internal) {
        const payload = route === "emitEvent" ? json({ event: "block.forged", body: { height: 2 } }) : json({});
        await serve(`${route}: from outside`, `p2p.internal.${route}`, payload);
        await serve(`${route}: from the forger`, `p2p.internal.${route}`, payload, forger);
        await serve(`${route}: not ready`, `p2p.internal.${route}`, payload, { ...forger, booted: false });
    }
    for (const [name, text] of [["no payload", ""], ["cut JSON", "{"], ["a number", "5"], ["null", "null"], ["a string", '"x"']]) {
        await serve(`getSlotNumber: ${name}`, "p2p.internal.getSlotNumber", Buffer.from(text), forger);
    }
    const emits = {
        "no event": {},
        "an empty event": { event: "" },
        "a numeric event": { event: 5 },
        "an array body": { event: "a", body: [] },
        "a text body": { event: "a", body: "{}" },
        "a null body": { event: "a", body: null },
        "another key": { event: "a", other: 1 },
        "any event name": { event: "anything", body: { nested: [1, { a: null }] } },
    };
    for (const [name, value] of Object.entries(emits)) await serve(`emitEvent: ${name}`, "p2p.internal.emitEvent", json(value), forger);
    for (const [name, text] of [["null", "null"], ["an array", "[]"], ["a number", "7"]]) {
        await serve(`emitEvent: ${name}`, "p2p.internal.emitEvent", Buffer.from(text), forger);
    }
    for (const [name, version] of [["a valid version", HEARTWOOD], ["an old version", "1.0.0"], ["an empty version", ""], ["a number", 5], ["zero", 0], ["false", false], ["an object", {}]]) {
        await serve(`getNetworkState: headers with ${name}`, "p2p.internal.getNetworkState", json({ log: true, headers: { version } }), forger);
    }
    await serve("getSlotNumber: over 1 MiB", "p2p.internal.getSlotNumber", json({ pad: "x".repeat(1048576) }), forger);
    await serve("getSlotNumber: 1 MiB", "p2p.internal.getSlotNumber", Buffer.from(`{"pad":"${"x".repeat(1048576 - 10)}"}`), forger);

    // ---- readiness -------------------------------------------------------------------------------
    for (const route of ["getStatus", "postBlock"]) {
        const bytes = encode(route, valid[route]);
        await serve(`${route}: not ready`, idOf(route), bytes, { booted: false });
        await serve(`${route}: not ready, from the forger`, idOf(route), bytes, { ...forger, booted: false });
        await serve(`${route}: not ready, a known peer`, idOf(route), bytes, { booted: false, known: true });
    }
    await serve("getStatus: not ready, a bad version", idOf("getStatus"), encode("getStatus", { headers: { version: "1.0.0" } }), { booted: false });

    // ---- routes and paths ------------------------------------------------------------------------
    const status = encode("getStatus", valid.getStatus);
    for (const route of ["p2p.peer.nothing", "P2P.peer.getStatus", "", "/p2p/peer/getStatus", "/p2p/peer/getStatus?x=1", "/nothing", "p2p.peer.getStatus/", "./p2p.peer.getStatus", "p2p/peer/getStatus"]) {
        await serve(`path ${JSON.stringify(route)}`, route, status);
        await serve(`path ${JSON.stringify(route)} from the forger`, route, status, forger);
    }

    // ---- the inbound limiter ----------------------------------------------------------------------
    const limiter = { inbound: true, rateLimit: 100, rateLimitPostTransactions: 25, remoteAccess: REMOTE_ACCESS };
    const run = async (name, events, limits = limiter) => {
        out.push({ ...(await rec("ratelimit.run", H, { limiter: limits, events })), name: `inbound limiter: ${name}` });
    };
    const burst = (route, n, t = 0, ip = OUTSIDER) => Array.from({ length: n }, () => ({ t, ip, route, op: "consume" }));
    for (const [route, points, window] of [
        ["postBlock", 2, 2000],
        ["getBlocks", 2, 1000],
        ["getPeers", 2, 1000],
        ["getStatus", 9, 1000],
        ["getCommonBlocks", 9, 1000],
        ["getUnconfirmedTransactions", 2, 1000],
        ["postTransactions", 25, 1000],
    ]) {
        await run(`${route} over its limit and at its window's end`, [
            ...burst(route, points + 1),
            ...burst(route, 1, window - 1),
            ...burst(route, 1, window),
            ...burst(route, points + 1, window + 1),
        ]);
        await run(`${route} from the forger`, burst(route, points + 3, 0, FORGER));
    }
    // The oracle's limiter knows the public routes only; the global limit counts refused points.
    const mixed = ["getStatus", "getCommonBlocks", "postTransactions", "getPeers"];
    await run("the global limit over every route", Array.from({ length: 102 }, (_, i) => ({ t: i, ip: OUTSIDER, route: mixed[i % 4], op: "consume" })));
    await run("the global limit of 3", [...burst("getStatus", 2), ...burst("getPeers", 2), ...burst("getStatus", 1, 999), ...burst("getStatus", 1, 1000)], { ...limiter, rateLimit: 3 });
    await run("postTransactions of 0 reads as 25", burst("postTransactions", 26), { ...limiter, rateLimitPostTransactions: 0 });
    await run("postTransactions of 3", burst("postTransactions", 5), { ...limiter, rateLimitPostTransactions: 3 });
    await run("two addresses", [...burst("getPeers", 3), ...burst("getPeers", 3, 1, "192.0.2.11")]);
    return out;
};
