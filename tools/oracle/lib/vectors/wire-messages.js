"use strict";
// V13, protobuf messages: each of the 24 messages with all fields present, all absent and present
// with default values, both ways (proto.encode, then proto.decode of its bytes); varints of 5 and
// 10 bytes; the negative slot; repeated, unknown and mistyped fields; groups, once nested 50,000
// deep; overruns at the top level and nested; invalid UTF-8; map entries without key or value,
// repeated names, array-index names and __proto__; the key order of a decoded object. Then the
// codecs that pass a message through as it is (getPeers, getCommonBlocks and the getStatus
// request), with the exact requests the reference's clients send, and wire.decodeSelected for
// every selector. Values are synthetic: documentation addresses, derived ids and keys.

const VERSION = "4.3.1";
const HEADERS = { version: VERSION, port: 6002 };
const FORGER_HEADERS = { version: VERSION, port: 0 };

/** `length` bytes of hex derived from `seed`. */
function fill(length, seed) {
    const b = Buffer.alloc(length);
    for (let i = 0; i < length; i++) b[i] = (i * 29 + seed * 7 + 3) & 0xff;
    return b.toString("hex");
}

const ID = [fill(32, 1), fill(32, 2), fill(32, 3)];
const PUBLIC_KEY = `02${fill(32, 4)}`;
const SIGNATURE = fill(64, 5);

const STATUS_HEADER = {
    id: ID[0],
    version: 0,
    timestamp: 1234567,
    previousBlock: ID[1],
    height: 42,
    numberOfTransactions: 2,
    totalAmount: "300000000",
    totalFee: "20000000",
    reward: "900000000",
    payloadLength: 300,
    payloadHash: fill(32, 6),
    generatorPublicKey: PUBLIC_KEY,
    blockSignature: SIGNATURE,
};
const PLUGIN = { port: 6003, enabled: true, estimateTotalCount: true };
const NETWORK = { name: "devnet", nethash: fill(32, 7), explorer: "https://explorer.example", token: { name: "DEV", symbol: "D" }, version: 30 };
const CONFIG = { version: VERSION, network: NETWORK, plugins: { "@solar-network/api": PLUGIN, "1234": { port: 1, enabled: false } } };
const STATE = { height: 42, forgingAllowed: true, currentSlot: 4000, header: STATUS_HEADER };

