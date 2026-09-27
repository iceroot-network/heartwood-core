"use strict";
// V27, the processor: chain.process on a short chain, in its chain form (the chain given whole in
// each record): every result the reference's BlockProcessor reaches, with the handler that decides
// it, and each unchained status. The chain is made from the first chain definition of V10: its
// genesis block keeps the issuance record and the registrations and votes of the first seven
// delegates, signed again by the generator, and the first milestone has seven active delegates,
// so that a round is seven blocks. Blocks are forged by the delegates of their slots with the
// reference's classes and the oracle's fixed aux.
//
// The cases sit on three tips: the middle of round 2 (height 12), the block before its end (13)
// and its end (14). They cover the accepted block (with transactions, after missed slots, a
// delegate's second block in a round, the forked block cleared or kept, the last downloaded block
// moved or kept, a blockchain not started, unstored blocks); each refusal before the apply
// (verification, nonce order, generator, reward, a unique type twice, a stored id); every unchained
// status, the double forging at a round end included; a block the state refuses in its apply,
// once after missed slots (the assertion record of the missed rows the reference keeps); and a
// block with a transaction of a removed type (refused by both, by Heartwood when it rebuilds it).
// Each record's expected result is checked when it is written.
//
// Needs V10's chain definitions: without the devnet generator the part has no records.

const crypto = require("crypto");

const { FIXED_AUX } = require("../ops");
const { ReplaySession } = require("../corpus");
const { IPFS_HASH } = require("./constants");

const CRYPTO_FILES = ["network", "milestones", "genesisBlock", "exceptions"];
// The delegates of the short chain, and so the length of its rounds.
const DELEGATES = 7;
// Where the forging stops: the end of round 3's first block, beyond every tip of the cases.
const CHAIN_TOP = 14;

function sha256(data) {
    return crypto.createHash("sha256").update(data).digest();
}

/** The short chain's crypto files, from V10's chain definition `definition`. */
function shortChainFiles(solar, definition) {
    const { Blocks, Crypto, Identities, Utils } = solar.lib;
    const files = Object.fromEntries(CRYPTO_FILES.map((f) => [f, JSON.parse(definition.files[`crypto/${f}.json`])]));
    const names = Array.from({ length: DELEGATES }, (_, i) => `genesis_${i + 1}`);
    const kept = new Set(names.map((name) => Identities.PublicKey.fromPassphrase(definition.passphrases[name])));
    const generator = Identities.PublicKey.fromPassphrase(definition.passphrases.generator);
    const transactions = files.genesisBlock.transactions.filter((t) => t.senderPublicKey === generator || kept.has(t.senderPublicKey));
    const ids = transactions.map((t) => Buffer.from(t.id, "hex"));
    const payloadHash = sha256(Buffer.concat(ids)).toString("hex");
    const sum = (key) => transactions.reduce((total, t) => total.plus(t[key] || 0), Utils.BigNumber.ZERO);
    const header = {
        version: files.genesisBlock.version,
        timestamp: 0,
        height: 1,
        previousBlock: "0".repeat(64),
        numberOfTransactions: transactions.length,
        totalAmount: sum("amount"),
        totalFee: sum("fee"),
        reward: Utils.BigNumber.ZERO,
        payloadLength: 32 * transactions.length,
        payloadHash,
        generatorPublicKey: generator,
    };
    const hash = sha256(Blocks.Serialiser.serialise(header, false));
    header.blockSignature = Crypto.Hash.signSchnorrBip340(hash, Identities.Keys.fromPassphrase(definition.passphrases.generator), FIXED_AUX);
    const genesisBlock = {
        blockSignature: header.blockSignature,
        generatorPublicKey: generator,
        height: 1,
        id: Blocks.Block.getId(header),
        numberOfTransactions: header.numberOfTransactions,
        payloadHash,
        payloadLength: header.payloadLength,
        previousBlock: header.previousBlock,
        reward: "0",
        timestamp: 0,
        totalAmount: header.totalAmount.toString(),
        totalFee: header.totalFee.toString(),
        transactions,
        version: header.version,
    };
    const milestones = files.milestones.map((m, i) => (i === 0 ? { ...m, activeDelegates: DELEGATES } : m));
    return { network: { ...files.network, nethash: payloadHash }, milestones, genesisBlock, exceptions: {} };
}

