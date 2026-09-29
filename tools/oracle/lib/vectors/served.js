"use strict";
// Vector class V32-served: the public controllers' answers (p2p.controller) on the one-node corpus
// at heights 2, 53, 54, 106 and 265, and over a synthetic store of full blocks:
//   getBlocks       from 0, 1, a middle height, the tip and past it, with a limit of 1, 400 and none,
//                   with and without headersOnly; the 20 MiB cut over the synthetic store
//   getCommonBlocks no known id, one, several, the genesis block's id alone (a row the node's
//                   memory no longer holds), ten ids, duplicates, an id in capitals
//   getPeers        equal latencies, port -1, the caller listed (also as an IPv4-mapped address),
//                   2,001 peers, peers without a latency
//   getUnconfirmedTransactions, postTransactions, getStatus
//   postBlock       pinged, not chaining, unregistered and unranked generators, a new valid block, a
//                   block from the forger, the reduced reward, above maxTransactions, and the blocks
//                   the reference receives that Heartwood refuses at the receipt (a memo that is not
//                   UTF-8, a header its schema refuses)
// The records whose name begins `documented:` are the answers Heartwood gives differently on
// purpose. The builder needs the corpus (lib/corpus.js): without it the class is not written.

const fs = require("fs");
const path = require("path");

const { ReplaySession, loadCorpus } = require("../corpus");
const { fullTransaction, syntheticStore } = require("../fixtures/full-blocks");
const { FIXED_AUX } = require("../ops");

const CORPUS = "txgen-one-node";
const HEADER = 205;
const TIPS = [2, 53, 54, 106, 265];
const CALLER = "10.0.0.9";
const FORGER = "127.0.0.1";
// The byte offsets of a block header's fields.
const COUNT_AT = 44;
const TOTAL_AT = 48;

function corpusDir() {
    const candidates = [path.join(__dirname, "..", "..", "corpus", CORPUS), path.join(__dirname, "..", "..", "..", "difftest", "corpus", CORPUS)];
    return candidates.find((dir) => fs.existsSync(path.join(dir, "corpus.json")));
}

const pad = (n, width = 64) => n.toString(16).padStart(width, "0");

