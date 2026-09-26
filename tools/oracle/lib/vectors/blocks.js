"use strict";
// V05 blocks: BlockFactory.make with fixed aux for 0, 1 and 150 transactions and for the largest
// block the devnet rules allow, each decoded and re-encoded; the payload-size rule at its boundary;
// too many transactions; payloadLength 32n - 1, 32n + 1 and 0 with transactions; the block
// signature scheme under a milestone with bip340 false; and blocks as a node receives them from
// another node (block.receive), next to block.fromBytes, for blocks that carry transactions in a
// wire form that is not the canonical one. All timestamps are in the past.

const { FIXED_AUX } = require("../ops");
const { DOUBLE_SENDER, GENERATOR, SECOND, SENDER } = require("./constants");
const { milestonesText } = require("./milestones");

module.exports = async function blocks({ solar, rec, must, craft, shared }) {
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

    // A transaction whose network byte is 0: the block factory reads it without checks and writes
    // it again with the configured network byte.
    const zeroNetwork = Buffer.from(shared.tx["transfer-1"].hex, "hex");
    zeroNetwork[2] = 0;
    await make("a transaction with network byte 0", 9, [zeroNetwork.toString("hex")]);

    await receiving({ solar, rec, craft, shared, out, aux, previousBlock, madeBlocks: { empty, "the kept types": mixed } });
    return out;
};

/**
 * Blocks as a node receives them (block.receive), each next to block.fromBytes on the same bytes.
 * The receiving node serialises every transaction again from its decoded data, so the ids, the
 * payload hash and the size it checks are those of the canonical form: a vote name that the rules
 * remove changes the id, and a stray resignation byte or a repeated vote name counts in the size of
 * the wire form only. A memo that is not UTF-8 reads as replacement characters, which is the
 * canonical form of a memo that holds them.
 */
async function receiving({ solar, rec, craft, shared, out, aux, previousBlock, madeBlocks }) {
    const { Transactions } = solar.lib;
    const SIGNATURE = 64;
    const unsigned = (hex) => hex.slice(0, hex.length - 2 * SIGNATURE);
    // The id the strict decoder computes (over the decoded fields, before the rules remove names),
    // and the id of the canonical form (after the rules).
    const decodeId = (hex) => Transactions.TransactionFactory.fromBytes(Buffer.from(hex, "hex")).id;
    const canonicalId = (hex) => Transactions.Utils.getId(Transactions.TransactionFactory.fromBytes(Buffer.from(hex, "hex")).data);
    const both = async (name, height, hex, extra = {}) => {
        out.push({ ...(await rec("block.fromBytes", height, { hex, ...extra })), name });
        out.push({ ...(await rec("block.receive", height, { hex, ...extra })), name });
    };
    const block = (height, transactions, ids) =>
        craft.blockBytes({ height, timestamp: 8 * (height - 1), previousBlock, reward: "180000000", passphrase: GENERATOR, aux, transactions, ids });

    // Blocks the factory made: both paths agree.
    for (const [name, made] of Object.entries(madeBlocks)) {
        if (made.output) out.push({ ...(await rec("block.receive", made.input.height, { hex: made.output.hex })), name });
    }

    // Votes signed over their canonical form whose wire form carries a name that the rules remove.
    const vote = shared.tx["vote-1"].hex; // {genesis_1: 100}
    const canonicalVote = unsigned(vote);
    const extraName = [
        ["an uppercase name", "A"],
        ["an empty name", ""],
        ["a name that is not UTF-8", Buffer.from([0xff])],
        ["a name of digits only", "1"],
    ];
    for (const [label, name] of extraName) {
        const hex = craft.withAsset(vote, craft.voteAsset([["genesis_1", 10000], [name, 5000]]), SENDER, canonicalVote);
        await both(`vote with ${label}, payload hash over the decoded id`, 10, block(10, [hex], [decodeId(hex)]));
        await both(`vote with ${label}, payload hash over the canonical id`, 10, block(10, [hex], [canonicalId(hex)]));
    }

    // Forms that decode to the canonical id: a repeated vote name, a stray resignation byte, the
    // version byte 0.
    const repeated = craft.withAsset(vote, craft.voteAsset([["genesis_1", 10000], ["genesis_1", 10000]]), SENDER, canonicalVote);
    const resignation0 = shared.tx["resignation-0"].hex;
    const strayByte = craft.withAsset(resignation0, Buffer.from([0xff]), SENDER, unsigned(resignation0));
    const versionZero = Buffer.from(shared.tx["transfer-1"].hex, "hex");
    versionZero[1] = 0;
    const forms = [repeated, strayByte, versionZero.toString("hex")];
    await both("a repeated vote name, a stray resignation byte and version 0", 10, block(10, forms, forms.map(decodeId)));

    // A memo of replacement characters, and a copy of the same block whose memo holds 0xff bytes in
    // their place: the same transaction id and block id.
    const replacement = shared.tx["transfer-memo-replacement"].hex;
    const malleated = craft.withMemoBytes(replacement, Buffer.from([0xff, 0xff]), SENDER, unsigned(replacement));
    await both("a memo of replacement characters", 10, block(10, [replacement], [decodeId(replacement)]));
    await both("the same block with 0xff bytes in the memo", 10, block(10, [malleated], [decodeId(malleated)]));

    // The payload-size rule counts the canonical bytes of a received block: under maxPayload equal
    // to the canonical size, the block with the wire forms is received but not decoded; one byte
    // less refuses it both ways.
    const transfer = shared.tx["transfer-1"].hex;
    const pair = [repeated, strayByte, transfer];
    const canonicalSize = 141 + 64 + pair.reduce((n, hex) => n + 4 + Transactions.Utils.toBytes(Transactions.TransactionFactory.fromBytes(Buffer.from(hex, "hex")).data).length, 0);
    const pairBlock = block(4, pair, pair.map(decodeId));
    for (const [name, maxPayload] of [
        ["maxPayload equal to the canonical size", canonicalSize],
        ["maxPayload one byte under the canonical size", canonicalSize - 1],
    ]) {
        await both(name, 4, pairBlock, { milestones: milestonesText({ block: `{"version":0,"maxTransactions":3,"maxPayload":${maxPayload}}` }) });
    }

    // A header that counts transactions, with the bytes ending after the signature.
    const counted = block(10, [transfer], [decodeId(transfer)]);
    await both("transactions counted, none sent", 10, counted.slice(0, 2 * (141 + 64)));
}
