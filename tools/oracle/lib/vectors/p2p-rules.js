"use strict";
// Vector classes of the peer protocol's rules, each a builder of records:
//   exchange  V24-p2p-exchange: every request shape the client writes, which a reference server
//             accepts (p2p.validateRequest); the ban-trap shapes, which it refuses and the client
//             cannot write; the version floor below, at and above; the replies of the six routes
//             (wire.checkReply): valid, each schema rule broken once, the edges of their caps, the
//             generator key's change, a seeded set of peer addresses, and replies whose route a
//             selector byte picks
//   rules     V25-peer-rules: peer.isValid over every ipaddr.js range and address form, the
//             private switch, the host's own addresses and a seeded set of texts;
//             peer.isValidVersion over the devnet's and the default ranges, prereleases on and off
//             mainnet, invalid semver, and a seeded set of range forms; peer.sameSubnet;
//             ratelimit.run sequences of each outgoing rule across window edges, and a seeded set
//             over every route
// Addresses are documentation ranges, except those that classify a range.

const path = require("path");

// The version a Heartwood node reports, and the reference's.
const HEARTWOOD = "4.3.1-heartwood.0.1.0";
const HEADERS = { version: HEARTWOOD, port: 4002 };
// The reference's default `minimumVersions` option.
const DEFAULT_RANGES = ["^3.2", "^3.2.0-next.0"];

// Protobuf writing, for replies the reference's encoders cannot write.
function varint(value) {
    const out = [];
    let v = BigInt(value);
    do {
        let byte = Number(v & 0x7fn);
        v >>= 7n;
        if (v) byte |= 0x80;
        out.push(byte);
    } while (v);
    return Buffer.from(out);
}
const tag = (field, wireType) => varint((field << 3) | wireType);
const uint = (field, value) => Buffer.concat([tag(field, 0), varint(value)]);
const bytes = (field, buffer) => Buffer.concat([tag(field, 2), varint(buffer.length), buffer]);
const text = (field, value) => bytes(field, Buffer.from(value, "utf8"));

/** A seeded generator (mulberry32), so the generated records are the same on every run. */
function seeded(seed) {
    let a = seed >>> 0;
    const next = () => {
        a = (a + 0x6d2b79f5) >>> 0;
        let t = a;
        t = Math.imul(t ^ (t >>> 15), t | 1);
        t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
        return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
    };
    return { below: (n) => Math.floor(next() * n), pick: (list) => list[Math.floor(next() * list.length)] };
}