// Each message: all fields present, and present with default values ({} is all absent).
const MESSAGES = {
    "shared.Headers": [HEADERS, { version: "", port: 0 }],
    "peer.GetPeersRequest": [{ headers: HEADERS }, { headers: {} }],
    "peer.GetPeersResponse": [{ peers: [{ ip: "192.0.2.10", port: 6002 }, { ip: "198.51.100.7", port: 4002 }] }, { peers: [{}] }],
    "peer.GetPeersResponse.Peer": [{ ip: "203.0.113.5", port: 6002 }, { ip: "", port: 0 }],
    "peer.GetCommonBlocksRequest": [{ ids: ID, headers: HEADERS }, { ids: [""], headers: {} }],
    "peer.GetCommonBlocksResponse": [{ common: { height: 12, id: ID[2] } }, { common: {} }],
    "peer.GetCommonBlocksResponse.Common": [{ height: 12, id: ID[2] }, { height: 0, id: "" }],
    "peer.GetStatusRequest": [{ headers: HEADERS }, { headers: {} }],
    "peer.GetStatusResponse": [
        { state: STATE, config: CONFIG, publicKeys: [PUBLIC_KEY, PUBLIC_KEY], signatures: [SIGNATURE, SIGNATURE] },
        { state: {}, config: {}, publicKeys: [""], signatures: [""] },
    ],
    "peer.GetStatusResponse.State": [STATE, { height: 0, forgingAllowed: false, currentSlot: 0, header: {} }],
    "peer.GetStatusResponse.State.BlockHeader": [
        STATUS_HEADER,
        Object.fromEntries(Object.entries(STATUS_HEADER).map(([k, v]) => [k, typeof v === "number" ? 0 : ""])),
    ],
    "peer.GetStatusResponse.Config": [CONFIG, { version: "", network: {}, plugins: { "": {} } }],
    "peer.GetStatusResponse.Config.Network": [NETWORK, { name: "", nethash: "", explorer: "", token: {}, version: 0 }],
    "peer.GetStatusResponse.Config.Network.Token": [{ name: "DEV", symbol: "D" }, { name: "", symbol: "" }],
    "peer.GetStatusResponse.Config.Plugin": [PLUGIN, { port: 0, enabled: false, estimateTotalCount: false }],
    "blocks.GetBlocksRequest": [
        { lastBlockHeight: 10, blockLimit: 400, headersOnly: false, serialised: true, headers: HEADERS },
        { lastBlockHeight: 0, blockLimit: 0, headersOnly: false, serialised: false, headers: {} },
    ],
    "blocks.GetBlocksResponse": [{ blocks: fill(40, 8) }, { blocks: "" }],
    "blocks.GetBlocksResponse.BlockHeader": [
        { ...STATUS_HEADER, transactions: `0000000a${fill(10, 9)}` },
        { ...Object.fromEntries(Object.entries(STATUS_HEADER).map(([k, v]) => [k, typeof v === "number" ? 0 : ""])), transactions: "" },
    ],
    "blocks.PostBlockRequest": [{ block: fill(180, 10), headers: HEADERS }, { block: "", headers: {} }],
    "blocks.PostBlockResponse": [{ status: true, height: 100 }, { status: false, height: 0 }],
    "transactions.GetUnconfirmedTransactionsRequest": [
        { countOnly: true, exclude: ID[0] + ID[1], headers: FORGER_HEADERS },
        { countOnly: false, exclude: "", headers: {} },
    ],
    "transactions.GetUnconfirmedTransactionsResponse": [
        { poolSize: 3, transactions: `00000004${fill(4, 11)}`, headers: HEADERS },
        { poolSize: 0, transactions: "", headers: {} },
    ],
    "transactions.PostTransactionsRequest": [{ transactions: `00000004${fill(4, 12)}`, headers: HEADERS }, { transactions: "", headers: {} }],
    "transactions.PostTransactionsResponse": [{ accept: [ID[0], ID[1]] }, { accept: [""] }],
};

/** The hex of text. */
const text = (s) => Buffer.from(s, "utf8").toString("hex");
/** A varint. */
function varint(n) {
    let out = "";
    let v = n;
    while (v >= 0x80) {
        out += ((v & 0x7f) | 0x80).toString(16).padStart(2, "0");
        v = Math.floor(v / 128);
    }
    return out + v.toString(16).padStart(2, "0");
}
/** A length-delimited field: its tag byte (hex) and the content (hex). */
const field = (tag, content) => `${tag}${varint(content.length / 2)}${content}`;
/** A map entry of Config.plugins (field 3), with the key and value that are not null. */
function entry(key, value) {
    return field("1a", `${key === null ? "" : field("0a", text(key))}${value === null ? "" : field("12", value)}`);
}

