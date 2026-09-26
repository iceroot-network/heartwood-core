"use strict";
// V02 transactions: the six types Heartwood keeps (1/1, 1/2, 1/6, 1/7, 2/0, 2/2), version 3,
// standard header, BIP340 with fixed aux. Each built case is recorded three ways: tx.build (bytes,
// id, JSON and the strict decode verdict), tx.fromBytes (the bytes back to JSON) and tx.fromJson
// (the JSON back to bytes). Refusals: memo over 255 bytes, 0 and 257 recipients.

const { DOUBLE_SENDER, SECOND, SENDER } = require("./constants");

/** Basis points split as evenly as possible over n entries, the first ones taking the remainder. */
function evenSplit(n) {
    const base = Math.floor(10000 / n);
    const extra = 10000 - base * n;
    return Array.from({ length: n }, (_, i) => base + (i < extra ? 1 : 0));
}

function votesOf(names, basisPoints) {
    return Object.fromEntries(names.map((name, i) => [name, basisPoints[i] / 100]));
}

module.exports = async function transactions({ solar, rec, must, craft, shared }) {
    const { Identities } = solar.lib;
    const H = shared.height;
    const out = [];

    const recipients = [];
    for (let i = 0; i < 257; i++) recipients.push(Identities.Address.fromPassphrase(`recipient ${i}`, 90));
    shared.recipients = recipients;

    const tx = (name, builder, calls, extra = {}) => ({ name, input: { builder, calls, sign: SENDER, ...extra } });
    const transfers = (n, from = 0) => recipients.slice(from, from + n).map((r, i) => ["addTransfer", r, String(1000 + i)]);
    let nonce = 0;
    const next = () => ["nonce", String(++nonce)];

    const cases = [
        tx("transfer-1", "transfer", [["recipientId", recipients[1]], ["amount", "100000000"], next()]),
        tx("transfer-1-memo", "transfer", [["recipientId", recipients[0]], ["amount", "123456789"], next(), ["memo", "hello"]]),
        tx("transfer-memo-empty", "transfer", [["recipientId", recipients[0]], ["amount", "1"], next(), ["memo", ""]]),
        tx("transfer-memo-1", "transfer", [["recipientId", recipients[0]], ["amount", "1"], next(), ["memo", "x"]]),
        // 255 bytes with a 2-, 3- and 4-byte character at the end, and 256 bytes (refused).
        tx("transfer-memo-255", "transfer", [["recipientId", recipients[4]], ["amount", "1"], next(), ["memo", "m".repeat(253) + "é"]]),
        tx("transfer-memo-255-3byte", "transfer", [["recipientId", recipients[4]], ["amount", "1"], next(), ["memo", "m".repeat(252) + "☃"]]),
        tx("transfer-memo-255-4byte", "transfer", [["recipientId", recipients[4]], ["amount", "1"], next(), ["memo", "m".repeat(251) + "😀"]]),
        tx("transfer-memo-256", "transfer", [["recipientId", recipients[4]], ["amount", "1"], next(), ["memo", "m".repeat(254) + "é"]]),
        tx("transfer-2", "transfer", [...transfers(2, 2), next()]),
        tx("transfer-255", "transfer", [...transfers(255), next(), ["fee", "100000000"]]),
        tx("transfer-256", "transfer", [...transfers(256), next(), ["fee", "100000000"], ["memo", "multipayment memo ☃ utf8"]]),
        tx("transfer-257", "transfer", [...transfers(257), next(), ["fee", "100000000"]]),
        tx("transfer-0", "transfer", [next()]),
        tx("second-signature", "secondSignature", [["signatureAsset", SECOND], next()]),
        tx("delegate-registration", "delegateRegistration", [["usernameAsset", "difftest_del1"], next()]),
        tx("resignation-0", "delegateResignation", [["resignationTypeAsset", 0], next()]),
        tx("resignation-1", "delegateResignation", [["resignationTypeAsset", 1], next()]),
        tx("resignation-2", "delegateResignation", [["resignationTypeAsset", 2], next()]),
        tx("burn", "burn", [["amount", "2000000000"], next()]),
        tx("burn-memo", "burn", [["amount", "2000000"], next(), ["memo", "burn memo"]]),
        tx("vote-2-uneven", "vote", [["votesAsset", { difftest_del1: 60.5, gym: 39.5 }], next()]),
        tx("unvote", "vote", [["votesAsset", {}], next()]),
        tx("vote-array-3", "vote", [["votesAsset", ["a", "b", "c"]], next()]),
    ];
    // Votes of 1 to 53 entries.
    for (let n = 1; n <= 53; n++) {
        const names = Array.from({ length: n }, (_, i) => `genesis_${i + 1}`);
        cases.push(tx(`vote-${n}`, "vote", [["votesAsset", votesOf(names, evenSplit(n))], next()]));
    }
    // Doubly signed variants, one per type that can carry a second signature.
    const double = (name, builder, calls) => ({ name, input: { builder, calls, sign: DOUBLE_SENDER, secondSign: SECOND } });
    let doubleNonce = 0;
    const nextDouble = () => ["nonce", String(++doubleNonce)];
    cases.push(
        double("transfer-second-signed", "transfer", [["recipientId", recipients[5]], ["amount", "5"], nextDouble()]),
        double("transfer-2-second-signed", "transfer", [...transfers(2, 6), nextDouble(), ["memo", "second signed"]]),
        double("delegate-registration-second-signed", "delegateRegistration", [["usernameAsset", "difftest_del2"], nextDouble()]),
        double("resignation-0-second-signed", "delegateResignation", [["resignationTypeAsset", 0], nextDouble()]),
        double("resignation-2-second-signed", "delegateResignation", [["resignationTypeAsset", 2], nextDouble()]),
        double("burn-second-signed", "burn", [["amount", "3000000"], nextDouble()]),
        double("vote-second-signed", "vote", [["votesAsset", { difftest_del2: 100 }], nextDouble()]),
    );

    const built = {};
    for (const c of cases) {
        const r = { ...(await rec("tx.build", H, c.input)), name: c.name };
        out.push(r);
        if (r.error) continue;
        built[c.name] = r.output;
        out.push({ ...must(await rec("tx.fromBytes", H, { hex: r.output.hex })), name: c.name });
        out.push({ ...(await rec("tx.fromJson", H, { json: r.output.json })), name: c.name });
    }
    shared.tx = built;

    // Refusals the builders do not produce as bytes: a JSON memo of 256 bytes, and 0 or 257
    // recipients signed and serialised with the reference's Signer and Serialiser.
    const memo256 = { ...built["transfer-memo-255"].json, memo: "m".repeat(254) + "é" };
    out.push({ ...(await rec("tx.fromJson", H, { json: memo256 })), name: "transfer-memo-256" });
    for (const [name, count] of [
        ["transfer-257", 257],
        ["transfer-0", 0],
    ]) {
        const data = craft.builderData("transfer", [["nonce", "900"], ["fee", "100000000"]], SENDER);
        data.asset = { transfers: recipients.slice(0, count).map((recipientId, i) => ({ recipientId, amount: solar.lib.Utils.BigNumber.make(1000 + i) })) };
        const hex = craft.signAndSerialise(data, SENDER).toString("hex");
        out.push({ ...(await rec("tx.fromBytes", H, { hex })), name });
        out.push({ ...(await rec("tx.fromBytes", H, { hex, strict: false })), name: `${name}-non-strict` });
    }
    return out;
};
