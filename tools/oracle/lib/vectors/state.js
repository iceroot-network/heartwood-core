"use strict";
// The vector classes of wallets, ranking and rounds:
//   V08-ranking       buildDelegateRanking and setDelegatesRound over generated delegate sets (53 to
//                     80 delegates, many equal vote balances, resigned delegates), their edges, and
//                     the public-key tie-break: 10,000 random keys of equal vote balance, every value
//                     of the first key byte after the prefix, and keys that first differ at each
//                     byte position;
//   V09-shuffle       the forging order of rounds 1 to 10,000 for every count from 1 to 53, as one
//                     SHA-256 per count, full orders of rounds 1 to 100 for counts 1, 2, 3, 52 and 53,
//                     and the rounds up to 60,000,000 whose shuffle of 53 rejects a draw;
//   V15-vote-amounts  a voter's shares for edge and random balances and vote maps of 1 to 53
//                     entries.
// Inputs come from a fixed SHA-256 counter stream, so every run writes the same records.

const crypto = require("crypto");

/** Deterministic bytes: SHA-256(label || counter), block after block. */
function stream(label) {
    let counter = 0;
    let block = Buffer.alloc(0);
    const bytes = (n) => {
        const out = [];
        while (out.length < n) {
            if (block.length === 0) {
                const c = Buffer.alloc(8);
                c.writeBigUInt64BE(BigInt(counter++));
                block = crypto.createHash("sha256").update(label).update(c).digest();
            }
            out.push(block[0]);
            block = block.subarray(1);
        }
        return Buffer.from(out);
    };
    // A number from 0 to n - 1 (n at most 2^32; the small bias is irrelevant for test inputs).
    const below = (n) => bytes(4).readUInt32BE(0) % n;
    // An integer from 0 to 2^64 - 1 as decimal text.
    const u64 = () => bytes(8).readBigUInt64BE(0).toString();
    return { bytes, below, u64 };
}

/** A valid compressed public key: prefix and x from `next`, retried until x is on the curve. */
function keyFrom(Identities, next) {
    for (;;) {
        const candidate = next();
        if (Identities.PublicKey.verify(candidate.toString("hex"))) return candidate.toString("hex");
    }
}

function randomKey(Identities, random) {
    return keyFrom(Identities, () => Buffer.concat([Buffer.from([2 + random.below(2)]), random.bytes(32)]));
}

