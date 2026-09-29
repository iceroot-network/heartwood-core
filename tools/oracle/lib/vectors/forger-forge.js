"use strict";
// V29, block assembly. forger.forge: an empty block, each kept type alone and together (a burn's
// amount is the only one the total amount counts), a 1/6 transfer with 256 recipients, 150 and 151
// transactions against maxTransactions, a block over maxPayload, totals near and above 64 bits
// (burned amounts and fees), rewards up to 2^64 - 1, several fixed aux values on the same inputs,
// and milestones that change at the next height (the transfer maximum and maxTransactions).
// forger.collateAndForge: a forger's collation of a pool over a wallet fixture, then the block: an
// empty pool, fee and nonce order, maxTransactions, the byte budget of the tip, a failed sender
// skipped and removed, and a tip that moved. Every transaction is built with the reference's
// builders and signed with the fixed aux.

const crypto = require("crypto");

const { FIXED_AUX } = require("../ops");
const { GENERATOR, SENDER } = require("./constants");
const { milestonesText } = require("./milestones");

const PREVIOUS = { id: "ab".repeat(32), height: 5 };
const TIMESTAMP = 48;
const REWARD = "180000000";
const U63 = 2n ** 63n;

// The pool cases: the fixtures' last block, and the rules of the next height.
const LAST = 999;
const BALANCE = "100000000000000";
const FEE = "50000000";
const T0 = 1800000000000;

module.exports = async function forgerForgeVectors({ solar, devnet, rec, must, shared }) {
    const { Identities } = solar.lib;
    const out = [];
    const privateKey = Identities.PrivateKey.fromPassphrase(GENERATOR);
    const aux = FIXED_AUX.toString("hex");
    const base = { privateKey, aux, previousBlock: PREVIOUS, timestamp: TIMESTAMP, reward: REWARD };

    const forge = async (name, transactions, extra = {}) => {
        const input = { ...base, transactions, ...extra };
        out.push({ ...(await rec("forger.forge", input.previousBlock.height, input)), name });
    };
    const build = async (input) => must(await rec("tx.build", 2, input)).output.hex;
    const tx = (name) => shared.tx[name].hex;

    await forge("an empty block", []);
    const kept = ["transfer-1", "second-signature", "delegate-registration", "resignation-1", "burn", "vote-2"];
    for (const name of kept) await forge(`one ${name}`, [tx(name)]);
    const together = [...kept.map(tx), tx("transfer-2"), tx("burn-memo")];
    await forge("the kept types together, two burns", together);
    await forge("a transfer to 256 recipients", [tx("transfer-256")]);

    // maxTransactions, and a block over maxPayload.
    const small = [];
    for (let i = 0; i < 151; i++) {
        small.push(await build({ builder: "transfer", calls: [["recipientId", shared.recipients[i]], ["amount", String(100 + i)], ["nonce", String(i + 1)]], sign: SENDER }));
    }
    await forge("150 transactions", small.slice(0, 150));
    await forge("151 transactions, over maxTransactions", small);
    const pair = small.slice(0, 2);
    const size = 141 + 64 + pair.reduce((n, hex) => n + 4 + hex.length / 2, 0);
    await forge("a block one byte over maxPayload", pair, { milestones: milestonesText({ block: `{"version":0,"maxTransactions":2,"maxPayload":${size - 1}}` }) });

    // Totals near and above 64 bits. Each amount and fee is at most 2^63 - 1, the schema's
    // maximum, which the block's totals must also keep; a total of 2^64 or more does not fit the
    // header at all.
    const burn = (amountText, nonce) => build({ builder: "burn", calls: [["amount", amountText], ["nonce", String(nonce)], ["fee", "10000000"]], sign: SENDER });
    const pay = (fee, nonce) => build({ builder: "transfer", calls: [["recipientId", shared.recipients[0]], ["amount", "1"], ["nonce", String(nonce)], ["fee", fee]], sign: SENDER });
    const most = String(U63 - 1n);
    for (const [label, make] of [["burned amounts", burn], ["fees", pay]]) {
        const top = [await make(most, 1), await make(most, 2), await make(most, 3)];
        const split = [await make(String(U63 - 1001n), 1), await make("1000", 2)];
        await forge(`${label} summing to 2^63 - 1`, split);
        await forge(`${label} summing to 2^63`, [...split, await make("1", 3)]);
        await forge(`${label} summing to 2^64 - 2`, top.slice(0, 2));
        await forge(`${label} above 2^64`, top);
    }

    // A reward at the schema's maximum and above it.
    for (const reward of [String(U63 - 1n), String(U63), String(2n ** 64n - 1n)]) {
        await forge(`a reward of ${reward}`, [], { reward });
    }

    // Several fixed aux values on the same inputs.
    for (const value of ["00".repeat(32), "ff".repeat(32), crypto.createHash("sha256").update("forger aux").digest("hex")]) {
        await forge(`the kept types with aux ${value.slice(0, 8)}`, together, { aux: value });
    }

    // Milestones that change at the next height: the transactions are read at the previous
    // block's height, the block is made at its own.
    const three = await build({ builder: "transfer", calls: shared.recipients.slice(0, 3).map((r, i) => ["addTransfer", r, String(10 + i)]).concat([["nonce", "1"]]), sign: SENDER });
    const next = PREVIOUS.height + 1;
    const transferMaximum = (maximum) => `{"height":${next},"transfer":{"maximum":${maximum},"minimum":1}}`;
    await forge("three recipients, a transfer maximum of 2 from the next height", [three], {
        milestones: milestonesText({}, [transferMaximum(2)]),
    });
    await forge("three recipients, a transfer maximum of 2 up to the previous block", [three], {
        milestones: milestonesText({ transfer: '{"maximum":2,"minimum":1}' }, [transferMaximum(256)]),
    });
    await forge("two transactions, maxTransactions 1 from the next height", pair, {
        milestones: milestonesText({}, [`{"height":${next},"block":{"version":0,"maxTransactions":1,"maxPayload":2097152}}`]),
    });

    await collations({ solar, devnet, rec, out, base });
    return out;
};

