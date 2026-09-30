"use strict";
// Regenerates v8-sort.json and collate.json from Node's own Array.prototype.sort and
// String.prototype.localeCompare. Run with the Node 18 the reference runs on:
//   node generate.js
// The cases are deterministic (a fixed generator), so a rerun writes the same files.

const fs = require("fs");
const path = require("path");

const seedState = { s: 0x1234abcd };
const random = () => {
    // mulberry32
    let t = (seedState.s += 0x6d2b79f5);
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
};
const int = (n) => Math.floor(random() * n);
const shuffle = (list) => {
    for (let i = list.length - 1; i > 0; i--) {
        const j = int(i + 1);
        [list[i], list[j]] = [list[j], list[i]];
    }
    return list;
};

// The comparators, each a function of two labels and a seed: the Rust test has the same four.
const mix = (a, b, seed) => {
    let h = Math.imul(a ^ seed, 0x9e3779b1) ^ Math.imul(b + 0x7f4a7c15, 0x85ebca6b);
    h ^= h >>> 15;
    h = Math.imul(h, 0x2c1b3c6d);
    h ^= h >>> 13;
    return h >>> 0;
};
const comparators = {
    // A total order.
    ascending: (a, b) => Math.sign(a - b),
    // Three-way ties and cycles.
    cycle: (a, b) => [0, -1, 1][(((a - b) % 3) + 3) % 3],
    // No structure at all: not transitive, not even antisymmetric.
    hash: (a, b, seed) => (mix(a, b, seed) % 3) - 1,
    // An order with a flipped answer now and then, so runs form and break.
    noisy: (a, b, seed) => (mix(a, b, seed) % 9 === 0 ? -Math.sign(a - b) : Math.sign(a - b)),
};

const labels = (n) => {
    const seen = new Set();
    while (seen.size < n) seen.add(int(100000));
    return [...seen];
};
const shapes = {
    sorted: (n) => labels(n).sort((a, b) => a - b),
    reversed: (n) => labels(n).sort((a, b) => b - a),
    random: (n) => labels(n),
    // Ascending blocks in a random order: merges gallop.
    blocks: (n) => {
        const all = labels(n).sort((a, b) => a - b);
        const parts = [];
        for (let at = 0; at < n; ) {
            const size = 1 + int(Math.max(1, n >> 2));
            parts.push(all.slice(at, at + size));
            at += size;
        }
        return shuffle(parts).flat();
    },
};
const sizes = [1, 2, 3, 4, 5, 8, 13, 31, 32, 33, 47, 63, 64, 65, 66, 99, 100, 101, 127, 128, 200, 257];
const cases = [];
for (const [kind, compare] of Object.entries(comparators)) {
    for (const [shape, make] of Object.entries(shapes)) {
        for (const n of [...sizes, ...Array.from({ length: 3 }, () => 1 + int(100))]) {
            const seed = int(1000000);
            const items = make(n);
            const sorted = items.slice().sort((a, b) => compare(a, b, seed));
            cases.push({ comparator: kind, shape, seed, items, sorted });
        }
    }
}
fs.writeFileSync(path.join(__dirname, "v8-sort.json"), JSON.stringify(cases) + "\n");

// Collation: pairs of strings over the username alphabet and the Base58 alphabet, with digit runs,
// leading zeros, equal-primary pairs (case, zeros) and shared prefixes.
const username = "abcdefghijklmnopqrstuvwxyz0123456789!@$&_.";
const base58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
const text = (alphabet, max) => Array.from({ length: 1 + int(max) }, () => alphabet[int(alphabet.length)]).join("");
const variant = (s) => {
    switch (int(6)) {
        case 0: return s.toUpperCase();
        case 1: return s + text(username, 2);
        case 2: return s.replace(/[0-9]+/, (d) => "0".repeat(1 + int(2)) + d);
        case 3: return s.slice(0, 1 + int(s.length));
        case 4: return s.replace(/[a-z]/, (c) => c.toUpperCase());
        default: return s;
    }
};
const pairs = [];
const add = (a, b) => pairs.push([a, b, a.localeCompare(b, "en", { numeric: true })]);
for (const digits of [["1", "01"], ["0", "00"], ["9", "10"], ["a9", "a10"], ["a09", "a9"], ["a1b", "a01b"], ["a", "A"], ["_", "a"], ["!", "_"], [".", "!"], ["@", "."], ["&", "@"], ["$", "&"], ["$", "0"], ["0", "a"], ["", "a"], ["ab", "aB"], ["a1", "A1"], ["a1", "a"], ["z", "0"]]) add(...digits);
for (let i = 0; i < 300; i++) {
    const a = text(username, 12);
    add(a, i % 3 === 0 ? variant(a) : text(username, 12));
}
for (let i = 0; i < 250; i++) {
    const a = text(base58, 34);
    add(a, i % 4 === 0 ? variant(a) : i % 4 === 1 ? a.slice(0, 5) + text(base58, 30) : text(base58, 34));
}
for (let i = 0; i < 150; i++) {
    const digits = () => (random() < 0.5 ? "0".repeat(int(3)) : "") + String(int(2000));
    add(text("ab", 2) + digits() + text("ab_", 2), text("ab", 2) + digits() + text("ab_", 2));
}
fs.writeFileSync(path.join(__dirname, "collate.json"), JSON.stringify(pairs) + "\n");
console.log(`v8-sort.json: ${cases.length} cases; collate.json: ${pairs.length} pairs`);