async function ranking({ rec, must, solar, shared }) {
    const { Identities } = solar.lib;
    const H = shared.height;
    const random = stream("heartwood V08 ranking");
    const out = [];
    const record = async (name, input) => out.push({ ...must(await rec("state.rank", H, input)), name });
    const refused = async (name, input) => {
        const r = await rec("state.rank", H, input);
        if (!r.error) throw new Error(`state.rank ${name}: the reference ranked a set it should refuse`);
        out.push({ ...r, name });
    };
    const named = (entries) => entries.map((entry, i) => ({ username: `d${String(i).padStart(3, "0")}`, ...entry }));

    // Generated sets: vote balances from a small pool (many ties) or unique, some resigned.
    const pool = ["0", "1", "100000000", "250000000000", "250000000001", "9223372036854775807"];
    for (let size = 53; size <= 80; size++) {
        const delegates = named(
            Array.from({ length: size }, () => {
                const entry = { publicKey: randomKey(Identities, random), voteBalance: random.below(3) === 0 ? random.u64() : pool[random.below(pool.length)] };
                const r = random.below(8);
                if (r === 0) entry.resigned = 0;
                else if (r === 1) entry.resigned = 1;
                return entry;
            }),
        );
        await record(`${size} delegates, ties and resignations`, size % 2 === 0 ? { delegates } : { delegates, maxDelegates: 53 });
    }

    // Edges.
    const keys = Array.from({ length: 60 }, () => randomKey(Identities, random));
    const equal = (n, voteBalance) => named(keys.slice(0, n).map((publicKey) => ({ publicKey, voteBalance })));
    await record("53 delegates, all vote balances 0", { delegates: equal(53, "0") });
    await record("53 delegates, equal vote balances", { delegates: equal(53, "123456789") });
    await record("the largest vote balances", { delegates: named(keys.slice(0, 55).map((publicKey, i) => ({ publicKey, voteBalance: (18446744073709551615n - BigInt(i % 3)).toString() }))) });
    await record("54 delegates, one resigned, a round of 53", { delegates: equal(54, "7").map((d, i) => (i === 10 ? { ...d, resigned: 0 } : d)) });
    await record("53 delegates, one resigned: too few for the round", { delegates: equal(53, "7").map((d, i) => (i === 10 ? { ...d, resigned: 1 } : d)) });
    await record("every delegate resigned", { delegates: equal(5, "7").map((d, i) => ({ ...d, resigned: i % 2 })), maxDelegates: 0 });
    await record("no delegates, a round of 0", { delegates: [], maxDelegates: 0 });
    await record("no delegates, a round of 1", { delegates: [], maxDelegates: 1 });
    await record("one delegate", { delegates: equal(1, "5"), maxDelegates: 1 });
    await record("a round larger than the ranking", { delegates: equal(3, "5"), maxDelegates: 4 });
    await refused("identical keys, equal vote balances", {
        delegates: named([
            { publicKey: keys[0], voteBalance: "5" },
            { publicKey: keys[1], voteBalance: "9" },
            { publicKey: keys[0], voteBalance: "5" },
        ]),
        maxDelegates: 1,
    });
    await record("identical keys, different vote balances", {
        delegates: named([
            { publicKey: keys[0], voteBalance: "5" },
            { publicKey: keys[0], voteBalance: "6" },
        ]),
        maxDelegates: 2,
    });
    await record("identical keys, one resigned", {
        delegates: named([
            { publicKey: keys[0], voteBalance: "5" },
            { publicKey: keys[0], voteBalance: "5", resigned: 0 },
        ]),
        maxDelegates: 1,
    });

    // The key tie-break over 10,000 random keys of equal vote balance, in 20 sets of 500.
    for (let set = 0; set < 20; set++) {
        const delegates = named(Array.from({ length: 500 }, () => ({ publicKey: randomKey(Identities, random), voteBalance: "1000" })));
        await record(`key order, 500 random keys, set ${set + 1}`, { delegates, maxDelegates: 53 });
    }

    // Every value of the first byte of x, with both prefixes: 512 keys of equal vote balance.
    const firstByte = [];
    for (const prefix of [2, 3]) {
        for (let b = 0; b < 256; b++) {
            firstByte.push(keyFrom(Identities, () => Buffer.concat([Buffer.from([prefix, b]), random.bytes(31)])));
        }
    }
    await record("key order, every first byte of x with both prefixes", { delegates: named(firstByte.map((publicKey) => ({ publicKey, voteBalance: "1000" }))), maxDelegates: 53 });

    // Keys that first differ at each byte position of x: four keys per position.
    const positions = [];
    for (let p = 1; p <= 32; p++) {
        const base = Buffer.concat([Buffer.from([2]), random.bytes(32)]);
        const seen = new Set();
        while (seen.size < 4) {
            const key = keyFrom(Identities, () => {
                const k = Buffer.from(base);
                k[p] = random.below(256);
                if (p < 32) random.bytes(32 - p).copy(k, p + 1);
                return k;
            });
            if (!seen.has(key)) {
                seen.add(key);
                positions.push(key);
            }
        }
    }
    await record("key order, keys that first differ at each byte of x", { delegates: named(positions.map((publicKey) => ({ publicKey, voteBalance: "1000" }))), maxDelegates: 53 });

    return out;
}

