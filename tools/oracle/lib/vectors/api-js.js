"use strict";
// The records of kinds api.walletsLike and js.localeCompare in V34.
//
// js.localeCompare: pairs over the alphabet of usernames ([a-z0-9!@$&_.]) and over Base58 (the
// addresses' alphabet), digit runs with and without leading zeros, and pairs whose primary
// weights are equal (the same letters in another case, the same number with other zeros).
//
// api.walletsLike: wallet sets searched by username prefix and by address and public key prefix,
// and the sort of the matches: three-wallet cycles (the reference's comparator is not a total
// order) in every order of the fixture, sets of 1 to 100 matches and more than 100 (the search
// keeps the first 100), sorted, reversed and shuffled by address, with runs that gallop, the
// 20 and 21 character boundary, and mixed case.
// Inputs come from a fixed SHA-256 counter stream, so every run writes the same records.

const crypto = require("crypto");

const USERNAME = "abcdefghijklmnopqrstuvwxyz0123456789!@$&_.";
const BASE58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
const NETWORK = 90;

function stream(label) {
    let counter = 0;
    let block = Buffer.alloc(0);
    const below = (n) => {
        if (block.length < 4) {
            const c = Buffer.alloc(8);
            c.writeBigUInt64BE(BigInt(counter++));
            block = Buffer.concat([block, crypto.createHash("sha256").update(label).update(c).digest()]);
        }
        const value = block.readUInt32BE(0);
        block = block.subarray(4);
        return value % n;
    };
    const pick = (text) => text[below(text.length)];
    const text = (alphabet, length) => Array.from({ length }, () => pick(alphabet)).join("");
    const shuffle = (list) => {
        const out = list.slice();
        for (let i = out.length - 1; i > 0; i--) {
            const j = below(i + 1);
            [out[i], out[j]] = [out[j], out[i]];
        }
        return out;
    };
    return { below, pick, text, shuffle };
}

const compare = (a, b) => a.localeCompare(b, "en", { numeric: true });

async function collation({ rec, must, shared }) {
    const out = [];
    const H = shared.height;
    const random = stream("heartwood V34 localeCompare");
    const add = async (a, b, note) => out.push({ ...must(await rec("js.localeCompare", H, { a, b })), name: `localeCompare: ${note}: ${JSON.stringify(a)} ${JSON.stringify(b)}` });

    // Every pair of the alphabet's characters, both orders.
    const singles = Array.from(USERNAME + "ABCXYZ");
    for (let i = 0; i < singles.length; i++) {
        for (const j of [i, (i * 7 + 3) % singles.length, (i * 13 + 11) % singles.length]) await add(singles[i], singles[j], "characters");
    }
    // Digit runs: values, lengths, leading zeros, a run against a letter or a mark.
    const runs = ["0", "00", "1", "01", "001", "9", "10", "010", "11", "19", "20", "100", "099", "1000", "12345678901234567890", "012345678901234567890", "99999999999999999999"];
    for (const a of runs) for (const b of runs) if (a <= b || a.length !== b.length) await add(`x${a}`, `x${b}`, "digit runs");
    for (const [a, b] of [["a1", "a01"], ["a1b", "a01b"], ["a10", "a9"], ["a9b", "a10b"], ["1a", "a1"], ["_1", "1"], ["1_", "1a"], ["a1b2", "a01b02"], ["a1b2", "a1b3"], ["a001b", "a1c"], ["0a", "00a"], ["9", "a"], ["a.1", "a.01"], ["a1.", "a01_"], ["a99", "a100"], ["", "0"], ["", "a"], ["a", "a0"], ["a0", "a00"], ["a00", "a1"]])
        await add(a, b, "digit runs");
    // Equal primary weights: only the case, or only the zeros, differ.
    for (const [a, b] of [["a", "A"], ["ab", "aB"], ["Ab", "aB"], ["AB", "ab"], ["aBc1", "AbC1"], ["a1B", "A1b"], ["a01", "A1"], ["A01b", "a1B"], ["zZ", "Zz"], ["aa", "aA"], ["aA", "Aa"], ["Aa", "aA"], ["a_B", "A_b"]])
        await add(a, b, "equal primary");
    // The marks against each other, digits and letters.
    for (const a of ["_", "!", ".", "@", "&", "$"]) for (const b of ["_a", "!a", ".a", "@a", "&a", "$a", "a", "1", "_1", "a_", "a!"]) await add(a, b, "marks");
    for (const [a, b] of [["a_b", "ab"], ["a.b", "a_b"], ["ab", "a_b"], ["a$", "a&"], ["a@b", "a!b"], ["a_", "a"], ["_a", "a"], ["a.1", "a.a"]]) await add(a, b, "marks");
    // Random usernames (short, so that prefixes and ties are common) and Base58 addresses.
    for (let i = 0; i < 70; i++) await add(random.text("ab_.1230A", 1 + random.below(6)), random.text("ab_.1230A", 1 + random.below(6)), "small alphabet");
    for (let i = 0; i < 50; i++) await add(random.text(USERNAME, 1 + random.below(20)), random.text(USERNAME, 1 + random.below(20)), "usernames");
    for (let i = 0; i < 50; i++) await add(`D${random.text(BASE58, 33)}`, `D${random.text(BASE58, 33)}`, "addresses");
    for (let i = 0; i < 40; i++) {
        const shared = random.text(BASE58, 4 + random.below(20));
        await add(`D${shared}${random.text(BASE58, 8)}`, `D${shared}${random.text(BASE58, 8)}`, "addresses with a common prefix");
    }
    for (let i = 0; i < 20; i++) await add(`d${random.text("0123456789", 1 + random.below(8))}x`, `d${random.text("0123456789", 1 + random.below(8))}x`, "digit strings");
    return out;
}

