"use strict";
// V13, lists and the block and transaction codecs: every direction both ways, the requests the
// reference's clients build, list edges and caps, exclude lengths, and amounts, one beyond 128
// bits. Returns its records.
//
// Encode records take `wire.encode` values in the shape `wire.decode` gives back: present keys
// only, bytes and transactions as hex, amounts as decimal text. Decode records take payloads
// crafted here with a small protobuf writer, so that field order, repeated fields and malformed
// lists are exact.

// The caps of the reference's list decoders.
const MAX_BLOCKS = 400;
const MAX_BLOCK_TRANSACTIONS = 500;
const MAX_TRANSACTIONS = 1000;
const MAX_EXCLUSIONS = 15000;

// Protobuf writing, for crafted payloads.
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
const hex = (...parts) => Buffer.concat(parts).toString("hex");

// A length-prefixed list of the reference's codecs: a u32 big-endian length before each item.
function u32be(n) {
    const b = Buffer.alloc(4);
    b.writeUInt32BE(n);
    return b;
}
const list = (items) => Buffer.concat(items.flatMap((item) => [u32be(item.length), item]));
/** n items of one byte each: the item's index modulo 256. */
const oneByteItems = (n) => Array.from({ length: n }, (_, i) => Buffer.from([i & 0xff]));
/** Exclude id i: 28 zero bytes and i as a u32 big-endian. */
const excludeId = (i) => Buffer.concat([Buffer.alloc(28), u32be(i)]);

// BlockHeader fields (blocks.proto, GetBlocksResponse.BlockHeader).
const HEADER = { id: 1, version: 3, timestamp: 4, previousBlock: 5, height: 7, numberOfTransactions: 8, totalAmount: 9, totalFee: 10, reward: 11 };
const HEADER_TRANSACTIONS = 16;

