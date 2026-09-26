"use strict";
// V05 blocks: BlockFactory.make with fixed aux for 0, 1 and 150 transactions and for the largest
// block the devnet rules allow, each decoded and re-encoded; the payload-size rule at its boundary;
// too many transactions; payloadLength 32n - 1, 32n + 1 and 0 with transactions; and the block
// signature scheme under a milestone with bip340 false. All timestamps are in the past.

const { FIXED_AUX } = require("../ops");
const { DOUBLE_SENDER, GENERATOR, SECOND, SENDER } = require("./constants");
const { milestonesText } = require("./milestones");

module.exports = async function blocks({ rec, must, shared }) {
    const out = [];
    const aux = FIXED_AUX.toString("hex");
    const previousBlock = "ab".repeat(32);
    const H = shared.height;

    const make = async (name, height, transactions, extra = {}) => {
        const input = { height, timestamp: 8 * (height - 1), previousBlock, reward: "180000000", passphrase: GENERATOR, aux, transactions, ...extra };
        const r = { ...(await rec("block.make", height, input)), name };
        out.push(r);
        return r;
    };
    const decode = async (name, made, extra = {}) => {
        if (!made.output) return;
        const hex = made.output.hex;
        if (hex === undefined) throw new Error(`${name}: a summarised block cannot be decoded from its record`);
        out.push({ ...(await rec("block.fromBytes", made.input.height, { hex, ...extra })), name });
    };

    // Transactions for the blocks: one sender, consecutive nonces.
    const build = async (input) => must(await rec("tx.build", H, input)).output.hex;
    const small = [];
    for (let i = 0; i < 151; i++) {
        small.push(await build({ builder: "transfer", calls: [["recipientId", shared.recipients[i]], ["amount", String(100 + i)], ["nonce", String(i + 1)]], sign: SENDER }));
    }
    const kept = ["transfer-1", "transfer-2", "second-signature", "delegate-registration", "resignation-1", "burn", "vote-2", "vote-53", "transfer-second-signed"];

    const empty = await make("empty", 2, []);
    await decode("empty", empty);
    const one = await make("one transfer", 3, [shared.tx["transfer-1-memo"].hex]);
    await decode("one transfer", one);
    const mixed = await make("the kept types", 4, kept.map((n) => shared.tx[n].hex));
    await decode("the kept types", mixed);
    const full = await make("150 transactions", 5, small.slice(0, 150));
    await decode("150 transactions", full);
    await make("151 transactions", 6, small);

    // The largest block: 150 transfers of 256 recipients with a 255-byte memo, second-signed.
    // Its bytes and JSON are recorded as SHA-256 and length (block.make with summary); the decode
    // record carries the full bytes.
    const largest = [];
    const memo = "m".repeat(253) + "é";
    for (let i = 0; i < 150; i++) {
        const calls = shared.recipients.slice(0, 256).map((r, j) => ["addTransfer", r, String(1 + i * 256 + j)]);
        calls.push(["nonce", String(i + 1)], ["fee", "100000000"], ["memo", memo]);
        largest.push(await build({ builder: "transfer", calls, sign: DOUBLE_SENDER, secondSign: SECOND }));
    }
    const big = await make("largest block", 7, largest, { summary: true });
    if (big.error) throw new Error(`the largest block was refused: ${big.error.message}`);
    const fullHex = must(await rec("block.make", 7, { ...big.input, summary: false })).output.hex;
    out.push({ ...(await rec("block.fromBytes", 7, { hex: fullHex, summary: true })), name: "largest block" });

    // The payload-size rule at its boundary: a block of two transfers under maxTransactions 2 and
    // maxPayload equal to its size (accepted) or one byte less (refused). The size is what
    // Serialiser.size counts: the header, the signature, and 4 + length per transaction. The
    // serialiser gives each transaction a buffer of floor(maxPayload / maxTransactions) x 2 bytes,
    // so a small maxPayload with the default maxTransactions refuses every transaction.
    const pair = [small[0], small[1]];
    const payloadSize = 141 + 64 + pair.reduce((n, hex) => n + 4 + hex.length / 2, 0);
    for (const [name, maxTransactions, maxPayload] of [
        ["maxPayload equal to the size", 2, payloadSize],
        ["maxPayload one byte under the size", 2, payloadSize - 1],
        ["serialiser buffer smaller than a transaction", 150, payloadSize],
    ]) {
        const milestones = milestonesText({ block: `{"version":0,"maxTransactions":${maxTransactions},"maxPayload":${maxPayload}}` });
        await make(name, 4, pair, { milestones });
        out.push({ ...(await rec("tx.fromBytes", 4, { hex: pair[0], milestones })), name });
    }

    // payloadLength must be 32 x numberOfTransactions.
    const three = kept.slice(0, 3).map((n) => shared.tx[n].hex);
    for (const [name, payloadLength] of [
        ["payloadLength 32n - 1", 32 * 3 - 1],
        ["payloadLength 32n + 1", 32 * 3 + 1],
        ["payloadLength 0", 0],
    ]) {
        const made = await make(name, 8, three, { overrides: { payloadLength } });
        await decode(name, made);
    }
    await make("empty, payloadLength 32", 8, [], { overrides: { payloadLength: 32 } });
    await make("block version 1", 8, three, { overrides: { version: 1 } });

    // Block signatures are BIP340 whatever the milestone says: under bip340 false a BIP340 block is
    // accepted and a legacy-signed one refused.
    const legacyMilestones = milestonesText({ bip340: "false" });
    for (const scheme of ["bip340", "legacy"]) {
        const made = await make(`${scheme}-signed, bip340 false`, 9, three, { scheme, milestones: legacyMilestones });
        await decode(`${scheme}-signed, bip340 false`, made, { milestones: legacyMilestones });
    }
    await make("legacy-signed, devnet", 9, three, { scheme: "legacy" });
    return out;
};