/** The forger of the short chain: blocks and transactions signed with the definition's keys. */
function makeForger(solar, definition, session) {
    const { Blocks, Crypto, Identities, Transactions, Utils } = solar.lib;
    const keysOf = new Map();
    for (const [name, passphrase] of Object.entries(definition.passphrases)) {
        const keys = Identities.Keys.fromPassphrase(passphrase);
        keysOf.set(keys.publicKey.secp256k1, { name, passphrase, keys });
    }
    const passphrase = (name) => definition.passphrases[name];
    const chain = () => session.chain;
    const blockTime = () => solar.lib.Managers.configManager.getMilestone(1).blockTime;
    const nonce = (name) => chain().walletRepository.getNonce(Identities.Address.fromPassphrase(passphrase(name)));

    /** A transaction of the builder `kind` from `sender`, with the sender's next nonce plus `skip`. */
    function transaction(kind, sender, calls, skip = 0) {
        let builder = Transactions.BuilderFactory[kind]();
        for (const [method, ...args] of calls) builder = builder[method](...args);
        const next = nonce(sender).plus(1 + skip);
        return builder.nonce(next.toFixed()).fee("20000000").sign(passphrase(sender)).build();
    }

    /**
     * The next block, in the first slot `gap` slots after the next, by the delegate of that slot,
     * with its reward; or with the fields `forge` sets. Transactions are built transactions or raw
     * bytes. Returns the block's hex.
     */
    async function block(forge = {}) {
        const last = chain().lastBlock().data;
        const lastSlot = Math.floor(last.timestamp / blockTime());
        const slot = forge.slot ?? lastSlot + 1 + (forge.gap || 0);
        const forging = chain().round().forging;
        const scheduled = forging[slot % forging.length];
        const username = scheduled.getAttribute("delegate.username");
        const height = forge.height ?? last.height + 1;
        const signer = forge.signer ? Identities.Keys.fromPassphrase(passphrase(forge.signer)) : Identities.Keys.fromPassphrase(passphrase(username));
        let reward = forge.reward;
        if (reward === undefined) {
            const wallet = chain().walletRepository.findByUsername(username);
            reward = (await chain().roundState.getRewardForBlockInRound(height, wallet)).reward.toFixed();
        }
        const txs = (forge.transactions || []).map((t) => (Buffer.isBuffer(t) ? { serialised: t, id: sha256(t).toString("hex"), amount: undefined, fee: undefined } : t));
        const bytes = txs.map((t) => t.serialised);
        const fromBytes = txs.map((t) => (t.data ? t.data : Transactions.TransactionFactory.fromBytesUnsafe(t.serialised).data));
        const data = {
            version: forge.version ?? 0,
            timestamp: forge.timestamp ?? slot * blockTime(),
            height,
            previousBlock: forge.previous ?? last.id,
            numberOfTransactions: txs.length,
            totalAmount: fromBytes.reduce((s, t) => s.plus(t.amount || 0), Utils.BigNumber.ZERO),
            totalFee: fromBytes.reduce((s, t) => s.plus(t.fee), Utils.BigNumber.ZERO),
            reward: Utils.BigNumber.make(reward),
            payloadLength: forge.payloadLength ?? 32 * txs.length,
            payloadHash: sha256(Buffer.concat(fromBytes.map((t) => Buffer.from(t.id, "hex")))).toString("hex"),
            generatorPublicKey: signer.publicKey.secp256k1,
        };
        data.blockSignature = Crypto.Hash.signSchnorrBip340(sha256(Blocks.Serialiser.serialise(data, false)), signer, FIXED_AUX);
        if (forge.badSignature) {
            const signature = Buffer.from(data.blockSignature, "hex");
            signature[63] ^= 1;
            data.blockSignature = signature.toString("hex");
        }
        const lengths = Buffer.alloc(4 * bytes.length);
        bytes.forEach((b, i) => lengths.writeUInt32LE(b.length, 4 * i));
        return Buffer.concat([Blocks.Serialiser.serialise(data, true), lengths, ...bytes]).toString("hex");
    }

    return { block, transaction, nonce, keysOf, blockTime };
}