async function search({ solar, rec, must, shared }) {
    const { Identities } = solar.lib;
    const out = [];
    const H = shared.height;
    const random = stream("heartwood V34 walletsLike");

    // A pool of wallets (addresses and keys are the slow part), taken in turn by every fixture.
    const pool = [];
    const wallet = (extra = {}) => {
        const i = pool.length;
        const passphrase = `v34 wallet ${i}`;
        const pk = Identities.Keys.fromPassphrase(passphrase).publicKey.secp256k1;
        pool.push({ a: Identities.Address.fromPublicKey(pk, NETWORK), pk });
        return { ...pool[i], bal: String(1000 + i), n: "1", ...extra };
    };
    const backing = Array.from({ length: 130 }, () => wallet());
    let used = 0;
    const next = (extra = {}) => ({ ...backing[used++ % backing.length], ...extra });

    const add = async (name, fixture, input, ranks) => {
        const args = { fixture, input };
        if (ranks && ranks.length) args.ranks = ranks;
        out.push({ ...must(await rec("api.walletsLike", H, args)), name: `walletsLike: ${name}` });
    };
    // A delegate with the reference's usual values; resigned: 0 temporary, 1 permanent.
    const delegate = (username, extra = {}) => ({ ...next(), u: username, pb: 2, ff: "10", fr: "20", bf: "1", dn: "0", ...extra });
    const addressOrder = (list) => list.slice().sort((x, y) => compare(x.a, y.a));

    // Cycles: two delegates in office (ordered by name) and a resigned one, whose comparisons with
    // both are by address. With the addresses ordered as low < middle < high, the names in the
    // order high, low give high < low (name), low < resigned (address), resigned < high (address).
    for (let cycle = 0; cycle < 6; cycle++) {
        used = cycle * 3;
        const [low, middle, high] = addressOrder([next(), next(), next()]);
        const names = [`cyc${cycle}a`, `cyc${cycle}b`, `cyc${cycle}${random.text("0123456789_.", 3)}`].sort(compare);
        const members = [
            { ...high, u: names[0] },
            { ...low, u: names[1] },
            { ...middle, u: names[2], rs: cycle % 2 },
        ];
        const orders = [[0, 1, 2], [0, 2, 1], [1, 0, 2], [1, 2, 0], [2, 0, 1], [2, 1, 0]];
        for (const order of orders) await add(`cycle ${cycle}, order ${order.join("")}`, order.map((i) => members[i]), cycle % 3 === 0 ? "CYC" : "cyc");
        // The other direction of the cycle: the resigned one is the highest address.
        const flipped = [{ ...low, u: names[0] }, { ...high, u: names[1] }, { ...middle, u: names[2], rs: 1 - (cycle % 2) }];
        for (const order of [[0, 1, 2], [2, 1, 0], [1, 2, 0]]) await add(`cycle ${cycle}, flipped, order ${order.join("")}`, order.map((i) => flipped[i]), `cyc${cycle}`);
    }

    // Sets of every size class: names in order, in reverse order, shuffled; resigned delegates
    // scattered or in blocks (blocks make runs of the address order, which gallop when merged).
    const names = (n, style) => {
        const list = Array.from({ length: n }, (_, i) => `m${style === "digits" ? i : style === "marks" ? `${random.pick("_.!@$&")}${i}` : String(i).padStart(3, "0")}`);
        return Array.from(new Set(list));
    };
    const shapes = ["sorted", "reversed", "shuffled"];
    const sizes = [1, 2, 3, 4, 7, 8, 9, 16, 31, 32, 33, 47, 63, 64, 65, 66, 90, 99, 100, 101, 128];
    for (const n of sizes) {
        for (const shape of n === 64 || n === 65 || n === 100 ? shapes : [shapes[n % 3]]) {
            used = 0;
            const style = ["padded", "digits", "marks"][n % 3];
            const resignedAt = (i) => (n % 2 === 0 ? i % 3 === 0 : Math.floor(i / 8) % 2 === 1);
            let list = names(n, style).map((username, i) => delegate(username, resignedAt(i) ? { rs: i % 2 } : {}));
            if (shape === "reversed") list = list.reverse();
            if (shape === "shuffled") list = random.shuffle(list);
            const ranks = list.filter((w, i) => i % 5 === 0 && w.rs === undefined).map((w, i) => [w.a, i + 1]);
            await add(`${n} delegates, ${shape}, ${style} names`, list, n % 4 === 1 ? "M" : "m", ranks);
        }
    }
    // Wallets in the order of their addresses (ascending, descending) with every delegate resigned
    // but a few: the comparator is by address nearly everywhere.
    for (const [shape, n] of [["ascending", 70], ["descending", 70], ["ascending", 100], ["descending", 33]]) {
        used = 0;
        let list = addressOrder(Array.from({ length: n }, (_, i) => next())).map((w, i) => ({ ...w, u: `r${String(i * 13 % n).padStart(3, "0")}`, pb: 1, rs: i % 11 === 0 ? undefined : i % 2 })).map((w) => (w.rs === undefined ? (delete w.rs, w) : w));
        if (shape === "descending") list = list.reverse();
        await add(`${n} delegates by address, ${shape}`, list, "r");
    }
    // Long runs to merge: ascending names in a few blocks, the blocks in another order.
    for (const blocks of [[40, 40, 40], [30, 50, 30, 20], [10, 90], [64, 64]]) {
        used = 0;
        let counter = 0;
        const parts = blocks.map((size, b) => Array.from({ length: size }, () => delegate(`g${String(counter++).padStart(3, "0")}`)));
        await add(`blocks ${blocks.join("+")} in order`, parts.flat(), "g");
        await add(`blocks ${blocks.join("+")} reversed`, parts.slice().reverse().flat(), "g");
        await add(`blocks ${blocks.join("+")} interleaved`, parts.flatMap((part, b) => part.map((w, i) => ({ w, k: i * blocks.length + b }))).sort((x, y) => x.k - y.k).map((x) => x.w), "g");
    }

    // The boundary: a username input of 20 characters is a username prefix, of 21 an address or key
    // prefix; usernames and addresses are lower-cased in the comparison, the input is too.
    used = 0;
    const longName = "a".repeat(20);
    const set = [
        delegate(longName),
        delegate(`${"a".repeat(19)}b`),
        delegate(`${"a".repeat(10)}`),
        delegate("aab", { rs: 1 }),
        next({ bal: "5" }),
        next({ bal: "6", v: [[longName, 6000], ["aab", 4000]] }),
        next(),
    ];
    const [d1, d2, d3, d4, plain, voter, other] = set;
    await add("input of 20 characters, a username", set, longName);
    await add("input of 20 characters, upper case", set, longName.toUpperCase());
    await add("input of 19 characters", set, "a".repeat(19));
    await add("input of 21 characters, no address", set, "a".repeat(21));
    await add("input of 20 characters, an address prefix", set, plain.a.slice(0, 20));
    await add("input of 21 characters, an address prefix", set, plain.a.slice(0, 21));
    await add("input of 21 characters, an address prefix in upper case", set, plain.a.slice(0, 21).toUpperCase());
    await add("input of 21 characters, an address prefix in lower case", set, plain.a.slice(0, 21).toLowerCase());
    await add("input of 22 characters, a delegate's address", set, d1.a.slice(0, 22));
    await add("the whole address", set, plain.a);
    await add("the whole address, upper case", set, plain.a.toUpperCase());
    await add("a public key prefix of 21 characters", set, plain.pk.slice(0, 21));
    await add("a public key prefix of 21 characters, upper case", set, plain.pk.slice(0, 21).toUpperCase());
    await add("a public key prefix of 20 characters", set, plain.pk.slice(0, 20));
    await add("a whole public key", set, d2.pk);
    await add("a voter's address", set, voter.a.slice(0, 30));
    await add("a voter's address prefix of 21 characters", set, voter.a.slice(0, 21));
    await add("the empty input", set, "");
    await add("no match", set, "zzz");
    await add("a mark", set, "_");
    await add("a delegate whose name has a resigned twin", set, "aa");
    await add("mixed case", set, "AaAaAaAaAa");
    await add("mixed case address", set, `${voter.a.slice(0, 12).toUpperCase()}${voter.a.slice(12, 30).toLowerCase()}`);
    await add("a 66 character input", set, voter.pk);
    await add("a non-ASCII input", set, "é");
    await add("a 20 unit input of 10 astral characters", set, "\u{1F600}".repeat(10));
    await add("a 22 unit input of 11 astral characters", set, "\u{1F600}".repeat(11));

    // Votes: per delegate voted for, fractions of percent, split votes, delegates named by numbers
    // (JavaScript orders an object's index-like keys first, ascending).
    used = 0;
    const named = ["20", "3", "abc", "007", "100", "x1", "4294967294", "4294967295", "1.5"].map((username) => delegate(username));
    const voters = [
        next({ v: [["20", 3333], ["3", 3333], ["abc", 3334]] }),
        next({ v: [["x1", 2500], ["100", 2500], ["007", 2500], ["3", 2500]] }),
        next({ v: [["4294967295", 5000], ["4294967294", 5000]] }),
        next({ v: [["1.5", 10000]] }),
        next({ v: [["abc", 1], ["3", 9999]], bal: "7" }),
    ];
    for (const [i, v] of voters.entries()) await add(`votes of voter ${i}`, [...named, ...voters], v.a.slice(0, 21 + i));
    await add("delegates named by numbers", [...voters, ...named], "");
    await add("delegates named by numbers, prefix 0", [...voters, ...named], "0");
    // Wallets that are delegates and vote, resigned and temporary.
    used = 0;
    const ds = [delegate("vd1", { v: [["vd2", 10000]] }), delegate("vd2", { rs: 0 }), delegate("vd3", { rs: 1, spk: pool[5].pk, v: [["vd1", 5000], ["vd2", 5000]] })];
    await add("delegates that vote", ds, "vd", [[ds[0].a, 1]]);
    await add("delegates that vote, reversed", ds.slice().reverse(), "VD");
    return out;
}

module.exports = async function apiJsVectors(context) {
    return [...(await collation(context)), ...(await search(context))];
};