/** forger.collateAndForge over pool fixtures. */
async function collations({ solar, devnet, rec, out, base }) {
    const { Identities, Transactions } = solar.lib;
    const key = (passphrase) => Identities.Keys.fromPassphrase(passphrase).publicKey.secp256k1;
    const address = (passphrase) => Identities.Address.fromPublicKey(key(passphrase), 90);
    const wallet = (passphrase, extra = {}) => ({ a: address(passphrase), pk: key(passphrase), bal: BALANCE, n: "5", ...extra });
    const delegate = (i) =>
        wallet(`v29 delegate ${i}`, { n: "1", bal: "1000000000", u: `v29d${i}`, pb: 3, ff: "300", fr: "600000000", bf: "270", dn: "60000000", lb: "ab".repeat(32) });
    const [A, B, C] = ["v29 a", "v29 b", "v29 c"];
    const fixture = [wallet(A), wallet(B), wallet(C), delegate(1), delegate(2), delegate(3)];
    const many = [wallet(A), wallet(B), wallet(C), ...Array.from({ length: 54 }, (_, i) => delegate(i + 1))];
    const build = (builder, calls, passphrase) => {
        let b = Transactions.BuilderFactory[builder]();
        for (const [method, ...args] of calls) b = b[method](...args);
        return b.sign(passphrase).build().serialised.toString("hex");
    };
    const R = address("v29 recipient");
    const transfer = (from, nonce, fee = FEE) => build("transfer", [["addTransfer", R, "1000"], ["nonce", String(nonce)], ["fee", String(fee)]], from);
    const resign = (i, fee) => build("delegateResignation", [["resignationTypeAsset", 0], ["nonce", "2"], ["fee", fee]], `v29 delegate ${i}`);
    const process = (hexes, now = T0) => ({ op: "process", entries: hexes.map((hex) => ({ hex })), now });
    const milestonesWith = (change) => JSON.stringify(devnet.config.milestones.map((m, i) => (i === 0 ? change(JSON.parse(JSON.stringify(m))) : m)));

    const add = async (name, input) => {
        const full = { fixture, lastBlockHeight: LAST, ...base, previousBlock: { id: base.previousBlock.id }, timestamp: 8000, ...input };
        out.push({ ...(await rec("forger.collateAndForge", LAST + 1, full)), name });
    };
    const mixed = process([transfer(A, 6, 20000000), transfer(A, 7, 90000000), transfer(B, 6, 50000000), transfer(C, 6, 50000000), transfer(C, 7, 10000000)]);
    await add("an empty pool", { steps: [] });
    await add("fee order, nonce order and ties", { steps: [mixed] });
    await add("maxTransactions", { milestones: milestonesWith((m) => ({ ...m, block: { ...m.block, maxTransactions: 2 } })), steps: [mixed] });
    // The budget is the tip's milestone: maxPayload leaves room for two of the transfers,
    // maxTransactions for three. With the default maxTransactions the tip's serialiser buffer
    // would be too small for any transaction, and the forger could not read the collation.
    const size = Buffer.from(transfer(A, 6), "hex").length;
    const block = devnet.config.milestones[0].block;
    const budget = 141 + 2 * (4 + size) + 3;
    const atTip = (maxTransactions) =>
        JSON.stringify([...devnet.config.milestones, { height: LAST, block: { ...block, maxTransactions, maxPayload: budget } }, { height: LAST + 1, block }]);
    await add("the byte budget of the tip", { milestones: atTip(3), steps: [mixed] });
    await add("the byte budget of the tip, the serialiser's buffer too small", { milestones: atTip(block.maxTransactions), steps: [mixed] });
    await add("a failed sender is skipped and removed", {
        fixture: many,
        steps: [process([resign(1, "50000000"), resign(2, "40000000"), transfer("v29 delegate 2", 3, 90000000), transfer(B, 6, 30000000)])],
    });
    await add("after the tip moved", { steps: [mixed, { op: "tip", height: LAST + 1 }, process([transfer(B, 7)], T0 + 1000)] });
}