async function exchange({ solar, devnet, rec, shared }) {
    const out = [];
    const H = shared.height;
    const Codecs = require(path.join(solar.solarDir, "packages", "p2p", "dist", "socket-server", "codecs")).Codecs;
    const push = async (name, op, input) => out.push({ ...(await rec(op, H, input)), name });
    const request = (name, route, payload, extra = {}) => push(`request ${route}: ${name}`, "p2p.validateRequest", { route, payload, ...extra });
    const reply = (name, route, payload, extra = {}) =>
        push(`reply ${route}: ${name}`, "wire.checkReply", { route, hex: Buffer.from(payload).toString("hex"), ...extra });
    const encoded = (route, value) => Codecs[route].response.serialise(value);
    const id = (n) => n.toString(16).padStart(2, "0").repeat(32);
    const txs = ["transfer-1", "vote-2", "burn", "transfer-2"].map((n) => shared.tx[n].hex);

    // ---- requests the client writes, each accepted ----------------------------------------------
    for (const headers of [HEADERS, { version: "4.3.1", port: 4002 }, { version: HEARTWOOD, port: 0 }, { version: "4.3.0", port: 65535 }]) {
        const v = `v${headers.version} port ${headers.port}`;
        await request(v, "getStatus", { headers });
        await request(v, "getPeers", { headers });
    }
    for (const n of [1, 9, 10]) await request(`${n} ids`, "getCommonBlocks", { ids: Array.from({ length: n }, (_, i) => id(i + 1)), headers: HEADERS });
    for (const [lastBlockHeight, blockLimit, headersOnly] of [
        [1, 400, undefined],
        [264, 400, undefined],
        [4294967295, 1, undefined],
        [53, 1, true],
        [53, 400, true],
    ]) {
        const payload = { lastBlockHeight, blockLimit, ...(headersOnly ? { headersOnly } : {}), serialised: true, headers: HEADERS };
        await request(`after ${lastBlockHeight}, ${blockLimit}${headersOnly ? ", headers only" : ""}`, "getBlocks", payload);
    }
    for (const n of [0, 1, 100]) await request(`exclude ${n}`, "getUnconfirmedTransactions", { exclude: Array.from({ length: n }, (_, i) => id(i)), headers: HEADERS });
    const block = shared.blocks.statusBlocks[1].output.hex;
    await request("a block with transactions", "postBlock", { block, headers: HEADERS });
    await request("an empty block field", "postBlock", { block: "", headers: HEADERS });
    await request("1 transaction", "postTransactions", { transactions: [txs[0]], headers: HEADERS });
    await request("40 transactions", "postTransactions", { transactions: Array.from({ length: 40 }, (_, i) => txs[i % 4]), headers: HEADERS });

    // ---- the ban traps: refused by a reference server, and not expressible by the client ---------
    for (const route of ["getStatus", "getPeers"]) await request("no headers", route, {});
    await request("no headers", "getCommonBlocks", { ids: [id(1)] });
    await request("no headers", "getBlocks", { lastBlockHeight: 5, blockLimit: 400, serialised: true });
    for (const route of ["getStatus", "getPeers"]) {
        await request("headers without a version", route, { headers: { port: 4002 } });
        await request("an empty version", route, { headers: { version: "", port: 4002 } });
    }
    await request("an empty version", "postBlock", { block, headers: { version: "", port: 4002 } });
    await request("an empty version", "postTransactions", { transactions: [txs[0]], headers: { version: "", port: 4002 } });
    await request("countOnly false", "getUnconfirmedTransactions", { countOnly: false, exclude: [], headers: HEADERS });
    await request("countOnly true", "getUnconfirmedTransactions", { countOnly: true, exclude: [], headers: HEADERS });
    await request("no ids", "getCommonBlocks", { ids: [], headers: HEADERS });
    await request("ids absent", "getCommonBlocks", { headers: HEADERS });
    await request("blockLimit 0", "getBlocks", { lastBlockHeight: 5, blockLimit: 0, serialised: true, headers: HEADERS });
    await request("blockLimit absent", "getBlocks", { lastBlockHeight: 5, serialised: true, headers: HEADERS });
    await request("lastBlockHeight 0", "getBlocks", { lastBlockHeight: 0, blockLimit: 400, serialised: true, headers: HEADERS });
    await request("lastBlockHeight absent", "getBlocks", { blockLimit: 400, serialised: true, headers: HEADERS });

    // ---- the version floor: the devnet's `>=4.3.0`, prereleases included ---------------------------
    for (const version of ["4.2.9", "4.3.0-next.9", "4.3.0", HEARTWOOD, "4.4.0-rc.1", "5.0.0", "not.a.version"]) {
        await request(`version ${version}`, "getStatus", { headers: { version, port: 4002 } });
    }
    // Under a milestone without a floor, the server's option decides.
    const unfloored = JSON.stringify(devnet.config.milestones.map(({ p2p, ...milestone }) => milestone));
    for (const minimumVersions of [DEFAULT_RANGES, [">=4.3.0"], [">=4.3.0-0"]]) {
        await request(`option ${minimumVersions.join(" ")}, no floor`, "getPeers", { headers: HEADERS }, { minimumVersions, milestones: unfloored });
    }

    // ---- getPeers replies -----------------------------------------------------------------------------
    const peer = (ip, port) => Buffer.concat([ip === undefined ? Buffer.alloc(0) : text(1, ip), port === undefined ? Buffer.alloc(0) : uint(2, port)]);
    const peers = (list) => Buffer.concat(list.map((p) => bytes(1, p)));
    await reply("empty", "getPeers", Buffer.alloc(0));
    await reply("IPv4 and IPv6", "getPeers", encoded("getPeers", [{ ip: "192.0.2.1", port: 4002 }, { ip: "2001:db8::1", port: 0 }, { ip: "198.51.100.7", port: 65535 }]));
    for (const ip of ["001.002.003.004", "256.0.0.1", "1.2.3", "192.0.2.1 ", " ::1", "::1 ", "fe80::1%eth0", "fe80::1%", "::ffff:192.0.2.1", "::192.0.2.1", "1::2::3", "::", "a:b:c:d:e:f:1.2.3.4", "a:b:c:d:e:f:g:1", "1:2:3:4:5:6:7::", "1:2:3:4:5:6::1.2.3.4", "::01.2.3.4", "ABCD::EF01", "x"]) {
        await reply(`address ${JSON.stringify(ip)}`, "getPeers", peers([peer(ip, 4002)]));
    }
    const rand = seeded(0xcafe);
    const part = () => rand.pick(["0", "1", "10", "255", "256", "010", "08", "001", "ffff", "FFFF", "fe80", "2001", "db8", "12345", "", "a", "g", "1.2.3.4", "01.2.3.4"]);
    for (let i = 0; i < 200; i++) {
        const body = rand.below(3) === 0 ? Array.from({ length: 1 + rand.below(5) }, part).join(".") : Array.from({ length: 1 + rand.below(9) }, part).join(rand.pick([":", ":", "::"]));
        const ip = rand.pick(["", "", " ", "\t"]) + body + rand.pick(["", "", "%eth0", "%", " ", "% ", "%a\n", "%\n"]);
        await reply(`seeded address ${JSON.stringify(ip)}`, "getPeers", peers([peer(ip, 4002)]));
    }
    await reply("no port", "getPeers", peers([peer("192.0.2.1")]));
    await reply("no address", "getPeers", peers([peer(undefined, 4002)]));
    await reply("port 65536", "getPeers", peers([peer("192.0.2.1", 65536)]));
    await reply("an empty entry", "getPeers", peers([Buffer.alloc(0)]));
    const many = (n) => Array.from({ length: n }, (_, i) => ({ ip: `198.51.100.${i % 256}`, port: 4000 + (i % 100) }));
    await reply("2000 entries", "getPeers", encoded("getPeers", many(2000)));
    await reply("2001 entries", "getPeers", encoded("getPeers", many(2001)));

    // ---- getCommonBlocks replies -------------------------------------------------------------------
    const common = (fields) => bytes(1, Buffer.concat(fields));
    await reply("common null", "getCommonBlocks", Buffer.alloc(0));
    await reply("a common block", "getCommonBlocks", encoded("getCommonBlocks", { common: { height: 7, id: id(7) } }));
    await reply("an uppercase id", "getCommonBlocks", encoded("getCommonBlocks", { common: { height: 1, id: id(0xab).toUpperCase() } }));
    await reply("height 0", "getCommonBlocks", common([uint(1, 0), text(2, id(7))]));
    await reply("no height", "getCommonBlocks", common([text(2, id(7))]));
    await reply("no id", "getCommonBlocks", common([uint(1, 7)]));
    await reply("a short id", "getCommonBlocks", common([uint(1, 7), text(2, id(7).slice(1))]));
    await reply("a non-hex id", "getCommonBlocks", common([uint(1, 7), text(2, "g" + id(7).slice(1))]));
    await reply("an empty common", "getCommonBlocks", common([]));

    // ---- getBlocks replies ---------------------------------------------------------------------------
    const blockJson = (b) => b.output.json;
    const [first, second] = shared.blocks.statusBlocks.map(blockJson);
    const headerOf = (json, changes = {}) => {
        const keys = ["id", "version", "timestamp", "previousBlock", "height", "numberOfTransactions", "totalAmount", "totalFee", "reward", "payloadLength", "payloadHash", "generatorPublicKey", "blockSignature"];
        const header = Object.fromEntries(keys.map((k) => [k, json[k]]));
        for (const [key, value] of Object.entries(changes)) {
            if (value === undefined) delete header[key];
            else header[key] = value;
        }
        return { ...header, totalAmount: header.totalAmount ?? "", totalFee: header.totalFee ?? "", reward: header.reward ?? "" };
    };
    const listed = (changes, json = second, transactions = []) => ({ ...headerOf(json, changes), transactions });
    const blocks = (list) => encoded("getBlocks", list);
    await reply("empty", "getBlocks", Buffer.alloc(0));
    await reply("two blocks", "getBlocks", blocks([listed({}, first), listed({}, second, txs.slice(0, 2))]));
    await reply("an uppercase generator key", "getBlocks", blocks([listed({ generatorPublicKey: second.generatorPublicKey.toUpperCase() })]));
    for (const [name, changes] of [
        ["no id", { id: undefined }],
        ["a short id", { id: second.id.slice(1) }],
        ["no timestamp", { timestamp: undefined }],
        ["no previous block", { previousBlock: undefined }],
        ["an empty previous block", { previousBlock: "" }],
        ["no height", { height: undefined }],
        ["height 0", { height: 0 }],
        ["no generator key", { generatorPublicKey: undefined }],
        ["a 65-character generator key", { generatorPublicKey: second.generatorPublicKey.slice(1) }],
        ["a non-hex generator key", { generatorPublicKey: "zz" + second.generatorPublicKey.slice(2) }],
        ["no signature", { blockSignature: undefined }],
        ["a 127-character signature", { blockSignature: second.blockSignature.slice(1) }],
        ["an empty payload hash", { payloadHash: "" }],
        ["a non-hex payload hash", { payloadHash: "0x12" }],
        ["no payload hash", { payloadHash: undefined }],
        ["a negative fee", { totalFee: "-1" }],
        ["a total amount of 2^63", { totalAmount: "9223372036854775808" }],
        ["a reward of 2^63", { reward: "9223372036854775808" }],
        ["a reward of 2^63 − 1", { reward: "9223372036854775807" }],
        ["an amount in hex", { totalAmount: "0x10" }],
        ["an amount with white space", { totalFee: " 12\n" }],
        ["an amount beyond 128 bits", { totalAmount: (1n << 130n).toString() }],
    ]) {
        await reply(name, "getBlocks", blocks([listed(changes)]));
    }
    // At height 1 the previous block may read as 0, and the totals are unbounded.
    for (const [name, changes] of [
        ["genesis with an empty previous block", { height: 1, previousBlock: "" }],
        ["genesis with previous block 0x0", { height: 1, previousBlock: "0x0" }],
        ["genesis with a total amount of 2^63", { height: 1, totalAmount: "9223372036854775808" }],
        ["genesis with a reward of 2^63", { height: 1, reward: "9223372036854775808" }],
    ]) {
        await reply(name, "getBlocks", blocks([listed(changes)]));
    }
    await reply("the second of two refused", "getBlocks", blocks([listed({ generatorPublicKey: second.generatorPublicKey.toUpperCase() }), listed({ height: undefined })]));
    const repeated = (n) => Array.from({ length: n }, () => listed({}));
    await reply("400 blocks", "getBlocks", blocks(repeated(400)));
    await reply("401 blocks", "getBlocks", blocks(repeated(401)));

    // ---- postBlock replies -------------------------------------------------------------------------------
    await reply("status and height", "postBlock", encoded("postBlock", { status: true, height: 264 }));
    await reply("height only", "postBlock", encoded("postBlock", { height: 1 }));
    await reply("status only", "postBlock", encoded("postBlock", { status: true }));
    await reply("height 0", "postBlock", uint(2, 0));
    await reply("empty", "postBlock", Buffer.alloc(0));

    // ---- getUnconfirmedTransactions replies --------------------------------------------------------------
    const pool = (value) => encoded("getUnconfirmedTransactions", value);
    const txList = (n) => Array.from({ length: n }, (_, i) => Buffer.from(txs[i % 4], "hex"));
    await reply("empty", "getUnconfirmedTransactions", Buffer.alloc(0));
    await reply("a pool size and transactions", "getUnconfirmedTransactions", pool({ poolSize: 9, transactions: txList(4) }));
    await reply("with the server's headers", "getUnconfirmedTransactions", pool({ transactions: txList(1), headers: { version: "4.3.1", port: 4002 } }));
    await reply("150 transactions", "getUnconfirmedTransactions", pool({ transactions: txList(150) }));
    await reply("151 transactions", "getUnconfirmedTransactions", pool({ transactions: txList(151) }));
    await reply("5 of a limit of 5", "getUnconfirmedTransactions", pool({ transactions: txList(5) }), { maxTransactions: 5 });
    await reply("6 of a limit of 5", "getUnconfirmedTransactions", pool({ transactions: txList(6) }), { maxTransactions: 5 });

    // ---- postTransactions replies -------------------------------------------------------------------------
    await reply("empty", "postTransactions", Buffer.alloc(0));
    await reply("two ids", "postTransactions", encoded("postTransactions", [id(1), id(2)]));

    // ---- the selector byte -----------------------------------------------------------------------------------
    const selected = (name, index, payload) => push(`selected ${name}`, "wire.checkReply", { hex: Buffer.concat([Buffer.from([index]), payload]).toString("hex") });
    await push("selected nothing", "wire.checkReply", { hex: "" });
    await selected("getPeers", 0, encoded("getPeers", [{ ip: "192.0.2.1", port: 4002 }]));
    await selected("getCommonBlocks by 7", 7, encoded("getCommonBlocks", { common: { height: 3, id: id(3) } }));
    await selected("getBlocks by 254", 254, blocks([listed({})]));
    await selected("postBlock", 3, encoded("postBlock", { status: false, height: 3 }));
    await selected("getUnconfirmedTransactions", 4, pool({ transactions: txList(2) }));
    await selected("postTransactions", 5, encoded("postTransactions", [id(9)]));
    await selected("postBlock by 255", 255, encoded("postBlock", { status: true, height: 9 }));
    await selected("a malformed getBlocks reply", 2, Buffer.from("00000005ffffffffff", "hex"));
    return out;
}

