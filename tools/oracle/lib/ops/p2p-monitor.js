"use strict";
// Oracle operations of the network monitor's decisions and the download path:
//   monitor.networkHeight  {heights} -> {height}: getNetworkHeight over peers whose state heights
//                          are `heights` (null for a peer without one)
//   monitor.forkDecision   {height, peers, delegates} -> {forked, blocksToRollback}:
//                          checkNetworkHealth(fast) with the cleanse stubbed, our last block at
//                          `height`; each peer is {height, publicKeys, verification}, with
//                          verification {myHeight, theirHeight, highestCommonHeight} or null, and
//                          `delegates` maps a public key to its rank (null: a delegate without one)
//   monitor.downloadPlan   {peers, networkHeight, calls} -> {calls}:
//                          downloadBlocksFromHeight with a stub communicator, once per call on the
//                          same monitor (its chunk size and chunk cache carry over). The peers are
//                          [{ip, port}] in the repository's order, each with the state height
//                          `networkHeight`. A call is {from, maxParallel, checkThrottle, throttled,
//                          networkHeight, answers, orders}, its networkHeight replacing the peers'
//                          heights from then on: `answers` maps a peer's ip to {top, fail}, the blocks
//                          it serves up to height `top`, and the chunk starts at which its reply
//                          fails as the transaction reader fails; `orders` are the orders the
//                          shuffles of the call draw, each the input indices in their new order,
//                          which Math.random is made to produce through the reference's own
//                          shuffle. Each call's output is {requests, blocks, cached, nextChunkSize}:
//                          the peers asked per chunk start, the blocks returned as runs
//                          [first, last, ip], the cached chunks as [from, to], and the chunk size
//                          after the call
//   block.fromListed       {hex} -> {valid, downloaded, blocks}: the getBlocks codec, the reply
//                          check, the transactions read as fromBytesUnsafe reads them, and
//                          BlockFactory.fromData per block ({id, accepted}); {valid: false} for a
//                          reply the codec or the check refuses, {valid: true, downloaded: false}
//                          when the transaction reader refuses one
// An output may carry `notes` (not compared): stackOverflow when the codec overflowed the Node
// stack, wideAmount when a decoded amount lies outside the signed 128-bit range. Loaded by
// lib/ops.js.

const path = require("path");

// The signed 128-bit range that Heartwood's amounts hold.
const I128_MIN = -(2n ** 127n);
const I128_MAX = 2n ** 127n - 1n;
// The address the stub peers of the decisions get.
const STUB_IP = (index) => `192.0.2.${(index % 250) + 1}`;

const SILENT = { emergency() {}, alert() {}, critical() {}, error() {}, warning() {}, notice() {}, info() {}, debug() {} };

function uint32(value, name) {
    if (!Number.isSafeInteger(value) || value < 0 || value > 0xffffffff) throw new TypeError(`${name} must be an integer from 0 to 2^32 - 1`);
    return value;
}

function list(value, name) {
    if (!Array.isArray(value)) throw new TypeError(`${name} must be an array`);
    return value;
}

/** The Math.random values that make the kernel's shuffle put the input indices in `order`. */
function randomsFor(order) {
    const n = order.length;
    const sorted = [...order].sort((a, b) => a - b);
    if (!sorted.every((value, index) => value === index)) throw new TypeError(`order ${JSON.stringify(order)} is not a permutation`);
    const current = [...order];
    const picks = new Array(n);
    // The shuffle swaps position i with a position drawn from 0..i, for i upwards; undone
    // downwards, the draw at step i is where input i sits then.
    for (let i = n - 1; i >= 0; i--) {
        const at = current.indexOf(i);
        picks[i] = at;
        current[at] = current[i];
        current[i] = i;
    }
    return picks.map((pick, i) => (pick + 0.5) / (i + 1));
}

/** Run `fn` with Math.random giving the values of `orders`, all of which it must use. */
async function withOrders(orders, fn) {
    const values = list(orders, "orders").flatMap((order) => randomsFor(list(order, "order")));
    const original = Math.random;
    // The reference catches what a download job throws, so running out is noted, not thrown.
    let overdrawn = false;
    Math.random = () => {
        overdrawn = overdrawn || values.length === 0;
        return values.length === 0 ? 0 : values.shift();
    };
    try {
        const result = await fn();
        if (overdrawn) throw new Error("the shuffles drew more than the orders give");
        if (values.length !== 0) throw new Error(`the shuffles left ${values.length} values of the orders unused`);
        return result;
    } finally {
        Math.random = original;
    }
}