module.exports = async function wireListsVectors({ rec, must, shared, solar }) {
    const out = [];
    const H = shared.height;
    const version = solar.version;
    const peerHeaders = { port: 4002, version };
    const forgerHeaders = { port: 0, version };
    const headersBytes = (h) => Buffer.concat([text(1, h.version), uint(2, h.port)]);

    const encode = async (name, route, direction, value) => {
        const r = must(await rec("wire.encode", H, { route, direction, value }));
        out.push({ ...r, name: `${route} ${direction}: ${name}` });
        return r.output.hex;
    };
    const decode = async (name, route, direction, payload) => {
        const r = await rec("wire.decode", H, { route, direction, hex: payload });
        out.push({ ...r, name: `${route} ${direction}: ${name}` });
        return r;
    };
    /** An encode record, and a decode record of its payload. */
    const both = async (name, route, direction, value) => decode(name, route, direction, await encode(name, route, direction, value));

    // Real blocks and transactions of the earlier classes.
    const txs = ["transfer-1", "vote-2", "burn", "transfer-2"].map((n) => shared.tx[n].hex);
    const [genesisChild, fullBlock] = shared.blocks.statusBlocks.map((b) => b.output);
    const headerOf = (json) => {
        const keys = ["id", "version", "timestamp", "previousBlock", "height", "numberOfTransactions", "totalAmount", "totalFee", "reward", "payloadLength", "payloadHash", "generatorPublicKey", "blockSignature"];
        return Object.fromEntries(keys.filter((k) => json[k] !== undefined).map((k) => [k, json[k]]));
    };

    // The requests the reference's clients build: the relay's peer communicator adds its port and
    // version, the forger sends port 0; the server decodes each.
    const ids = [genesisChild.id, fullBlock.id, "ab".repeat(32)];
    await both("the client's status request", "getStatus", "request", { headers: peerHeaders });
    await both("the forger's status request", "getStatus", "request", { headers: forgerHeaders });
    await both("the client's request", "getPeers", "request", { headers: peerHeaders });
    await both("the client's request", "getCommonBlocks", "request", { ids, headers: peerHeaders });
    await both("a download", "getBlocks", "request", { lastBlockHeight: 1, blockLimit: MAX_BLOCKS, serialised: true, headers: peerHeaders });
    await both("one block", "getBlocks", "request", { lastBlockHeight: 0, blockLimit: 1, serialised: true, headers: peerHeaders });
    await both("the verifier's headers", "getBlocks", "request", { lastBlockHeight: 1, blockLimit: 2, headersOnly: true, serialised: true, headers: peerHeaders });
    await both("the client's request", "getUnconfirmedTransactions", "request", { exclude: ids.slice(0, 2), headers: peerHeaders });
    await both("no exclusions", "getUnconfirmedTransactions", "request", { exclude: [], headers: peerHeaders });
    await both("the forger's request", "getUnconfirmedTransactions", "request", { headers: forgerHeaders });
    await both("the client's block", "postBlock", "request", { block: fullBlock.hex, headers: peerHeaders });
    await both("the forger's block", "postBlock", "request", { block: genesisChild.hex, headers: forgerHeaders });
    await both("the client's transactions", "postTransactions", "request", { transactions: txs, headers: peerHeaders });

    // The servers' responses, read by the clients.
    const served = [
        { ...headerOf(genesisChild.json), transactions: [] },
        { ...headerOf(fullBlock.json), transactions: [shared.tx["transfer-1"].hex] },
    ];
    await both("two blocks", "getBlocks", "response", served);
    await both("headers only", "getBlocks", "response", served.map(({ transactions, ...header }) => header));
    await both("none", "getBlocks", "response", []);
    await both("amounts of 64 bits", "getBlocks", "response", [{ height: 3, totalAmount: "18446744073709551615", totalFee: "9223372036854775807", reward: "0" }]);
    await both("accepted", "postBlock", "response", { status: true, height: 12 });
    await both("refused", "postBlock", "response", { status: false, height: 0 });
    await both("the pool", "getUnconfirmedTransactions", "response", { poolSize: 7, transactions: txs });
    await both("count only", "getUnconfirmedTransactions", "response", { poolSize: 0, transactions: [] });
    await both("accepted ids", "postTransactions", "response", [shared.tx["transfer-1"].id, shared.tx["burn"].id]);
    await both("none accepted", "postTransactions", "response", []);

    // Lists: sizes, the caps (one item more than the cap is read), a final and a middle empty
    // item, trailing bytes and an item that overruns the buffer. Each list is read by every
    // codec that has one.
    const minimalBlock = (i) => uint(HEADER.height, i + 1);
    const blocksWith = (n) => hex(list(Array.from({ length: n }, (_, i) => minimalBlock(i))));
    for (const n of [0, 1, 2, MAX_BLOCKS, MAX_BLOCKS + 1, MAX_BLOCKS + 2]) {
        await decode(`${n} blocks`, "getBlocks", "response", blocksWith(n));
    }
    const blockWithTransactions = (transactions) => hex(list([Buffer.concat([uint(HEADER.height, 2), bytes(HEADER_TRANSACTIONS, transactions)])]));
    for (const n of [0, 1, MAX_BLOCK_TRANSACTIONS, MAX_BLOCK_TRANSACTIONS + 1, MAX_BLOCK_TRANSACTIONS + 2]) {
        await decode(`a block of ${n} transactions`, "getBlocks", "response", blockWithTransactions(list(oneByteItems(n))));
    }
    const listed = [
        ["postTransactions", "request", (l) => hex(bytes(1, l), bytes(2, headersBytes(peerHeaders)))],
        ["getUnconfirmedTransactions", "response", (l) => hex(uint(1, 3), bytes(2, l))],
    ];
    for (const [route, direction, payload] of listed) {
        for (const n of [0, 1, MAX_TRANSACTIONS, MAX_TRANSACTIONS + 1, MAX_TRANSACTIONS + 2]) {
            await decode(`${n} transactions`, route, direction, payload(list(oneByteItems(n))));
        }
    }
    const edges = [
        ["1 byte", Buffer.from("00", "hex")],
        ["3 bytes", Buffer.from("000001", "hex")],
        ["4 bytes", Buffer.from("00000001", "hex")],
        ["a length and 1 byte", Buffer.from("0000000199", "hex")],
        ["a final empty item", Buffer.concat([list(oneByteItems(2)), u32be(0)])],
        ["two final empty items", Buffer.concat([list(oneByteItems(1)), u32be(0), u32be(0)])],
        ["an empty item and 1 byte", Buffer.from("0000000077", "hex")],
        ["a middle empty item", list([Buffer.from("aa", "hex"), Buffer.alloc(0), Buffer.from("bb", "hex")])],
        ["1 trailing byte", Buffer.concat([list(oneByteItems(2)), Buffer.from("ff", "hex")])],
        ["2 trailing bytes", Buffer.concat([list(oneByteItems(2)), Buffer.from("ffff", "hex")])],
        ["3 trailing bytes", Buffer.concat([list(oneByteItems(2)), Buffer.from("ffffff", "hex")])],
        ["4 trailing bytes", Buffer.concat([list(oneByteItems(2)), Buffer.from("00000009", "hex")])],
        ["5 trailing bytes", Buffer.concat([list(oneByteItems(2)), Buffer.from("00000009ee", "hex")])],
        ["an item that overruns", Buffer.concat([list(oneByteItems(1)), Buffer.from("00000100abcd", "hex")])],
        ["a length of 2^32 - 1", Buffer.from("ffffffff0102", "hex")],
    ];
    for (const [name, l] of edges) {
        await decode(`transactions: ${name}`, "getBlocks", "response", blockWithTransactions(l));
        for (const [route, direction, payload] of listed) await decode(`transactions: ${name}`, route, direction, payload(l));
    }
    const blockEdges = [
        ["1 byte", Buffer.from("00", "hex")],
        ["4 bytes", Buffer.from("00000000", "hex")],
        ["a final empty block", Buffer.concat([list([minimalBlock(0)]), u32be(0)])],
        ["a middle empty block", list([minimalBlock(0), Buffer.alloc(0), minimalBlock(2)])],
        ["3 trailing bytes", Buffer.concat([list([minimalBlock(0)]), Buffer.from("000000", "hex")])],
        ["a block cut short", Buffer.concat([list([minimalBlock(0)]), u32be(9), text(HEADER.id, "abc")])],
        ["a block whose string is cut", Buffer.concat([u32be(3), Buffer.from("0a0561", "hex")])],
        ["a block whose varint is cut", Buffer.concat([u32be(2), Buffer.from("38ff", "hex")])],
        ["a block with wire type 7", Buffer.concat([list([minimalBlock(0)]), list([Buffer.from("17", "hex")])])],
        ["a block whose height has wire type 7", Buffer.concat([list([minimalBlock(0)]), list([Buffer.from("3f", "hex")])])],
        ["an amount cut at the end of a block", Buffer.concat([list([minimalBlock(0)]), list([Buffer.from("4a02", "hex")])])],
    ];
    for (const [name, payload] of blockEdges) await decode(name, "getBlocks", "response", hex(payload));

    // The exclude list: whole 32-byte ids or none, at most one more than its cap.
    const excludeRequest = (excludeBytes) => hex(bytes(2, excludeBytes), bytes(3, headersBytes(peerHeaders)));
    for (const n of [0, 1, 2, MAX_EXCLUSIONS, MAX_EXCLUSIONS + 1, MAX_EXCLUSIONS + 2]) {
        await decode(`${n} excluded ids`, "getUnconfirmedTransactions", "request", excludeRequest(Buffer.concat(Array.from({ length: n }, (_, i) => excludeId(i)))));
    }
    for (const length of [1, 31, 33, 63, 65, 80]) {
        await decode(`an exclude list of ${length} bytes`, "getUnconfirmedTransactions", "request", excludeRequest(Buffer.alloc(length, 0x11)));
    }

    // The keys the codecs set, where the wire has them first, last or not at all, and a field
    // sent twice (the last one wins).
    const headers = headersBytes(peerHeaders);
    await decode("no exclude list, count only", "getUnconfirmedTransactions", "request", hex(uint(1, 1)));
    await decode("headers before the exclude list", "getUnconfirmedTransactions", "request", hex(bytes(3, headers), bytes(2, excludeId(5)), uint(1, 0)));
    await decode("no block", "postBlock", "request", hex(bytes(2, headers)));
    await decode("headers before the block", "postBlock", "request", hex(bytes(2, headers), bytes(1, Buffer.from("0102", "hex"))));
    await decode("the block twice", "postBlock", "request", hex(bytes(1, Buffer.from("01", "hex")), bytes(2, headers), bytes(1, Buffer.from("0203", "hex"))));
    await decode("empty", "postBlock", "request", "");
    await decode("headers before the transactions", "postTransactions", "request", hex(bytes(2, headers), bytes(1, list(oneByteItems(2)))));
    await decode("no transactions", "postTransactions", "request", hex(bytes(2, headers)));
    await decode("empty", "postTransactions", "request", "");
    await decode("headers first, no transactions", "getUnconfirmedTransactions", "response", hex(bytes(3, headers), uint(1, 9)));
    await decode("empty", "getUnconfirmedTransactions", "response", "");
    const headerFirst = Buffer.concat([text(HEADER.reward, "5"), bytes(HEADER_TRANSACTIONS, list(oneByteItems(1))), text(HEADER.id, "ab"), uint(HEADER.height, 4), text(HEADER.totalAmount, "1")]);
    await decode("amounts and transactions first", "getBlocks", "response", hex(list([headerFirst])));
    await decode("an amount twice", "getBlocks", "response", hex(list([Buffer.concat([text(HEADER.totalFee, "1"), uint(HEADER.height, 4), text(HEADER.totalFee, "2")])])));

    // Amounts: BigInt reads the text. White space around it, the empty text, a sign on decimal
    // text only, 0x, 0o and 0b; anything else is a SyntaxError. Values beyond 128 bits are read
    // by the reference; Heartwood refuses them (a documented difference the oracle notes).
    const amounts = [
        "0x10", "0X1f", "0o17", "0b101", " 12\n", "", " ", "\t ﻿7 ", "-0", "+5", "-5", "007",
        "170141183460469231731687303715884105727", "-170141183460469231731687303715884105728",
        "0x7fffffffffffffffffffffffffffffff",
        "1.5", "1e3", "0x", "-0x10", "+0x10", "12a", "1 2", "\u00857", "Infinity",
        "170141183460469231731687303715884105728", "-170141183460469231731687303715884105729",
        "0x" + "f".repeat(40), "9".repeat(200),
    ];
    for (const amount of amounts) {
        const label = amount.length > 48 ? `${amount.slice(0, 8)}... (${amount.length} characters)` : JSON.stringify(amount);
        for (const [field, key] of [[HEADER.totalAmount, "totalAmount"], [HEADER.reward, "reward"]]) {
            const block = Buffer.concat([uint(HEADER.height, 2), text(field, amount)]);
            await decode(`${key} ${label}`, "getBlocks", "response", hex(list([block])));
        }
    }
    await decode("a wide amount, then a bad block", "getBlocks", "response", hex(list([text(HEADER.totalAmount, "9".repeat(50)), Buffer.from("17", "hex")])));
    await decode("a wide amount, then a syntax error", "getBlocks", "response", hex(list([Buffer.concat([text(HEADER.totalAmount, "9".repeat(50)), text(HEADER.totalFee, "x")])])));
    return out;
};