module.exports = async function wireMessagesVectors({ rec, must, shared }) {
    const H = shared.height;
    const out = [];
    const push = async (op, input, name) => {
        const record = { ...(await rec(op, H, input)), name };
        out.push(record);
        return record;
    };
    const encode = async (message, value, name) => must(await push("proto.encode", { message, value }, name)).output.hex;
    const decode = (message, hex, name) => push("proto.decode", { message, hex: hex.replace(/ /g, "") }, name);

    // ---- every message, both ways ----------------------------------------------------------------
    for (const [message, [full, zeros]] of Object.entries(MESSAGES)) {
        for (const [value, what] of [[full, "all present"], [zeros, "defaults present"], [{}, "all absent"]]) {
            const hex = await encode(message, value, `${message}, ${what}`);
            await decode(message, hex, `${message}, ${what}`);
        }
    }

    // ---- varints ---------------------------------------------------------------------------------
    await encode("peer.GetStatusResponse.State", { currentSlot: -3 }, "negative slot");
    await encode("peer.GetStatusResponse", { state: { height: 1, forgingAllowed: false, currentSlot: -3, header: STATUS_HEADER } }, "negative slot in a status");
    await encode("shared.Headers", { port: 4294967295 }, "largest uint32");
    for (const [hex, what] of [
        ["10 ff ff ff ff 0f", "5-byte varint"],
        ["10 80 80 80 80 7f", "5-byte varint, high bits of the fifth byte dropped"],
        ["10 ff ff ff ff 8f 01 02 03 04 05", "10-byte varint, five bytes skipped whatever they hold"],
        ["10 ff ff ff ff ff ff ff ff ff 01", "10-byte varint of a 64-bit value"],
        ["10 ff ff ff ff 8f 01 02 03 04", "10-byte varint cut short"],
        ["10 80 80", "varint cut short"],
        ["90 80 80 80 00 05", "5-byte tag"],
        ["90 80 80 80 80 aa bb cc dd ee 05", "10-byte tag"],
    ]) {
        await decode("shared.Headers", hex, what);
    }
    await decode("peer.GetStatusResponse.State", "18 fd ff ff ff 0f", "negative slot as written");
    await decode("peer.GetStatusResponse.State", "10 80 80 80 80 10", "bool of 2^32 is false");
    await decode("peer.GetStatusResponse.State", "10 02", "bool of 2 is true");
    await decode("blocks.GetBlocksRequest", "18 ff ff ff ff 8f 00 00 00 00 00 20 80 01", "bools of 10-byte and 2-byte varints");

    // ---- repeated, unknown and mistyped fields ---------------------------------------------------
    await decode("shared.Headers", "10 05 10 07", "a scalar repeated: the last wins");
    await decode("shared.Headers", "10 05 0a 01 61 10 07", "own keys in the order of first appearance");
    await decode("peer.GetPeersRequest", "0a 02 10 05 0a 03 0a 01 61", "a message repeated: replaced, not merged");
    await decode("peer.GetCommonBlocksRequest", "0a 01 61 12 00 0a 01 62 0a 00", "repeated strings append");
    await decode("peer.GetStatusResponse", "12 00 0a 00 1a 01 61 22 00 1a 00", "config before state, repeated fields first");
    await decode("peer.GetStatusResponse.State.BlockHeader", `7a 01 61 0a 01 62 38 07 4a 01 35 18 01`, "block header fields out of order");
    await decode("peer.GetStatusResponse.Config", `${entry("x", "0801")} 0a 01 76`, "plugins before version");
    for (const [hex, what] of [
        ["00 00 10 05", "field 0 skipped"],
        ["18 ff ff ff ff ff ff ff ff ff ff ff 01 10 05", "unknown varint of 12 bytes"],
        ["19 01 02 03 04 05 06 07 08 10 05", "unknown fixed64"],
        ["1a 02 01 02 10 05", "unknown length-delimited"],
        ["1d 01 02 03 04 10 05", "unknown fixed32"],
        ["1b 1b 0c 0c 10 05", "unknown group, nested"],
        ["1b 08 01 11 01 02 03 04 05 06 07 08 1a 01 00 1d 01 02 03 04 0c 10 05", "unknown group holding every wire type"],
        ["1b 23 2b 2c 24 0c 10 05", "group ended by any end-group tag"],
        ["1b 1b 0c 10 05", "unknown group never ended"],
        ["1b 1e 0c", "wire type 6 in a group"],
        ["1c", "wire type 4 outside a group"],
        ["1e", "wire type 6"],
        ["1f", "wire type 7"],
        ["18 ff", "unknown varint cut short"],
        ["19 01 02 03 04 05 06 07", "unknown fixed64 cut short"],
        ["1a 03 01 02", "unknown length-delimited cut short"],
        ["1d 01 02 03", "unknown fixed32 cut short"],
    ]) {
        await decode("shared.Headers", hex, what);
    }
    await decode("peer.GetStatusResponse.Config.Plugin", "20 01 2d 01 02 03 04 08 05", "unknown fields in a plugin");
    await decode("peer.GetStatusResponse", "2b 0c 0a 00", "unknown group beside a state");
    for (const [message, hex, what] of [
        ["shared.Headers", "08 01 61", "string field sent as a varint"],
        ["shared.Headers", "0d 01 61", "string field sent as fixed32"],
        ["shared.Headers", "12 05", "uint32 field sent as length-delimited"],
        ["shared.Headers", "15 05", "uint32 field sent as fixed32"],
        ["shared.Headers", "14 05", "uint32 field sent as an end group"],
        ["peer.GetStatusResponse.Config.Plugin", "12 01", "bool field sent as length-delimited"],
        ["peer.GetPeersRequest", "08 02 10 05", "message field sent as a varint"],
        ["peer.GetPeersRequest", "0b 02 10 05", "message field sent as a start group"],
        ["peer.GetCommonBlocksRequest", "08 01 61", "repeated string sent as a varint"],
        ["blocks.PostBlockRequest", "08 01 61", "bytes field sent as a varint"],
        ["peer.GetStatusResponse.Config", "18 02 0a 00", "map entry sent as a varint"],
    ]) {
        await decode(message, hex, what);
    }

    // ---- overruns and text -----------------------------------------------------------------------
    for (const [message, hex, what] of [
        ["shared.Headers", "0a 05 61", "string cut at the end"],
        ["shared.Headers", "0a ff ff ff ff 0f 61", "string of length 2^32 - 1 cut at the end"],
        ["peer.GetPeersRequest", "0a 03 0a 05 61", "nested string cut at the end"],
        ["peer.GetPeersRequest", "0a 05 0a 05 61", "nested string cut, then a tag is missing"],
        ["peer.GetPeersRequest", "0a 04 0a 05 61 62 10 05", "nested string runs into the enclosing message"],
        ["peer.GetPeersRequest", "0a 05 10 05", "nested message longer than the payload"],
        ["peer.GetStatusResponse", "0a 04 22 02 0a 05 61", "string cut two levels down"],
        ["blocks.PostBlockRequest", "0a 05 61", "bytes past the end"],
        ["blocks.PostBlockRequest", "0a 01 61", "bytes of one byte"],
        ["transactions.GetUnconfirmedTransactionsRequest", "08 01 12 05 01 02", "bytes past the end after another field"],
        ["shared.Headers", "0a 02 ff fe", "invalid UTF-8"],
        ["shared.Headers", "0a 03 e2 82 61", "UTF-8 sequence cut short"],
        ["shared.Headers", "0a 03 ed a0 80", "UTF-8 of a surrogate"],
        ["shared.Headers", "0a 04 f4 90 80 80", "UTF-8 beyond U+10FFFF"],
        ["shared.Headers", "0a 02 c0 af", "overlong UTF-8"],
        ["shared.Headers", "0a 04 f0 9f 98 80", "UTF-8 of an emoji"],
        ["shared.Headers", "0a 03 f0 9f 98", "UTF-8 of an emoji cut at the end"],
    ]) {
        await decode(message, hex, what);
    }

    // ---- the plugins map -------------------------------------------------------------------------
    const config = "peer.GetStatusResponse.Config";
    for (const [hex, what] of [
        ["1a 00", "entry without key or value"],
        [entry(null, "0809"), "entry without a key"],
        [entry("ab", null), "entry without a value"],
        [entry("b-plugin", "0801") + entry("b-plugin", "0802"), "repeated name: first position, last value"],
        [entry("b-plugin", "0801") + entry("1234", "0802") + entry("7", "") + entry("4294967295", "") + entry("01", ""), "array-index names first"],
        [entry("b-plugin", "0801") + entry("b-plugin", null), "repeated name, the second without a value"],
        ["1a 08 12 02 08 01 0a 02 61 62", "value before key"],
        ["1a 0a 0a 01 61 0a 01 62 12 02 08 01", "key repeated in an entry"],
        ["1a 07 18 01 0a 01 61 12 00", "unknown field in an entry"],
        ["1a 04 0a 05 61 62", "key cut at the end"],
        ["1a 05 12 05 08 01", "value longer than the entry and the payload"],
        ["1a 02 1e 00", "wire type 6 in an entry"],
        [entry("__proto__", "0803") + entry("x", "0804"), "__proto__ with a value replaces the prototype"],
        [entry("__proto__", null) + entry("__proto__", "0801") + entry("x", "0802"), "__proto__ without a value, then as an own key"],
        [entry("__proto__", "0801") + entry("__proto__", null) + entry("__proto__", "0802"), "__proto__ three times"],
        [entry("toString", "0801") + entry("constructor", "0802") + entry("hasOwnProperty", null), "names of Object.prototype"],
    ]) {
        await decode(config, hex, what);
    }
    await encode(config, { plugins: { "b-plugin": { port: 1 }, "1234": { enabled: false }, "7": {} } }, "array-index names written first");

    // ---- the codecs that pass a message through --------------------------------------------------
    const codec = async (route, direction, value, name) => {
        const hex = must(await push("wire.encode", { route, direction, value }, name)).output.hex;
        await push("wire.decode", { route, direction, hex }, name);
    };
    const codecDecode = (route, direction, hex, name) => push("wire.decode", { route, direction, hex: hex.replace(/ /g, "") }, name);
    await codec("getPeers", "request", { headers: HEADERS }, "a relay's request");
    await codec("getPeers", "request", {}, "empty");
    await codec("getPeers", "response", MESSAGES["peer.GetPeersResponse"][0].peers, "two peers");
    await codec("getPeers", "response", [], "no peer");
    await codec("getCommonBlocks", "request", { ids: ID, headers: HEADERS }, "a relay's request");
    await codec("getCommonBlocks", "response", { common: { height: 12, id: ID[2] } }, "a common block");
    await codec("getCommonBlocks", "response", {}, "no common block");
    await codec("getStatus", "request", { headers: HEADERS }, "a relay's request");
    await codec("getStatus", "request", { headers: FORGER_HEADERS }, "a forger's request");
    await codecDecode("getPeers", "request", "0a 02 10 05 10 07", "unknown field after the headers");
    await codecDecode("getPeers", "response", "0a 00 0a 05 0a 01 61 10 07 12 00", "peers with an unknown field");
    await codecDecode("getPeers", "response", "0a 04 0a 05 61", "peer cut short");
    await codecDecode("getCommonBlocks", "request", "12 00 0a 00 0a 01 61", "headers before ids");
    await codecDecode("getCommonBlocks", "response", "0a 02 08 07 0a 02 08 03", "common repeated: the last wins");
    await codecDecode("getStatus", "request", "17", "wire type 7");
    // Groups nested beyond the Node stack: the reference overflows, Heartwood skips them (a
    // documented difference).
    const depth = 50000;
    await codecDecode("getStatus", "request", `${"1b".repeat(depth)}${"0c".repeat(depth)}0a 02 10 05`, "groups nested 50,000 deep");

    // ---- the fuzz selector -----------------------------------------------------------------------
    for (let selector = 0; selector < 14; selector++) {
        await push("wire.decodeSelected", { hex: selector.toString(16).padStart(2, "0") }, `selector ${selector}, empty payload`);
    }
    for (const [hex, what] of [
        ["", "empty input"],
        ["0e 0a 00", "selector 14 is selector 0"],
        ["ff 0a 02 08 07", "selector 255 is selector 3"],
        ["02 0a 01 61 0a 01 62", "selector 2"],
        ["04 0a 07 0a 05 34 2e 33 2e 31", "selector 4"],
    ]) {
        await push("wire.decodeSelected", { hex: hex.replace(/ /g, "") }, what);
    }
    return out;
};
