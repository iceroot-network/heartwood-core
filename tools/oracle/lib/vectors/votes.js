"use strict";
// V07 vote rules: all 9,999 two-way splits of 100 in cents (compact: one record per name pair),
// the vote schema on single cases, sortVotes orders, and the 1,024-byte limit of the vote asset.

const { SENDER } = require("./constants");

/** A name of exactly `length` characters that matches the username pattern. */
function nameOf(index, length) {
    const stem = `v${String(index).padStart(2, "0")}`;
    return (stem + "abcdefghijklmnopqrstuvwxyz").slice(0, length);
}

module.exports = async function votes({ rec, must, craft, shared }) {
    const out = [];
    const H = shared.height;

    for (const names of [
        ["a", "b"],
        ["b", "a"],
        ["genesis_1", "genesis_53"],
    ]) {
        out.push({ ...must(await rec("vote.splits", H, { names, from: 1, to: 9999 })), name: names.join(" / ") });
    }

    for (const [name, votes] of [
        ["50 + 49.99", { a: 50, b: 49.99 }],
        ["50 + 50.01", { a: 50, b: 50.01 }],
        ["100", { a: 100 }],
        ["empty", {}],
        ["33.33 + 33.33 + 33.34", { a: 33.33, b: 33.33, c: 33.34 }],
        ["three thirds rounded down", { a: 33.33, b: 33.33, c: 33.33 }],
        ["100.01", { a: 100.01 }],
        ["0 + 100", { a: 0, b: 100 }],
        ["0.005 + 99.995", { a: 0.005, b: 99.995 }],
        ["0.01 + 99.99", { a: 0.01, b: 99.99 }],
        ["18.1 + 81.9 (a float sum of 99.99999999999999)", { a: 18.1, b: 81.9 }],
        ["uppercase name", { A: 100 }],
        ["uppercase and valid names", { A: 50, b: 50 }],
        ["leading underscore", { _a: 100 }],
        ["21-character name", { abcdefghijklmnopqrstu: 100 }],
        ["20-character name", { abcdefghijklmnopqrst: 100 }],
        ["digits only", { 123: 100 }],
        ["text value", { a: "100" }],
        ["null value", { a: null }],
        ["53 names", Object.fromEntries(Array.from({ length: 53 }, (_, i) => [`genesis_${i + 1}`, i < 36 ? 1.89 : 1.88]))],
    ]) {
        out.push({ ...(await rec("vote.validate", H, { votes })), name });
    }

    for (const [name, votes] of [
        ["equal weights, bytewise names", { b2: 25, a9: 25, b10: 25, a10: 25 }],
        ["weights first", { a: 10, b: 60, c: 30 }],
        ["weights, then names", { c: 40, b: 20, a: 40 }],
        ["punctuation and digits", { "a.b": 25, a_b: 25, "a!b": 25, a1b: 25 }],
        ["prefixes", { abc: 50, ab: 25, a: 25 }],
        ["one", { genesis_7: 100 }],
        ["empty", {}],
    ]) {
        out.push({ ...must(await rec("vote.sort", H, { votes })), name });
    }

    // The vote asset: 1 + sum(3 + name bytes) at most 1,024. 44 names of 20 characters and one of 8
    // make exactly 1,024 bytes; one of 9 makes 1,025, which the builder refuses and the decoder too.
    const names = Array.from({ length: 44 }, (_, i) => nameOf(i, 20));
    const split = (last) => {
        const v = Object.fromEntries(names.map((n) => [n, 2.22]));
        v[last] = 2.32;
        return v;
    };
    const vote1024 = split(nameOf(44, 8));
    const vote1025 = split(nameOf(44, 9));
    const at = { ...(await rec("tx.build", H, { builder: "vote", calls: [["votesAsset", vote1024], ["nonce", "700"]], sign: SENDER })), name: "vote asset of 1,024 bytes" };
    out.push(at);
    out.push({ ...(await rec("tx.build", H, { builder: "vote", calls: [["votesAsset", vote1025], ["nonce", "701"]], sign: SENDER })), name: "vote asset of 1,025 bytes" });
    if (at.error) throw new Error(`the 1,024-byte vote was refused: ${at.error.message}`);
    out.push({ ...(await rec("tx.fromBytes", H, { hex: at.output.hex })), name: "vote asset of 1,024 bytes" });
    const entries1025 = Object.entries(vote1025).map(([n, v]) => [n, Math.round(v * 100)]);
    const hex1025 = craft.withAsset(at.output.hex, craft.voteAsset(entries1025), SENDER);
    out.push({ ...(await rec("tx.fromBytes", H, { hex: hex1025 })), name: "vote asset of 1,025 bytes" });
    return out;
};