// The rounds up to 60,000,000 whose shuffle of 53 delegates rejects one draw.
const REJECTION_ROUNDS = [1054879, 1125219, 6447578, 9918318, 11922839, 14530287, 48658113, 51036157, 57852255, 58685437];

async function shuffle({ rec, must, shared }) {
    const H = shared.height;
    const out = [];
    const record = async (name, input) => out.push({ ...must(await rec("state.shuffle", H, input)), name });
    for (let count = 1; count <= 53; count++) {
        await record(`rounds 1 to 10,000, ${count} delegates`, { from: 1, to: 10000, count });
    }
    for (const count of [1, 2, 3, 52, 53]) {
        for (let round = 1; round <= 100; round++) {
            await record(`round ${round}, ${count} delegates`, { round, count });
        }
    }
    for (const round of REJECTION_ROUNDS) {
        await record(`round ${round}, 53 delegates, a rejected draw`, { round, count: 53 });
    }
    await record("round 1, no delegates", { round: 1, count: 0 });
    await record("round 7, 2 delegates", { round: 7, count: 2 });
    await record("round 7, 3 delegates", { round: 7, count: 3 });
    await record("round 0, 53 delegates", { round: 0, count: 53 });
    await record("round 2^53 - 1, 53 delegates", { round: Number.MAX_SAFE_INTEGER, count: 53 });
    await record("round 4294967296, 53 delegates", { round: 4294967296, count: 53 });
    await record("round 1, 200 delegates", { round: 1, count: 200 });
    return out;
}

async function voteAmounts({ rec, must, shared }) {
    const H = shared.height;
    const random = stream("heartwood V15 vote amounts");
    const out = [];
    const record = async (name, input) => out.push({ ...must(await rec("wallet.voteAmounts", H, input)), name });

    /** k basis points that add up to 10,000, each at least 1, in random order. */
    const split = (k) => {
        const cuts = new Set();
        while (cuts.size < k - 1) cuts.add(1 + random.below(9999));
        const points = [0, ...[...cuts].sort((a, b) => a - b), 10000];
        return points.slice(1).map((p, i) => p - points[i]);
    };
    const names = (k) => {
        const all = Array.from({ length: 53 }, (_, i) => `genesis_${i + 1}`);
        for (let i = all.length - 1; i > 0; i--) {
            const j = random.below(i + 1);
            [all[i], all[j]] = [all[j], all[i]];
        }
        return all.slice(0, k);
    };

    const edges = ["0", "1", "2", "53", "101", "9999", "10000", "10001", "9223372036854775806", "9223372036854775807", "9223372036854775808", "18446744073709551615"];
    for (let k = 1; k <= 53; k++) {
        const points = split(k);
        const votes = names(k).map((name, i) => [name, points[i]]);
        for (const balance of [edges[random.below(edges.length)], random.u64(), String(random.below(1000000))]) {
            await record(`${k} entries, balance ${balance}`, { balance, votes });
        }
    }
    for (const balance of edges) {
        await record(`an even three-way split, balance ${balance}`, { balance, votes: [["c", 3334], ["a", 3333], ["b", 3333]] });
        await record(`one entry, balance ${balance}`, { balance, votes: [["genesis_1", 10000]] });
        await record(`53 entries of 188 and 189, balance ${balance}`, { balance, votes: Array.from({ length: 53 }, (_, i) => [`genesis_${i + 1}`, i < 36 ? 189 : 188]) });
    }
    await record("the remainder in map order: a10 before a9", { balance: "101", votes: [["a10", 5000], ["a9", 5000]] });
    await record("the remainder in map order: a9 before a10", { balance: "101", votes: [["a9", 5000], ["a10", 5000]] });
    await record("the remainder in map order: small shares first", { balance: "10007", votes: [["x", 1], ["y", 2], ["z", 9997]] });
    await record("no votes", { balance: "1000", votes: [] });
    return out;
}

module.exports = { ranking, shuffle, voteAmounts };