module.exports = async function processorVectors(ctx) {
    const { solar, devnet, rec, shared } = ctx;
    if (!shared.genesisDefinitions) return null;
    const definition = shared.genesisDefinitions[0];
    const files = shortChainFiles(solar, definition);
    const { Identities } = solar.lib;
    const address = (name) => Identities.Address.fromPassphrase(definition.passphrases[name]);

    // The short chain: funds and a vote early in round 1, missed slots in round 2.
    const session = new ReplaySession(solar, files);
    const forger = makeForger(solar, definition, session);
    const hexes = [];
    const epochMs = Date.parse(files.milestones[0].epoch);
    await session.chain.withConfig(
        () => devnet.restore(),
        async () => {
            await session.start();
            const plan = {
                3: () => [forger.transaction("transfer", "genesis-1", [["addTransfer", address("wallet/1"), "500000000000"], ["addTransfer", address("wallet/2"), "500000000000"]])],
                4: () => [forger.transaction("vote", "wallet/1", [["votesAsset", { genesis_2: 100 }]])],
            };
            while (session.chain.lastBlock().data.height < CHAIN_TOP) {
                const height = session.chain.lastBlock().data.height + 1;
                const hex = await forger.block({ transactions: plan[height] ? plan[height]() : [], gap: height === 10 ? 2 : 0 });
                const r = await session.apply(Buffer.from(hex, "hex"));
                if (!r.accepted) throw new Error(`V27: the short chain's block ${height} was refused: ${r.error.message}`);
                hexes.push(hex);
            }
        },
    );

    const out = [];
    const chainAt = (tip) => ({ crypto: files, blocks: hexes.slice(0, tip - 1) });

    /** The state of the chain session at `tip`: the cases' blocks are built on it. */
    async function at(tip, build) {
        const s = new ReplaySession(solar, files);
        const f = makeForger(solar, definition, s);
        const cases = [];
        await s.chain.withConfig(
            () => devnet.restore(),
            async () => {
                await s.start();
                for (const hex of hexes.slice(0, tip - 1)) await s.apply(Buffer.from(hex, "hex"));
                const tipData = s.chain.lastBlock().data;
                const tipKey = [...f.keysOf.values()].find((k) => k.keys.publicKey.secp256k1 === tipData.generatorPublicKey);
                await build({ f, s, tipData, tipKey, add: (c) => cases.push(c) });
            },
        );
        for (const c of cases) {
            const ts = Buffer.from(c.hex, "hex").readUInt32LE(4);
            const input = { chain: chainAt(tip), hex: c.hex, now: c.now ?? epochMs + ts * 1000 + 1500 };
            if (c.state) input.state = c.state;
            const r = await rec("chain.process", tip + 1, input);
            const got = r.output || {};
            const wanted = c.expect;
            for (const [key, value] of Object.entries(wanted)) {
                if (JSON.stringify(got[key]) !== JSON.stringify(value)) {
                    throw new Error(`V27 "${c.name}": ${key} is ${JSON.stringify(got[key])}, expected ${JSON.stringify(value)} (${JSON.stringify(r.error || got).slice(0, 300)})`);
                }
            }
            out.push({ ...r, name: c.name });
        }
    }

    const accepted = { result: "Accepted", handler: "AcceptBlockHandler" };
    const refused = (handler) => ({ result: "Rejected", handler });
    const unchained = (status, result) => ({ result, handler: "UnchainedHandler", unchained: status });
    const link = (d) => ({ height: d.height, id: d.id, previousBlock: d.previousBlock, timestamp: d.timestamp });

    // The middle of round 2.
    await at(12, async ({ f, s, tipData, tipKey, add }) => {
        const next = await f.block();
        const transfer = f.transaction("transfer", "genesis-1", [["addTransfer", address("wallet/3"), "1000"]]);
        const vote = f.transaction("vote", "wallet/2", [["votesAsset", { genesis_3: 100 }]]);
        add({ name: "accepted: the next block", hex: next, expect: accepted });
        add({ name: "accepted: with transactions", hex: await f.block({ transactions: [transfer, vote] }), expect: { ...accepted, removed: [transfer.id, vote.id] } });
        add({ name: "accepted: after two missed slots", hex: await f.block({ gap: 2 }), expect: accepted });
        add({ name: "accepted: the blockchain not started, no last downloaded block", hex: next, state: { started: false, lastDownloaded: null }, expect: { ...accepted, wakeUp: false, lastDownloaded: null } });
        const ahead = { height: 20, id: "ab".repeat(32), previousBlock: "cd".repeat(32), timestamp: 999 };
        add({ name: "accepted: a last downloaded block ahead stays", hex: next, state: { lastDownloaded: ahead }, expect: { ...accepted, lastDownloaded: { height: 20, id: ahead.id } } });
        add({ name: "accepted: the forked block at its height is cleared", hex: next, state: { forked: next }, expect: { ...accepted, forked: null } });
        const later = await f.block({ height: 20 });
        add({ name: "accepted: a forked block at another height stays", hex: next, state: { forked: later }, expect: { ...accepted, forked: 20 } });
        add({ name: "accepted: two blocks of the job not stored yet", hex: next, state: { unstored: 2 }, expect: accepted });
        // A delegate that has forged in this round gets the secondary reward in a slot after a wrap.
        const inRound = new Set(s.chain.round().blocks.map((b) => b.data.username));
        const forging = s.chain.round().forging;
        const lastSlot = Math.floor(tipData.timestamp / f.blockTime());
        const second = Array.from({ length: 14 }, (_, g) => g).find((g) => inRound.has(forging[(lastSlot + 1 + g) % forging.length].getAttribute("delegate.username")));
        add({ name: "accepted: a delegate's second block in a round", hex: await f.block({ gap: second }), expect: accepted });
        add({ name: "verification: a bad signature", hex: await f.block({ badSignature: true }), expect: refused("VerificationFailedHandler") });
        add({ name: "verification: version 1", hex: await f.block({ version: 1 }), expect: refused("VerificationFailedHandler") });
        add({ name: "verification: a payload length for one transaction", hex: await f.block({ payloadLength: 32 }), expect: refused("VerificationFailedHandler") });
        const nextTs = Buffer.from(next, "hex").readUInt32LE(4);
        add({ name: "verification: a timestamp in the future", hex: next, now: epochMs + (nextTs - 9) * 1000, expect: refused("VerificationFailedHandler") });
        add({ name: "accepted: a timestamp one block time ahead", hex: next, now: epochMs + (nextTs - 8) * 1000, expect: accepted });
        const skipped = f.transaction("transfer", "genesis-1", [["addTransfer", address("wallet/3"), "1000"]], 1);
        add({ name: "nonce: a gap", hex: await f.block({ transactions: [skipped] }), expect: refused("NonceOutOfOrderHandler") });
        const again = f.transaction("transfer", "genesis-1", [["addTransfer", address("wallet/4"), "1000"]]);
        add({ name: "nonce: one nonce twice", hex: await f.block({ transactions: [transfer, again] }), expect: refused("NonceOutOfOrderHandler") });
        add({ name: "unchained: a height above the next", hex: await f.block({ height: tipData.height + 2 }), expect: unchained("NotReadyToAcceptNewHeight", "Rejected") });
        add({ name: "unchained: a block of the chain", hex: hexes[8], expect: unchained("AlreadyInBlockchain", "DiscardedButCanBeBroadcasted") });
        add({ name: "unchained: the tip", hex: hexes[tipData.height - 2], expect: unchained("EqualToLastBlock", "DiscardedButCanBeBroadcasted") });
        const tipSlot = { height: tipData.height, previous: tipData.previousBlock, reward: tipData.reward.toFixed() };
        const other = [...f.keysOf.values()].find((k) => k.name.startsWith("genesis_") && k !== tipKey).name;
        add({ name: "unchained: the tip's height, earlier", hex: await f.block({ ...tipSlot, signer: tipKey.name, timestamp: tipData.timestamp - 8 }), expect: unchained("InvalidTimestamp", "Rejected") });
        add({ name: "unchained: the tip's slot again, by its delegate", hex: await f.block({ ...tipSlot, signer: tipKey.name, timestamp: tipData.timestamp + 1 }), expect: unchained("DoubleForging", "Rollback") });
        add({ name: "unchained: the tip's slot again, by another delegate", hex: await f.block({ ...tipSlot, signer: other, timestamp: tipData.timestamp + 1 }), expect: unchained("GeneratorMismatch", "Rejected") });
        add({ name: "unchained: the next height on another parent", hex: await f.block({ previous: "07".repeat(32) }), expect: unchained("DoubleForging", "Rollback") });
        const scheduled = f.keysOf.get(hexData(solar, next).generatorPublicKey).name;
        const notScheduled = [...f.keysOf.values()].find((k) => k.name.startsWith("genesis_") && k.name !== scheduled).name;
        add({ name: "unchained: another parent, another delegate", hex: await f.block({ previous: "07".repeat(32), signer: notScheduled }), expect: unchained("GeneratorMismatch", "Rejected") });
        add({ name: "generator: another delegate", hex: await f.block({ signer: notScheduled }), expect: refused("InvalidGeneratorHandler") });
        add({ name: "generator: no delegate", hex: await f.block({ signer: "wallet/5" }), expect: refused("InvalidGeneratorHandler") });
        add({ name: "reward: one more", hex: await f.block({ reward: "180000001" }), expect: refused("InvalidRewardHandler") });
        add({ name: "reward: none", hex: await f.block({ reward: "0" }), expect: refused("InvalidRewardHandler") });
        const vote1 = f.transaction("vote", "wallet/1", [["votesAsset", { genesis_4: 100 }]]);
        const vote2 = f.transaction("vote", "wallet/1", [["votesAsset", { genesis_5: 100 }]], 1);
        add({ name: "conflicting: two votes from one wallet", hex: await f.block({ transactions: [vote1, vote2] }), expect: refused("ConflictingTransactionsHandler") });
        add({ name: "forged: a stored id", hex: await f.block({ transactions: [transfer] }), state: { forged: [transfer.id] }, expect: refused("AlreadyForgedHandler") });
        const all = f.transaction("transfer", "wallet/2", [["addTransfer", address("wallet/3"), "600000000000"]]);
        add({ name: "apply: a transfer over the balance", hex: await f.block({ transactions: [all] }), expect: { ...refused("AcceptBlockHandler"), missed: [] } });
        add({ name: "apply: a transfer over the balance after missed slots", hex: await f.block({ transactions: [all], gap: 2 }), expect: refused("AcceptBlockHandler") });
        const ipfs = f.transaction("ipfs", "genesis-1", [["ipfsAsset", IPFS_HASH]]);
        add({ name: "a removed type", hex: await f.block({ transactions: [ipfs] }), expect: refused("AcceptBlockHandler") });
        add({ name: "every refusal resets the last downloaded block", hex: await f.block({ reward: "0" }), state: { lastDownloaded: link(hexData(solar, hexes[4])) }, expect: { ...refused("InvalidRewardHandler"), lastDownloaded: { height: tipData.height, id: tipData.id } } });
    });

    // The block before the end of round 2, and its end.
    await at(13, async ({ f, add }) => {
        add({ name: "accepted: the block that ends a round", hex: await f.block(), expect: accepted });
    });
    await at(14, async ({ f, tipData, tipKey, add }) => {
        add({ name: "accepted: the first block of a round", hex: await f.block(), expect: accepted });
        const tipSlot = { height: tipData.height, previous: tipData.previousBlock, reward: tipData.reward.toFixed() };
        const other = [...f.keysOf.values()].find((k) => k.name.startsWith("genesis_") && k !== tipKey).name;
        add({ name: "unchained: the slot of a round's last block again, by its delegate", hex: await f.block({ ...tipSlot, signer: tipKey.name, timestamp: tipData.timestamp + 1 }), expect: unchained("DoubleForging", "Rollback") });
        add({ name: "unchained: the slot of a round's last block again, by another delegate", hex: await f.block({ ...tipSlot, signer: other, timestamp: tipData.timestamp + 1 }), expect: unchained("GeneratorMismatch", "Rejected") });
    });
    return out;
};

/** The header data of block hex. */
function hexData(solar, hex) {
    return solar.lib.Blocks.Deserialiser.deserialise(Buffer.from(hex, "hex"), true).data;
}
