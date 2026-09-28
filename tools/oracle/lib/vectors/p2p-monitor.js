"use strict";
// V26, the monitor: monitor.networkHeight over height lists; monitor.forkDecision cases where the
// fork heights sorted as text and as numbers disagree, at the thresholds of attesting peers and
// relays, with capped and upward rollbacks; monitor.downloadPlan over several peers and chunk
// sizes, with failed, empty and cached chunks and the backup peers in a drawn order; and
// block.fromListed on getBlocks replies of real blocks: kept and removed types, a memo that is
// not UTF-8, a vote name the rules remove, transaction counts, hex fields of odd lengths, and
// replies the check or the transaction reader refuses. The reply sizes the check bounds are V24's.
//
// The peers of the decisions use the documentation range 192.0.2.0/24, one subnet, so the draw
// round-robin across subnets is the reference's shuffle.

const crypto = require("crypto");
const path = require("path");

const { FIXED_AUX } = require("../ops");
const { GENERATOR, IPFS_HASH, SENDER } = require("./constants");

// The devnet's active delegates.
const DELEGATES = 53;

const peerIp = (n) => `192.0.2.${n}`;

/** A deterministic permutation of 0..n-1 from `seed` (Fisher-Yates over SHA-256 bytes). */
function permutation(n, seed) {
    const order = Array.from({ length: n }, (_, i) => i);
    for (let i = n - 1; i > 0; i--) {
        const digest = crypto.createHash("sha256").update(`${seed}/${i}`).digest();
        const j = digest.readUInt32BE(0) % (i + 1);
        [order[i], order[j]] = [order[j], order[i]];
    }
    return order;
}

/** `jobs` orders of `backups` peers each, drawn from `seed`. */
function orders(jobs, backups, seed) {
    return Array.from({ length: jobs }, (_, job) => permutation(backups, `${seed}/${job}`));
}