/** The [from, to] of a chunk key as the reference writes it ("[1,201, ..]"); to is null for "..". */
function rangeOf(key) {
    const [from, to] = key.slice(1, -1).split(", ");
    const number = (text) => Number(text.replace(/[^0-9]/g, ""));
    return [number(from), to === ".." ? null : number(to)];
}

/** Blocks as runs of consecutive heights from one peer: [first, last, ip]. */
function runs(blocks) {
    const out = [];
    for (const { height, ip } of blocks) {
        const last = out[out.length - 1];
        if (last && last[2] === ip && last[1] + 1 === height) last[1] = height;
        else out.push([height, height, ip]);
    }
    return out;
}

module.exports = function p2pMonitorOps({ solar, hexBuffer }) {
    let reference;

    /** The reference's network monitor, peers, chunk cache, codec and reply schemas, loaded once. */
    function load() {
        if (!reference) {
            const p2p = path.join(solar.solarDir, "packages", "p2p");
            const dist = (file) => require(path.join(p2p, "dist", file));
            const crypto = require(require.resolve("@solar-network/crypto", { paths: [p2p] }));
            if (crypto !== solar.lib) throw new Error("@solar-network/p2p loaded a second copy of @solar-network/crypto");
            reference = {
                NetworkMonitor: dist("network-monitor").NetworkMonitor,
                Peer: dist("peer").Peer,
                PeerVerificationResult: dist("peer-verifier").PeerVerificationResult,
                ChunkCache: dist("chunk-cache").ChunkCache,
                getBlocks: dist(path.join("socket-server", "codecs", "blocks")).getBlocks,
                replySchemas: dist("schemas").replySchemas,
            };
        }
        return reference;
    }

    /** A monitor with no collaborator but a silent logger. */
    function monitor() {
        const created = Object.create(load().NetworkMonitor.prototype);
        created.logger = SILENT;
        return created;
    }

    return {
        "monitor.networkHeight": ({ heights }) => {
            const { Peer } = load();
            const peers = list(heights, "heights").map((height, index) => {
                const peer = new Peer(STUB_IP(index), 4002);
                if (height !== null) peer.state.height = uint32(height, "height");
                return peer;
            });
            const created = monitor();
            created.repository = { getPeers: () => [...peers] };
            return { height: created.getNetworkHeight() };
        },

        "monitor.forkDecision": async ({ height, peers, delegates }) => {
            const { Peer, PeerVerificationResult } = load();
            uint32(height, "height");
            if (delegates === null || typeof delegates !== "object") throw new TypeError("delegates must be an object");
            const repository = list(peers, "peers").map((stub, index) => {
                const peer = new Peer(STUB_IP(index), 4002);
                if (stub.height !== null) peer.state.height = uint32(stub.height, "peer height");
                peer.publicKeys = [...list(stub.publicKeys, "publicKeys")];
                const v = stub.verification;
                if (v) peer.verificationResult = new PeerVerificationResult(uint32(v.myHeight, "myHeight"), uint32(v.theirHeight, "theirHeight"), uint32(v.highestCommonHeight, "highestCommonHeight"));
                return peer;
            });
            const has = (key) => Object.prototype.hasOwnProperty.call(delegates, key);
            const created = monitor();
            created.repository = { getPeers: () => [...repository] };
            created.discoverPeers = async () => false;
            created.cleansePeers = async () => undefined;
            created.getDelegatesOnThisNode = async () => [];
            created.stateStore = { getLastBlock: () => ({ data: { height } }) };
            created.configuration = { getRequired: () => 4002 };
            created.walletRepository = {
                hasByPublicKey: (key) => has(key),
                findByPublicKey: (key) => ({
                    hasAttribute: (name) => name === "delegate.rank" && delegates[key] !== null,
                    getAttribute: () => delegates[key],
                }),
            };
            const status = await created.checkNetworkHealth(true);
            return { forked: status.forked, blocksToRollback: status.blocksToRollback ?? null };
        },

        "monitor.downloadPlan": async ({ peers, networkHeight, calls }) => {
            const { Peer, ChunkCache } = load();
            uint32(networkHeight, "networkHeight");
            const repository = list(peers, "peers").map(({ ip, port }) => {
                const peer = new Peer(ip, port);
                peer.state.height = networkHeight;
                return peer;
            });
            const created = monitor();
            created.repository = { getPeers: () => [...repository] };
            created.chunkCache = Object.create(ChunkCache.prototype);
            created.chunkCache.downloadedChunksCache = [];
            created.chunkCache.downloadedChunksCacheMax = 100;
            created.downloadChunkSize = 400;
            const outputs = [];
            for (const call of list(calls, "calls")) {
                if (call.networkHeight !== undefined) {
                    for (const peer of repository) peer.state.height = uint32(call.networkHeight, "networkHeight");
                }
                const requests = new Map();
                const answers = call.answers || {};
                const throttled = new Set(call.throttled || []);
                created.communicator = {
                    wouldThrottleOnDownload: async (peer) => throttled.has(peer.ip),
                    getPeerBlocks: async (peer, { fromBlockHeight, blockLimit }) => {
                        if (!requests.has(fromBlockHeight)) requests.set(fromBlockHeight, []);
                        requests.get(fromBlockHeight).push(peer.ip);
                        const answer = answers[peer.ip] || {};
                        if ((answer.fail || []).includes(fromBlockHeight)) throw new Error("the transaction reader refused a transaction");
                        const blocks = [];
                        for (let h = fromBlockHeight + 1; h <= Math.min(fromBlockHeight + blockLimit, answer.top ?? 0); h++) blocks.push({ height: h, ip: peer.ip });
                        return blocks;
                    },
                };
                const download = () => created.downloadBlocksFromHeight(uint32(call.from, "from"), uint32(call.maxParallel, "maxParallel"), false, 30000, call.checkThrottle === true);
                const blocks = await withOrders(call.orders || [], download);
                outputs.push({
                    requests: [...requests.entries()].sort((a, b) => a[0] - b[0]),
                    blocks: runs(blocks),
                    cached: created.chunkCache.downloadedChunksCache.map((chunk) => rangeOf(chunk.key)),
                    nextChunkSize: created.downloadChunkSize,
                });
            }
            return { calls: outputs };
        },

        "block.fromListed": ({ hex }) => {
            const { getBlocks, replySchemas } = load();
            const { Blocks, Transactions, Validation } = solar.lib;
            const bytes = hexBuffer(hex, "hex");
            let reply;
            try {
                reply = getBlocks.response.deserialise(bytes);
            } catch (error) {
                const overflow = error instanceof RangeError && String(error.message).startsWith("Maximum call stack size exceeded");
                return overflow ? { valid: false, notes: { stackOverflow: true } } : { valid: false };
            }
            const wide = reply.some((block) =>
                [block.totalAmount, block.totalFee, block.reward].some((amount) => {
                    const value = BigInt(amount.toString());
                    return value < I128_MIN || value > I128_MAX;
                }),
            );
            const notes = wide ? { notes: { wideAmount: true } } : {};
            if (Validation.validator.validate(replySchemas["p2p.blocks.getBlocks"], reply).error) return { valid: false, ...notes };
            try {
                for (const block of reply) {
                    block.ip = STUB_IP(0);
                    for (const transaction in block.transactions) {
                        const { data } = Transactions.TransactionFactory.fromBytesUnsafe(Buffer.from(block.transactions[transaction], "hex"));
                        data.blockId = block.id;
                        block.transactions[transaction] = data;
                    }
                }
            } catch {
                return { valid: true, downloaded: false, ...notes };
            }
            const blocks = reply.map((data) => {
                try {
                    const block = Blocks.BlockFactory.fromData(data);
                    return block ? { id: block.data.id, accepted: true } : { id: null, accepted: false };
                } catch {
                    return { id: null, accepted: false };
                }
            });
            return { valid: true, downloaded: true, blocks, ...notes };
        },
    };
};