async function rules({ rec, shared }) {
    const out = [];
    const H = shared.height;
    const push = async (name, op, input) => out.push({ ...(await rec(op, H, input)), name });
    const rand = seeded(0x5eed);

    // ---- peer.isValid: ipaddr.js's ranges, forms and texts -------------------------------------------
    const addresses = [
        // IPv4 ranges.
        "0.1.2.3", "255.255.255.255", "224.0.0.1", "239.255.255.255", "169.254.1.1", "127.0.0.1", "100.64.0.1", "100.127.255.255",
        "100.128.0.1", "10.0.0.1", "172.16.0.1", "172.31.255.255", "172.32.0.1", "192.168.1.1", "192.0.0.1", "192.0.2.1",
        "192.88.99.1", "198.51.100.1", "203.0.113.1", "240.0.0.1", "8.8.8.8", "1.1.1.1",
        // IPv4 forms.
        "010.0.0.1", "0x7f.1", "0X7F.0.0.1", "3232235777", "8.8.2056", "8.524296", "08.8.8.8", "8.8.8.8.8", "8.8.8.256", "8.8.8.", "",
        "4294967296", "8.16777215", "8.16777216", " 8.8.8.8", "8.8.8.8\n",
        // IPv6 ranges.
        "::", "::1", "fe80::1", "febf::1", "fec0::1", "ff02::1", "fc00::1", "fd12:3456::1", "::ffff:8.8.8.8", "::ffff:0:808:808",
        "64:ff9b::808:808", "2002::1", "2001::1", "2001:0:1::1", "2001:db8::1", "2001:db9::1", "2a00:1450:4001::200e",
        // IPv6 forms.
        "2a00:1450:4001:0:0:0:0:200e", "2A00:1450:4001::200E", "1:0:2:0:0:0:0:3", "0:0:0:0:0:0:0:1", "::8.8.8.8", "::0x8.8.8.8",
        "1::8.8.8.8", "2a00:1450:4001:0:0:0:8.8.8.8", "2a00::1%eth0", "fe80::1%eth0", "fe80::1%", "fe80::1%et-h", "2a00:0:0:0:0:0:0:0%x",
        "2a00:::1", "2a00::1::1", "2a00:1450:4001:0:0:0:0:200e:1", "2a00:1450", "12345::1", "::g",
    ];
    for (const ip of addresses) {
        for (const allowPrivate of [false, true]) await push(`address ${JSON.stringify(ip)}${allowPrivate ? ", private allowed" : ""}`, "peer.isValid", { ip, allowPrivate, interfaces: [] });
    }
    // A seeded set of address texts built from the parts of both forms.
    const part = () => rand.pick(["0", "1", "10", "255", "256", "010", "08", "0x1f", "0X1F", "ffff", "FFFF", "fe80", "2001", "db8", "12345", "", "g", "4294967295", "16777215"]);
    const seededAddress = () => {
        const kind = rand.below(4);
        const text = [
            () => Array.from({ length: 1 + rand.below(5) }, part).join("."),
            () => Array.from({ length: 1 + rand.below(9) }, part).join(rand.pick([":", ":", "::"])),
            () => rand.pick(["::", "::ffff:", "1::", "", "2001:db8::"]) + Array.from({ length: 4 }, part).join("."),
            () => Array.from({ length: 1 + rand.below(8) }, part).join(":") + rand.pick(["", "::", ":"]),
        ][kind]();
        return text + rand.pick(["", "", "", "%eth0", "%", "%a-b", " "]);
    };
    for (let i = 0; i < 300; i++) {
        const ip = seededAddress();
        await push(`seeded address ${JSON.stringify(ip)}`, "peer.isValid", { ip, allowPrivate: rand.below(2) === 0, interfaces: ["2001:db8::1", "10.0.0.1"] });
    }
    const own = ["192.0.2.10", "2001:db8::10", "::ffff:198.51.100.10", "fe80::1", "::1.2.3.4"];
    for (const ip of ["192.0.2.10", "::ffff:192.0.2.10", "2001:db8::10", "2001:db8:0:0:0:0:0:10", "198.51.100.10", "::ffff:198.51.100.10", "fe80::1", "::102:304", "::1.2.3.4"]) {
        await push(`the host's address ${ip}`, "peer.isValid", { ip, allowPrivate: true, interfaces: own });
    }

    // ---- peer.isValidVersion -----------------------------------------------------------------------------
    const versions = ["4.3.1", HEARTWOOD, "4.3.0", "4.2.9", "4.3.0-next.1", "4.3.1-next.0", "3.2.0", "3.2.0-next.0", "3.9.9", "4.0.0", "v4.3.1", " 4.3.1 ", "4.3", "04.3.1", "", "4.3.1+build.5"];
    const floors = [undefined, [">=4.3.0"], [">=4.2.2-next.0"], [">=4.3.0", "^3.2"], [], [5, ">=4.3.0"], [">=5.0.0"]];
    for (const networkName of ["devnet", "mainnet"]) {
        for (const minimumVersions of floors) {
            for (const version of versions) {
                const input = { version, configured: DEFAULT_RANGES, networkName };
                if (minimumVersions !== undefined) input.minimumVersions = minimumVersions;
                await push(`version ${JSON.stringify(version)} under ${JSON.stringify(minimumVersions)} on ${networkName}`, "peer.isValidVersion", input);
            }
        }
    }
    // The range forms of the supported subset, over a seeded set of versions.
    const pieces = ["0", "1", "2", "3", "4", "10", "x", "X", "*"];
    const partialVersion = () => {
        const n = 1 + rand.below(3);
        const parts = Array.from({ length: n }, () => rand.pick(pieces));
        let v = parts.join(".");
        if (n === 3 && rand.below(4) === 0) v += rand.pick(["-0", "-alpha", "-alpha.1", "-1", "-rc.2", "-x"]);
        if (n === 3 && rand.below(8) === 0) v += "+build";
        return rand.pick(["", "", "", "v", "=", "v="]) + v;
    };
    const comparator = () => {
        const op = rand.pick(["", "", "=", "<", "<=", ">", ">=", "^", "~", "~>"]);
        const space = rand.below(5) === 0 && op !== "" ? " " : "";
        return `${op}${space}${partialVersion()}`;
    };
    const range = () => {
        const sets = Array.from({ length: 1 + rand.below(3) }, () => Array.from({ length: 1 + rand.below(2) }, comparator).join(" "));
        return rand.below(12) === 0 ? rand.pick(["*", "", ">=0.0.0", "<0.0.0-0", "<*", "x || 1.x"]) : sets.join(rand.pick([" || ", "||"]));
    };
    const sample = () => {
        const v = [rand.pick(["0", "1", "2", "3", "4", "10"]), rand.pick(["0", "1", "2", "3", "10"]), rand.pick(["0", "1", "2", "3", "4"])].join(".");
        return rand.below(3) === 0 ? `${v}-${rand.pick(["0", "1", "alpha", "alpha.1", "rc.2", "x"])}` : v;
    };
    for (let i = 0; i < 400; i++) {
        const minimumVersions = [range()];
        await push(`range ${JSON.stringify(minimumVersions[0])}`, "peer.isValidVersion", {
            version: sample(),
            minimumVersions,
            configured: DEFAULT_RANGES,
            networkName: rand.pick(["devnet", "mainnet"]),
        });
    }

    // ---- peer.sameSubnet ------------------------------------------------------------------------------------
    const known = ["192.0.2.1", "192.0.2.200", "192.0.3.1", "198.51.100.1", "2001:db8::1", "::ffff:192.0.2.5"];
    for (const ip of ["192.0.2.7", "192.0.3.7", "203.0.113.1", "2001:db8::2", "::ffff:192.0.2.9"]) {
        await push(`subnet of ${ip}`, "peer.sameSubnet", { ip, peers: known });
    }

    // ---- ratelimit.run ----------------------------------------------------------------------------------------
    const limiter = { rateLimit: 100, rateLimitPostTransactions: 25 };
    const A = "192.0.2.1";
    const B = "192.0.2.2";
    const run = (name, events, limits = limiter) => push(`limiter: ${name}`, "ratelimit.run", { limiter: limits, events });
    const burst = (route, times, op = "throttle", ip = A) => times.map((t) => ({ t, ip, route, op }));
    // Each rule: its points within a window, the refusal, and the window's end.
    for (const [route, points, ms] of [
        ["postBlock", 2, 4000],
        ["getBlocks", 1, 2000],
        ["getPeers", 1, 1000],
        ["getStatus", 9, 1000],
        ["getCommonBlocks", 9, 1000],
        ["getUnconfirmedTransactions", 1, 2000],
        ["postTransactions", 25, 1000],
    ]) {
        const times = [...Array.from({ length: points + 1 }, (_, i) => i), ms - 1, ms, ms + 1];
        await run(`${route} across its window`, burst(route, times));
        await run(`${route} consumed past its limit`, [...burst(route, times.slice(0, points + 2), "consume"), ...burst(route, [ms - 1, ms], "wouldThrottle")]);
    }
    await run(
        "the global limit over routes and peers",
        [
            ...burst("getStatus", [0, 0, 0], "throttle"),
            ...burst("getPeers", [1], "throttle"),
            ...burst("getCommonBlocks", [2], "throttle"),
            ...burst("getStatus", [3], "throttle", B),
            ...burst("getStatus", [999, 1000, 1001], "wouldThrottle"),
        ],
        { rateLimit: 3, rateLimitPostTransactions: 25 },
    );
    // A seeded set of sequences over every route, two peers and small limits.
    const routes = ["postBlock", "getBlocks", "getPeers", "getStatus", "getCommonBlocks", "getUnconfirmedTransactions", "postTransactions"];
    for (let i = 0; i < 100; i++) {
        let t = 0;
        const events = Array.from({ length: 40 }, () => {
            t += rand.pick([0, 0, 1, 10, 100, 499, 500, 999, 1000, 1001, 1999, 2000, 4000]);
            return { t, ip: rand.pick([A, B]), route: rand.pick(routes), op: rand.pick(["consume", "wouldThrottle", "throttle", "throttle"]) };
        });
        await run(`seeded sequence ${i}`, events, { rateLimit: rand.pick([100, 3, 5, 1, 0]), rateLimitPostTransactions: rand.pick([25, 0, 2, 1]) });
    }
    await run("postTransactions with the option at 0", burst("postTransactions", Array.from({ length: 27 }, (_, i) => i)), { rateLimit: 100, rateLimitPostTransactions: 0 });
    await run("postTransactions with the option at 2", burst("postTransactions", [0, 1, 2, 999, 1000]), { rateLimit: 100, rateLimitPostTransactions: 2 });
    await run("a global refusal leaves the route alone", [...burst("getBlocks", [0, 1], "consume"), ...burst("getBlocks", [1000, 1999, 2000], "wouldThrottle")], {
        rateLimit: 1,
        rateLimitPostTransactions: 25,
    });
    return out;
}

module.exports = { exchange, rules };