module.exports = async function monitorVectors({ solar, rec, must, craft, shared }) {
    const { Blocks, Transactions } = solar.lib;
    const H = shared.height;
    const out = [];
    const record = async (name, op, height, input) => out.push({ ...(await rec(op, height, input)), name });

    // ---- monitor.networkHeight ----------------------------------------------------------------
    for (const [name, heights] of [
        ["no peer", []],
        ["no known height", [null, 0, null]],
        ["one peer", [7]],
        ["two peers: the higher", [9, 3]],
        ["unsorted, with unknown heights", [null, 9, 3, 5, 0]],
        ["an even count", [4, 1, 3, 2]],
        ["duplicates", [5, 5, 5, 6]],
        ["the largest heights", [4294967295, 4294967294, 1]],
    ]) {
        await record(`network height: ${name}`, "monitor.networkHeight", H, { heights });
    }

    // ---- monitor.forkDecision -----------------------------------------------------------------
    const key = (n) => `02${n.toString(16).padStart(64, "0")}`;
    const delegates = {};
    for (let n = 1; n <= 60; n++) delegates[key(n)] = n;
    delegates[key(61)] = null;
    const keys = (from, count) => Array.from({ length: count }, (_, i) => key(from + i));
    const forkedAt = (common, mine, theirs) => ({ myHeight: mine, theirHeight: theirs, highestCommonHeight: common });
    const fork = (name, height, peers) => record(`fork decision: ${name}`, "monitor.forkDecision", height, { height, peers, delegates });
    const attesting = (from, count, common, height, theirs = height + 10) => ({
        height: theirs,
        publicKeys: keys(from, count),
        verification: forkedAt(common, height, theirs),
    });
    await fork("heights sorted as text: 9 after 100, and 9 wins", 120, [attesting(1, 20, 9, 120), attesting(21, 10, 100, 120)]);
    await fork("heights sorted as text: 9 first, ignored, then 100", 120, [attesting(1, 10, 9, 120), attesting(11, 20, 100, 120)]);
    await fork("four heights as text: 500, 50, 5, 1000", 2000, [
        attesting(1, 5, 5, 2000),
        attesting(6, 6, 50, 2000),
        attesting(12, 4, 500, 2000),
        attesting(16, 8, 1000, 2000),
        attesting(24, 7, 2000, 2000),
    ]);
    await fork("26 attestations: too few", 120, [attesting(1, 26, 100, 120)]);
    await fork("26 attestations and a relay above us", 120, [attesting(1, 26, 100, 120), { height: 121, publicKeys: [], verification: forkedAt(100, 120, 121) }]);
    await fork("26 attestations and a relay at our height", 120, [attesting(1, 26, 100, 120), { height: 120, publicKeys: [], verification: forkedAt(100, 120, 120) }]);
    await fork("a relay above us without a verification", 120, [attesting(1, 26, 100, 120), { height: 130, publicKeys: [], verification: null }]);
    await fork("no verified peer", 120, [{ height: 130, publicKeys: keys(1, 30), verification: null }]);
    await fork("ranks above 53, an unranked delegate and unknown keys do not count", 120, [
        { height: 130, publicKeys: [...keys(50, 10), key(61), key(99)], verification: forkedAt(100, 120, 130) },
        attesting(1, 20, 100, 120),
    ]);
    await fork("a key attested by two peers counts twice", 120, [attesting(1, 14, 100, 120), attesting(1, 14, 100, 120)]);
    await fork("the rollback capped at 5,000", 9000, [attesting(1, 30, 10, 9000)]);
    await fork("a rollback of exactly 5,000", 5010, [attesting(1, 30, 10, 5010)]);
    await fork("a common height above ours: no fork", 140, [attesting(1, 30, 150, 140, 200)]);
    await fork("peers on our chain: no fork", 120, [attesting(1, 30, 120, 120)]);
    await fork("peers behind us on our chain: no fork", 120, [attesting(1, 30, 110, 120, 110)]);
    await fork("more above the fork than on it: ignored", 120, [attesting(1, 10, 100, 120), attesting(11, 20, 115, 120)]);
    await fork("no peer", 120, []);

    // ---- monitor.downloadPlan -----------------------------------------------------------------
    const peers = (n) => Array.from({ length: n }, (_, i) => ({ ip: peerIp(i + 1), port: 4002 }));
    const serve = (n, top, fail = []) => Object.fromEntries(peers(n).map(({ ip }) => [ip, { top, fail }]));
    const plan = (name, input) => record(`download plan: ${name}`, "monitor.downloadPlan", H, input);
    await plan("three chunks to three peers", {
        peers: peers(3),
        networkHeight: 1200,
        calls: [{ from: 0, maxParallel: 10, answers: serve(3, 1200), orders: [[], [], []] }],
    });
    await plan("a failed middle chunk, the chunk above it cached, then taken", {
        peers: peers(3),
        networkHeight: 1200,
        calls: [
            { from: 0, maxParallel: 10, answers: serve(3, 1200, [400]), orders: [[], [], []] },
            // The last chunk's key is [801, ..]: the cached [801, 1,200] is not taken.
            { from: 400, maxParallel: 10, answers: serve(3, 1200), orders: [[0], [0]] },
            // Higher, [801, 1,200] is a middle chunk: taken from the cache.
            { from: 400, maxParallel: 10, networkHeight: 1600, answers: serve(3, 1600), orders: [[], []] },
        ],
    });
    await plan("the chunk size falls to a tenth after an empty download", {
        peers: peers(2),
        networkHeight: 1000,
        calls: [
            { from: 0, maxParallel: 10, answers: serve(2, 0), orders: [[], []] },
            { from: 0, maxParallel: 10, answers: serve(2, 0), orders: [[], []] },
            { from: 0, maxParallel: 10, answers: serve(2, 0), orders: [[], []] },
            { from: 0, maxParallel: 10, answers: serve(2, 0), orders: [[], []] },
            { from: 0, maxParallel: 10, answers: serve(2, 1000), orders: [[], []] },
            { from: 0, maxParallel: 10, answers: serve(2, 0), orders: [[], []] },
            { from: 0, maxParallel: 10, answers: serve(2, 1000), orders: [[], []] },
        ],
    });
    await plan("the last chunk may be empty, a middle one may not", {
        peers: peers(3),
        networkHeight: 1200,
        calls: [
            { from: 0, maxParallel: 10, answers: serve(3, 800), orders: [[], [], []] },
            { from: 0, maxParallel: 10, answers: { ...serve(3, 1200), [peerIp(2)]: { top: 400 } }, orders: [[], [], []] },
        ],
    });
    await plan("a reader failure on the last chunk is not an answer", {
        peers: peers(2),
        networkHeight: 800,
        calls: [{ from: 0, maxParallel: 10, answers: { ...serve(2, 800), [peerIp(2)]: { top: 800, fail: [400] } }, orders: [[], []] }],
    });
    await plan("throttled peers left out", {
        peers: peers(4),
        networkHeight: 1600,
        calls: [
            { from: 0, maxParallel: 10, checkThrottle: true, throttled: [peerIp(1), peerIp(3)], answers: serve(4, 1600), orders: [[], []] },
            { from: 0, maxParallel: 10, checkThrottle: true, throttled: peers(4).map((p) => p.ip), answers: serve(4, 1600), orders: [] },
        ],
    });
    await plan("backup peers in a drawn order", {
        peers: peers(6),
        networkHeight: 2000,
        calls: [
            {
                from: 0,
                maxParallel: 2,
                answers: { ...serve(6, 2000, [0, 400]), [peerIp(5)]: { top: 2000, fail: [400] }, [peerIp(4)]: { top: 2000 } },
                orders: orders(2, 4, "backups"),
            },
        ],
    });
    await plan("ten chunks of twelve peers, the fourth failing everywhere", {
        peers: peers(12),
        networkHeight: 10000,
        calls: [
            { from: 0, maxParallel: 10, answers: serve(12, 10000, [1200]), orders: orders(10, 2, "ten") },
            { from: 1200, maxParallel: 10, answers: serve(12, 10000), orders: orders(4, 2, "again") },
        ],
    });
    await plan("chunks of 40 over five peers", {
        peers: peers(5),
        networkHeight: 100,
        calls: [
            { from: 0, maxParallel: 10, answers: serve(5, 0), orders: orders(1, 4, "empty") },
            { from: 0, maxParallel: 10, answers: serve(5, 100), orders: orders(3, 2, "forty") },
        ],
    });
    await plan("a network height at or below ours: one chunk", {
        peers: peers(3),
        networkHeight: 50,
        calls: [
            { from: 50, maxParallel: 10, answers: serve(3, 52), orders: [[0, 1]] },
            { from: 60, maxParallel: 10, answers: serve(3, 52), orders: [[1, 0]] },
        ],
    });
    await plan("no peer", { peers: [], networkHeight: 0, calls: [{ from: 0, maxParallel: 10, answers: {}, orders: [] }] });

    // ---- block.fromListed ---------------------------------------------------------------------
    const p2p = path.join(solar.solarDir, "packages", "p2p");
    const { getBlocks } = require(path.join(p2p, "dist", "socket-server", "codecs", "blocks"));
    const aux = FIXED_AUX.toString("hex");
    const decodeId = (hex) => Transactions.TransactionFactory.fromBytes(Buffer.from(hex, "hex")).id;
    const unsafeId = (hex) => Transactions.TransactionFactory.fromBytesUnsafe(Buffer.from(hex, "hex")).id;
    let previousBlock = "ab".repeat(32);
    /** A block's bytes with `transactions`, chained to the last one made. */
    const made = (height, transactions, ids = transactions.map(decodeId)) => {
        const bytes = craft.blockBytes({ height, timestamp: 8 * (height - 1), previousBlock, reward: "0", passphrase: GENERATOR, aux, transactions, ids });
        const { data } = Blocks.Deserialiser.deserialise(Buffer.from(bytes, "hex"), true);
        previousBlock = data.id;
        return { data, transactions };
    };
    /** The listed form of a made block, with `changes` (a key set to undefined is left out). */
    const listed = ({ data, transactions }, changes = {}) => {
        const block = {
            id: data.id,
            version: data.version,
            timestamp: data.timestamp,
            previousBlock: data.previousBlock,
            height: data.height,
            numberOfTransactions: data.numberOfTransactions,
            totalAmount: data.totalAmount.toString(),
            totalFee: data.totalFee.toString(),
            reward: data.reward.toString(),
            payloadLength: data.payloadLength,
            payloadHash: data.payloadHash,
            generatorPublicKey: data.generatorPublicKey,
            blockSignature: data.blockSignature,
            transactions,
            ...changes,
        };
        for (const [k, v] of Object.entries(block)) if (v === undefined) delete block[k];
        return block;
    };
    const reply = (blocks) => getBlocks.response.serialise(blocks).toString("hex");
    const fromListed = (name, blocks, height = H) => record(`listed: ${name}`, "block.fromListed", height, { hex: reply(blocks) });

    const tx = (name) => shared.tx[name].hex;
    const chain = [made(3, [tx("transfer-1"), tx("transfer-2")]), made(4, [tx("burn"), tx("vote-2")]), made(5, [])];
    await fromListed("three blocks of kept types", chain.map((b) => listed(b)));
    await fromListed("no block", []);
    await fromListed("the generator's key in capitals", [listed(chain[0], { generatorPublicKey: chain[0].data.generatorPublicKey.toUpperCase() })]);
    await fromListed("ids and hashes in capitals", [
        listed(chain[1], { id: chain[1].data.id.toUpperCase(), previousBlock: chain[1].data.previousBlock.toUpperCase(), payloadHash: chain[1].data.payloadHash.toUpperCase() }),
    ]);
    await fromListed("absent version, count and payload length", [listed(chain[0], { version: undefined, numberOfTransactions: undefined, payloadLength: undefined })]);
    await fromListed("a count of 0 with transactions", [listed(chain[0], { numberOfTransactions: 0 })]);
    await fromListed("a count of 3 with two transactions", [listed(chain[0], { numberOfTransactions: 3 })]);
    await fromListed("a payload hash with an odd last digit", [listed(chain[0], { payloadHash: chain[0].data.payloadHash + "7" })]);
    await fromListed("a payload hash one digit short", [listed(chain[0], { payloadHash: chain[0].data.payloadHash.slice(0, 63) })]);
    await fromListed("a payload hash of 33 bytes", [listed(chain[0], { payloadHash: chain[0].data.payloadHash + "00" })]);
    await fromListed("no payload hash", [listed(chain[0], { payloadHash: undefined })]);
    await fromListed("no block signature", [listed(chain[0], { blockSignature: undefined })]);
    await fromListed("a total amount beyond 64 bits", [listed(chain[0], { totalAmount: "18446744073709551616" })]);
    await fromListed("a reward of 2^63 - 1", [listed(chain[2], { reward: "9223372036854775807" })]);
    await fromListed("a reward of 2^63", [listed(chain[2], { reward: "9223372036854775808" })]);
    await fromListed("height 1 with a zero previous block", [listed(made(1, []), { previousBlock: "0".repeat(64) })]);

    // Transactions the reader refuses, and forms it reads that a rebuild changes.
    await fromListed("a transaction the reader refuses, after a good block", [listed(chain[2]), listed(made(6, [tx("transfer-1")]), { transactions: ["ff"] })]);
    const ipfs = must(await rec("tx.build", H, { builder: "ipfs", calls: [["ipfsAsset", IPFS_HASH], ["nonce", "40"]], sign: SENDER })).output.hex;
    await fromListed("a removed type (IPFS)", [listed(made(7, [ipfs], [unsafeId(ipfs)]))]);
    const SIGNATURE = 64;
    const unsigned = (hex) => hex.slice(0, hex.length - 2 * SIGNATURE);
    const memo = tx("transfer-memo-replacement");
    const notUtf8 = craft.withMemoBytes(memo, Buffer.from([0xff, 0xff]), SENDER, unsigned(memo));
    await fromListed("a memo that is not UTF-8", [listed(made(8, [notUtf8], [unsafeId(notUtf8)]))]);
    const vote = tx("vote-1");
    const renamed = craft.withAsset(vote, craft.voteAsset([["genesis_1", 10000], ["A", 5000]]), SENDER, unsigned(vote));
    await fromListed("a vote name the rules remove", [listed(made(9, [renamed], [unsafeId(renamed)]))]);

    return out;
};