module.exports = async function servedVectors({ solar, devnet, rec, must, craft }) {
    const dir = corpusDir();
    if (!dir) return null;
    const c = loadCorpus(dir);
    const { Blocks } = solar.lib;
    const milestones = JSON.stringify(c.crypto.milestones);
    const epochMs = new Date(c.crypto.milestones[0].epoch).getTime();

    // The genesis block of the corpus, as the reference's first start on its chain definition makes it.
    const session = new ReplaySession(solar, c.crypto);
    let genesisBlock;
    await session.chain.withConfig(
        () => devnet.restore(),
        async () => {
            await session.start();
            genesisBlock = session.chain.lastBlock();
        },
    );
    const genesisHex = genesisBlock.serialised;
    const genesisId = genesisBlock.data.id;

    const blk = (height) => (height === 1 ? { h: 1, id: genesisId, hex: genesisHex } : c.blocks.find((b) => b.h === height));
    const header = (hex) => Blocks.Deserialiser.deserialise(Buffer.from(hex, "hex"), true).data;
    const headerOnly = (hex) => hex.slice(0, HEADER * 2);
    const codecs = require(path.join(solar.solarDir, "packages", "p2p", "dist", "socket-server", "codecs")).Codecs;
    const encode = (name, value) => Buffer.from(codecs[name].request.serialise(value)).toString("hex");
    const patched = (hex, at, bytes) => hex.slice(0, at * 2) + Buffer.from(bytes).toString("hex") + hex.slice((at + bytes.length) * 2);
    const u32 = (n) => {
        const b = Buffer.alloc(4);
        b.writeUInt32LE(n);
        return b;
    };
    const out = [];

    /** A served record at `height`: the request `payload` of `route` with `fields` of the input. */
    const serve = async (name, height, route, payload, fields = {}) => {
        const request = payload === undefined ? "" : encode(route.split(".")[2], payload);
        const input = { route, request, ip: CALLER, remoteAccess: [FORGER], milestones, ...fields };
        out.push({ ...must(await rec("p2p.controller", height, input)), name });
    };

    /**
     * The store of a node at `tip`: its block, and the rows of `full` in full and of `headers` as
     * headers, in the corpus (height 1 is the genesis block).
     */
    const store = (tip, full = [], headers = []) => ({
        tip: blk(tip).hex,
        blocks: [
            ...full.filter((h) => h !== tip).map((h) => blk(h).hex),
            ...headers.filter((h) => h !== tip && !full.includes(h)).map((h) => headerOnly(blk(h).hex)),
        ],
    });
    const range = (from, to) => Array.from({ length: Math.max(0, to - from + 1) }, (_, i) => from + i);

    // getPeers.
    const peers = (n, at) => Array.from({ length: n }, (_, i) => ({ ip: `10.${at}.${i >> 8}.${i & 255}`, port: 4000 + (i % 100), latency: (i * 7) % 5 }));
    const peerRecord = (name, list, ip = CALLER, height = 106) => serve(name, height, "p2p.peer.getPeers", {}, { ip, peers: list });
    await peerRecord("getPeers: none", []);
    await peerRecord("getPeers: sorted by latency", [
        { ip: "10.0.0.1", port: 4000, latency: 30 },
        { ip: "10.0.0.2", port: 4001, latency: 10 },
        { ip: "10.0.0.3", port: 4002, latency: 20 },
    ]);
    await peerRecord("getPeers: equal latencies keep their order", [
        { ip: "10.0.0.5", port: 4001, latency: 7 },
        { ip: "10.0.0.4", port: 4000, latency: 7 },
        { ip: "10.0.0.7", port: 4003, latency: 3 },
        { ip: "10.0.0.6", port: 4002, latency: 7 },
        { ip: "10.0.0.8", port: 4004, latency: 3 },
    ]);
    await peerRecord("getPeers: port -1 entries are dropped", [
        { ip: "10.0.0.1", port: -1, latency: 1 },
        { ip: "10.0.0.2", port: 4001, latency: 2 },
        { ip: "10.0.0.3", port: -1, latency: 3 },
    ]);
    await peerRecord("getPeers: the caller is not listed", [
        { ip: "10.0.0.1", port: 4000, latency: 2 },
        { ip: CALLER, port: 4001, latency: 1 },
        { ip: "10.0.0.2", port: 4002, latency: 3 },
    ]);
    await peerRecord(
        "getPeers: an IPv4-mapped caller",
        [
            { ip: "10.0.0.9", port: 4000, latency: 1 },
            { ip: "::ffff:10.0.0.9", port: 4001, latency: 2 },
            { ip: "10.0.0.2", port: 4002, latency: 3 },
        ],
        "::ffff:10.0.0.9",
    );
    await peerRecord(
        "getPeers: an IPv6 caller",
        [
            { ip: "2001:db8::1", port: 4000, latency: 1 },
            { ip: "2001:db8::2", port: 4001, latency: 2 },
        ],
        "2001:db8::1",
    );
    await peerRecord("getPeers: one peer without a latency", [{ ip: "10.0.0.1", port: 4000 }]);
    await peerRecord("getPeers: 2000 peers", peers(2000, 1));
    await peerRecord("getPeers: 2001 peers with tied latencies", peers(2001, 2));
    await peerRecord("documented: getPeers sorts a peer without a latency last", [
        { ip: "10.0.0.1", port: 4000 },
        { ip: "10.0.0.2", port: 4001, latency: 5 },
    ]);

    // getBlocks over the corpus.
    const blocks = async (name, tip, last, limit, headersOnly) => {
        const payload = { lastBlockHeight: last };
        if (limit !== undefined) payload.blockLimit = limit;
        if (headersOnly !== undefined) payload.headersOnly = headersOnly;
        const count = Math.min(limit || 400, Math.max(0, tip - last));
        const rows = range(last + 1, last + count);
        const kept = rows.filter((h) => h <= tip);
        await serve(`getBlocks: ${name}`, tip, "p2p.blocks.getBlocks", payload, { store: store(tip, kept) });
    };
    for (const tip of [2, 53, 54, 106]) {
        await blocks(`tip ${tip}, all of them`, tip, 0, undefined, false);
        await blocks(`tip ${tip}, from the tip`, tip, tip, undefined, false);
    }
    await blocks("tip 106, from 102", 106, 102, 400, false);
    await blocks("tip 106, from 102, headers only", 106, 102, 400, true);
    for (const last of [0, 1, 130, 264, 265, 266]) {
        for (const limit of [1, 400, undefined]) {
            for (const headersOnly of [false, true]) {
                await blocks(`tip 265, from ${last}, limit ${limit ?? "absent"}${headersOnly ? ", headers only" : ""}`, 265, last, limit, headersOnly);
            }
        }
    }
    // Zero and negative limits and heights the codec allows fall back to 400 or an empty answer.
    await blocks("tip 106, a limit of 0", 106, 100, 0, false);

    // getBlocks over the synthetic store: the reference stops at the 20 MiB cut.
    const transaction = await devnet.withMilestones(milestones, () => fullTransaction(solar));
    const synthetic = syntheticStore({ from: 2, count: 12, transaction });
    const syntheticTip = blk(265).hex;
    for (const [name, last, limit] of [
        ["from the first row, limit 400", 0, 400],
        ["from the first row, no limit", 0, undefined],
        ["from the third row, limit 400", 3, 400],
        ["a limit that stops before the cut", 0, 4],
        ["from the last rows, no cut", 8, 400],
    ]) {
        const payload = { lastBlockHeight: last };
        if (limit !== undefined) payload.blockLimit = limit;
        await serve(`getBlocks: synthetic store, ${name}`, 265, "p2p.blocks.getBlocks", payload, { store: { tip: syntheticTip, synthetic }, digest: true });
    }
    await serve("getBlocks: synthetic store, headers only", 265, "p2p.blocks.getBlocks", { lastBlockHeight: 0, headersOnly: true }, { store: { tip: syntheticTip, synthetic }, digest: true });

    // getCommonBlocks.
    const common = (name, tip, ids, full = [], headers = []) =>
        serve(`getCommonBlocks: ${name}`, tip, "p2p.peer.getCommonBlocks", { ids }, { store: store(tip, full, headers) });
    const id = (h) => blk(h).id;
    for (const tip of TIPS) {
        await common(`no id known, tip ${tip}`, tip, []);
        await common(`an unknown id, tip ${tip}`, tip, ["f".repeat(64)]);
    }
    await common("one id in memory", 106, [id(104)], [], [104]);
    await common("several, the highest wins", 106, [id(53), id(104), id(80), "e".repeat(64)], [], [53, 104, 80]);
    await common("several in the order of the request, the highest last", 265, [id(200), id(150), id(240)], [], [200, 150, 240]);
    await common("the genesis block's id alone", 265, [genesisId], [], [1]);
    await common("the genesis block and one in memory", 265, [genesisId, id(250)], [], [1, 250]);
    await common("one below the memory window and one in it", 265, [id(100), id(200)], [], [100, 200]);
    await common("ten ids", 265, [1, 3, 5, 50, 100, 150, 165, 166, 250, 264].map(id), [], [1, 3, 5, 50, 100, 150, 165, 166, 250, 264]);
    await common("duplicates", 106, [id(104), id(104), id(53), id(53)], [], [104, 53]);
    await common("the tip's id", 106, [id(106)]);
    await common("an id in capitals", 106, [id(104).toUpperCase(), id(53)], [], [104, 53]);
    await common("ids of blocks of the database only", 54, [id(2), id(3)], [], [2, 3]);

    // getStatus.
    const keys = [...new Set(c.blocks.slice(0, 40).map((b) => header(b.hex).generatorPublicKey))].slice(0, 6);
    const network = c.crypto.network;
    const peerConfig = {
        version: "4.3.1",
        network: { version: network.pubKeyHash, name: network.name, nethash: network.nethash, explorer: network.client.explorer, token: { name: network.client.token, symbol: network.client.symbol } },
        plugins: { "@solar-network/api": { enabled: true, port: 6003, estimateTotalCount: true }, "@solar-network/webhooks": { enabled: false, port: 6004 } },
    };
    for (const tip of TIPS) {
        const at = epochMs + tip * 8000;
        const status = (now) => ({ tip: { hex: blk(tip).hex }, now, peerConfig, registered: keys, forgerKeys: [] });
        await serve(`getStatus: tip ${tip}, in the first half of a slot`, tip, "p2p.peer.getStatus", undefined, { status: status(at + 500) });
        await serve(`getStatus: tip ${tip}, in the second half of a slot`, tip, "p2p.peer.getStatus", undefined, { status: status(at + 5500) });
    }

    // getUnconfirmedTransactions and postTransactions, over the corpus's own transactions.
    const bytesOf = (hex) => {
        const buffer = Buffer.from(hex, "hex");
        const count = header(hex).numberOfTransactions;
        const list = [];
        let offset = HEADER + 4 * count;
        for (let i = 0; i < count; i++) {
            const length = buffer.readUInt32LE(HEADER + 4 * i);
            list.push(buffer.subarray(offset, offset + length));
            offset += length;
        }
        return list;
    };
    const txs = [60, 64, 68, 4].flatMap((h) => bytesOf(blk(h).hex));
    const hexes = txs.map((b) => b.toString("hex"));
    const sha = (b) => require("crypto").createHash("sha256").update(b).digest("hex");
    const ids = txs.map(sha);
    const pool = (fields) => ({ size: fields.candidates.length, ...fields });
    const unconfirmed = (name, payload, fields) => serve(`getUnconfirmedTransactions: ${name}`, 106, "p2p.transactions.getUnconfirmedTransactions", payload, { pool: pool(fields) });
    await unconfirmed("an empty pool", {}, { candidates: [], size: 0 });
    await unconfirmed("an empty pool, count only", { countOnly: true }, { candidates: [], size: 0 });
    await unconfirmed("the candidates", {}, { candidates: hexes.slice(0, 5) });
    await unconfirmed("count only", { countOnly: true }, { candidates: hexes.slice(0, 5) });
    await unconfirmed("a pool larger than the candidates", {}, { candidates: hexes.slice(0, 3), size: 40 });
    await unconfirmed("with ids to exclude", { exclude: ids.slice(0, 2) }, { candidates: hexes.slice(2, 6) });
    await unconfirmed("count only with ids to exclude", { countOnly: true, exclude: ids.slice(0, 2) }, { candidates: hexes.slice(2, 6) });
    const post = (name, list, fields = {}) => serve(`postTransactions: ${name}`, 106, "p2p.transactions.postTransactions", { transactions: list }, { pool: { size: 0, candidates: [], ...fields } });
    await post("valid entries", txs.slice(0, 3));
    await post("no entries", []);
    await post("a duplicate in the request", [txs[0], txs[1], txs[0]]);
    await post("a transaction the pool holds", txs.slice(0, 3), { known: [ids[1]] });
    await post("refused entries", txs.slice(0, 4), { refuse: [ids[0], ids[2]] });
    await post("every entry refused", txs.slice(0, 2), { refuse: ids.slice(0, 2) });

    // postBlock.
    const delegate = (h, rank = 5) => ({ publicKey: header(blk(h).hex).generatorPublicKey, username: `delegate_${h}`, rank });
    const blockRequest = (hex) => ({ block: Buffer.from(hex, "hex") });
    const receive = (name, tip, hex, fields = {}) => serve(`postBlock: ${name}`, tip, "p2p.blocks.postBlock", blockRequest(hex), { store: store(tip), ...fields });
    await receive("pinged", 105, blk(106).hex, { pinged: true });
    await receive("pinged with a header only", 105, headerOnly(blk(106).hex), { pinged: true });
    await receive("not chaining, another previous block", 105, blk(107).hex, { delegates: [delegate(107)] });
    await receive("not chaining, the last downloaded block is older", 105, blk(106).hex, { delegates: [delegate(106)], lastDownloaded: blk(53).hex });
    await receive("an unregistered generator", 105, blk(106).hex);
    await receive("a delegate without a rank", 105, blk(106).hex, { delegates: [delegate(106, null)] });
    await receive("a new valid block", 105, blk(106).hex, { delegates: [delegate(106)] });
    await receive("a new valid block with transactions", 59, blk(60).hex, { delegates: [delegate(60, 17)] });
    await receive("a block from the forger", 105, blk(107).hex, { ip: FORGER, delegates: [delegate(107)] });
    await receive("a block from the forger, not chaining, unregistered", 105, blk(107).hex, { ip: FORGER });
    await receive("the caller's address is mapped", 105, blk(106).hex, { ip: "::ffff:10.0.0.9", delegates: [delegate(106)] });
    // The reduced reward: a block at the secondary reward from a delegate whose rank pays more.
    const rewarded = (rank, already) => ({ delegates: [delegate(106, rank)], alreadyForged: already });
    const reduced = await devnet.withMilestones(milestones, () => {
        const previous = header(blk(105).hex);
        const passphrase = "reduced reward delegate";
        const bytes = craft.blockBytes({ height: 106, timestamp: previous.timestamp + 8, previousBlock: previous.id, reward: "180000000", passphrase, aux: FIXED_AUX.toString("hex"), transactions: [], ids: [] });
        return { hex: bytes, key: solar.lib.Identities.Keys.fromPassphrase(passphrase).publicKey.secp256k1 };
    });
    const reducedDelegate = (rank) => ({ publicKey: reduced.key, username: "reduced_delegate", rank });
    await receive("the reward was reduced", 105, reduced.hex, { delegates: [reducedDelegate(30)], alreadyForged: true });
    await receive("the secondary reward, not forged before", 105, reduced.hex, { delegates: [reducedDelegate(30)], alreadyForged: false });
    await receive("the secondary reward at the rank's own reward", 105, reduced.hex, { delegates: [reducedDelegate(1)], alreadyForged: true });
    await receive("another reward than the secondary", 105, blk(106).hex, rewarded(30, true));
    // Over maxTransactions: the reference throws, the server answers 500 and ends the socket.
    const tip105 = header(blk(105).hex);
    await receive("above maxTransactions", 105, patched(blk(106).hex, COUNT_AT, u32(151)), { delegates: [delegate(106)] });
    await receive("above maxTransactions from the forger", 105, patched(blk(106).hex, COUNT_AT, u32(151)), { ip: FORGER, delegates: [delegate(106)] });
    // The blocks Heartwood refuses at the receipt, which the reference accepts.
    const raw = await devnet.withMilestones(milestones, () => {
        const { Identities, Transactions } = solar.lib;
        const passphrase = "raw memo delegate";
        const base = Transactions.BuilderFactory.transfer()
            .addTransfer(Identities.Address.fromPassphrase("raw memo recipient"), "1000")
            .memo("x")
            .nonce("1")
            .fee("100000000")
            .sign("raw memo sender")
            .build();
        const transaction = craft.withMemoBytes(base.serialised.toString("hex"), Buffer.from([0xff, 0xfe, 0x80]), "raw memo sender");
        const id = Transactions.TransactionFactory.fromBytesUnsafe(Buffer.from(transaction, "hex")).id;
        const bytes = craft.blockBytes({ height: 106, timestamp: tip105.timestamp + 8, previousBlock: tip105.id, reward: "200000000", passphrase, aux: FIXED_AUX.toString("hex"), transactions: [transaction], ids: [id] });
        return { hex: bytes, key: Identities.Keys.fromPassphrase(passphrase).publicKey.secp256k1 };
    });
    await receive("documented: a memo that is not UTF-8 is accepted at the receipt", 105, raw.hex, { delegates: [{ publicKey: raw.key, username: "raw_delegate", rank: 5 }] });
    await receive("documented: a header the schema refuses is accepted at the receipt", 105, patched(blk(106).hex, TOTAL_AT, Buffer.alloc(8, 0xff)), { delegates: [delegate(106)] });
    return out;
};
