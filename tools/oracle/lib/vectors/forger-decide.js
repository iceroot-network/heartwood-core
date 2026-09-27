"use strict";
// V29, the forger's decisions and the internal routes' bodies:
//   forger.decide           every branch and line of one attempt of forgeNewBlock and of
//                           isForgingAllowed: forged (an empty pool, a pool with a transaction,
//                           the height line), the early stops with and without the "already forged
//                           on another node" line (the network state's clause, which compares a key,
//                           held by a state that names the key), each refusal with its line and
//                           the debug dump, the distinct over-height headers, an over-height header
//                           by the delegate's own key (empty, not empty, and not signed), the two
//                           late lines, the same-slot answer, and each call failing, with the relay
//                           not ready
//   internal.networkState   NetworkState objects of every status built by the reference's class,
//                           with and without a tip, its generator and slot, over-height headers,
//                           the quorum's counts around the threshold, and bodies without a status
//   internal.currentRound   getCurrentRound's object over wallets of the state package, with and
//                           without a forger at the current and next index
// The delegates' keys come from public test passphrases.

const { SENDER } = require("./constants");

const passphrase = (n) => `forger decision delegate ${n}`;

module.exports = async function forgerDecideVectors({ solar, rec, must, shared }) {
    const { Blocks, Crypto, Identities, Utils } = solar.lib;
    const H = shared.height;
    const out = [];
    const record = async (name, op, input) => out.push({ ...(await rec(op, H, input)), name });
    const keys = (n) => Identities.Keys.fromPassphrase(passphrase(n));
    const hex = (n) => keys(n).publicKey.secp256k1;
    const wallet = (n) => ({
        address: Identities.Address.fromPublicKey(hex(n)),
        publicKey: hex(n),
        balance: "0",
        nonce: "0",
        delegate: { username: `genesis_${n}`, voteBalance: "0", rank: n },
    });

    // ---- internal.currentRound ------------------------------------------------------------------
    // The body a reference relay writes (internal.roundJson), then the forger's reading of it.
    const round = async (name, fixture) => {
        const { json } = must(await rec("internal.roundJson", H, fixture)).output;
        await record(name, "internal.currentRound", { json });
    };
    const wallets = [1, 2, 3, 4].map((n) => ({ passphrase: passphrase(n), username: `genesis_${n}`, balance: String(n * 100), nonce: String(n), rank: n }));
    const tip = { passphrase: passphrase(2), height: 12, timestamp: 88, username: "genesis_2" };
    for (const [name, current, next] of [
        ["both forgers", 0, 1],
        ["no forger at the next index", 2, null],
        ["no forger at the current index", null, 0],
    ]) {
        await round(`current round: ${name}`, {
            wallets,
            active: [1, 0, 3],
            current,
            next,
            lastBlock: tip,
            reward: "500000000",
            timestamp: 104,
            canForge: true,
            round: 1,
        });
    }
    await round("current round: the tip without a username, before the epoch", {
        wallets: wallets.slice(0, 1),
        active: [0],
        current: 0,
        next: 0,
        lastBlock: { passphrase: passphrase(1), height: 1, timestamp: 0 },
        reward: "0",
        timestamp: -8,
        canForge: false,
        round: 1,
    });

    // ---- internal.networkState ------------------------------------------------------------------
    const header = (n, height) => ({
        id: `${String(n).padStart(2, "0")}`.repeat(32),
        version: 0,
        timestamp: height * 8,
        previousBlock: "00".repeat(32),
        height,
        numberOfTransactions: 0,
        totalAmount: "0",
        totalFee: "0",
        reward: "0",
        payloadLength: 0,
        payloadHash: "00".repeat(32),
        generatorPublicKey: hex(n),
        blockSignature: "00".repeat(64),
    });
    const details = (has, no, extra = {}) => ({ hasQuorum: has, noQuorum: no, ...extra });
    for (const status of [0, 1, 2, 3, 4]) {
        await record(`network state: status ${status} with a tip`, "internal.networkState", {
            status,
            lastBlock: tip,
            lastSlotNumber: 11,
            quorumDetails: details(30, 2, {
                overHeight: 1,
                overHeightBlockHeaders: { "192.0.2.7": header(5, 13) },
                forked: 1,
                differentSlot: 3,
                forgingNotAllowed: 4,
                delegates: { hasQuorum: ["genesis_1"], noQuorum: ["genesis_5"], noResponse: ["genesis_9"] },
            }),
        });
    }
    await record("network state: a tip without a username", "internal.networkState", {
        status: 0,
        lastBlock: { passphrase: passphrase(1), height: 3, timestamp: 16 },
        lastSlotNumber: 2,
        quorumDetails: details(53, 0),
    });
    await record("network state: no tip", "internal.networkState", { status: 1 });
    for (const [has, no] of [[27, 26], [26, 27], [3, 0], [2, 1], [0, 0], [60, 0], [53, 53]]) {
        await record(`network state: the quorum of ${has} to ${no}`, "internal.networkState", { status: 0, lastBlock: tip, lastSlotNumber: 11, quorumDetails: details(has, no) });
    }
    await record("network state: over-height headers of three peers", "internal.networkState", {
        status: 0,
        lastBlock: tip,
        lastSlotNumber: 11,
        quorumDetails: details(10, 3, { overHeight: 3, overHeightBlockHeaders: { "192.0.2.1": header(4, 14), "192.0.2.2": header(3, 13), "192.0.2.3": header(4, 14) } }),
    });
    for (const [name, body] of [
        ["an empty object", "{}"],
        ["null", "null"],
        ["a number", "5"],
        ["the test status only", '{"status":2}'],
        ["a height without a status", '{"nodeHeight":5}'],
        ["no quorum details", '{"status":0,"nodeHeight":5,"lastBlockId":"ab","lastSlotNumber":3}'],
    ]) {
        await record(`network state body: ${name}`, "internal.networkState", { body });
    }

    // ---- forger.decide --------------------------------------------------------------------------
    // Slot 13 is delegate 1's: its round at height 12, the relay's tip, forged by delegate 2 in
    // slot 12. The forger's clock leaves 6 s of the slot.
    const roundBody = (overrides = {}) => ({
        allDelegates: [1, 2, 3, 4].map(wallet),
        canForge: true,
        current: 1,
        currentForger: wallet(1),
        delegates: [wallet(1), wallet(2), wallet(3)],
        lastBlock: { height: 12, id: "12".repeat(32) },
        nextForger: wallet(2),
        reward: "500000000",
        timestamp: 104,
        ...overrides,
    });
    const state = (overrides = {}, quorum = {}) => ({
        status: 0,
        quorumDetails: {
            hasQuorum: 53,
            noQuorum: 0,
            overHeight: 0,
            overHeightBlockHeaders: {},
            forked: 0,
            differentSlot: 0,
            forgingNotAllowed: 0,
            delegates: { hasQuorum: [], noQuorum: [], noResponse: [] },
            ...quorum,
        },
        nodeHeight: 12,
        lastBlockId: "12".repeat(32),
        lastGenerator: "genesis_2",
        lastSlotNumber: 12,
        ...overrides,
    });
    const base = {
        round: roundBody(),
        networkState: state(),
        status: { timestamp: 96, generatorPublicKey: hex(2) },
        slots: [13, 13, 12, 13],
        timeLeftInMs: 6000,
        privateKey: keys(1).privateKey,
        firstAttempt: true,
    };
    const decide = (name, overrides) => record(`decide: ${name}`, "forger.decide", { ...base, ...overrides });

    await decide("forged, first attempt, an empty pool", {});
    await decide("forged, a later attempt", { firstAttempt: false });
    await decide("forged, a pool of 7 with nothing collated", { pool: { poolSize: 7, transactions: [] } });
    const transfer = must(await rec("tx.build", H, { builder: "transfer", calls: [["recipientId", Identities.Address.fromPublicKey(hex(9))], ["amount", "5"], ["nonce", "1"]], sign: SENDER })).output.hex;
    await decide("forged with one transaction", { pool: { poolSize: 1, transactions: [transfer] } });
    await decide("forged, exactly 2,000 ms left", { timeLeftInMs: 2000 });
    await decide("forged, the network state ahead of the round", { networkState: state({ nodeHeight: 1234, lastBlockId: "34".repeat(32) }), round: roundBody({ lastBlock: { height: 1000, id: "00".repeat(32) } }) });
    await decide("forged, the test status with no quorum", { networkState: state({ status: 2 }, { hasQuorum: 0, noQuorum: 9 }) });

    await decide("stop: the last block is in the current slot", { slots: [13, 13, 13, 13], status: { timestamp: 104, generatorPublicKey: hex(3) } });
    await decide("stop: already forged on another node, by the status", { slots: [13, 13, 13, 13], status: { timestamp: 104, generatorPublicKey: hex(1) } });
    await decide("stop: already forged, a later attempt", { slots: [13, 13, 13, 13], status: { timestamp: 104, generatorPublicKey: hex(1) }, firstAttempt: false });
    await decide("stop: not the round's forger", { round: roundBody({ currentForger: wallet(2) }) });
    await decide("stop: the network state names the key", { networkState: state({ lastGenerator: hex(1), lastSlotNumber: 13 }), round: roundBody({ currentForger: wallet(3) }) });
    await decide("stop: the status has no header fields", { slots: [13, 13, 13, 13], status: {} });

    for (const [name, status] of [["unknown", 4], ["cold start", 3], ["below the minimum", 1]]) {
        await decide(`refused: ${name}`, { networkState: state({ status }) });
    }
    await decide("refused: no quorum, with the dump", {
        networkState: state({}, { hasQuorum: 26, noQuorum: 27, delegates: { hasQuorum: ["genesis_2"], noQuorum: ["genesis_3"], noResponse: ["genesis_4"] } }),
    });
    await decide("refused: no delegate counted", { networkState: state({}, { hasQuorum: 0, noQuorum: 0 }) });

    // Over-height headers: another delegate's twice and once more, the delegate's own empty block,
    // its own block with a transaction, and its own header with a signature that does not verify.
    const made = (n, height) => {
        const block = Blocks.BlockFactory.make(
            {
                version: 0,
                timestamp: height * 8,
                previousBlock: "12".repeat(32),
                height,
                numberOfTransactions: 0,
                totalAmount: Utils.BigNumber.ZERO,
                totalFee: Utils.BigNumber.ZERO,
                reward: Utils.BigNumber.ZERO,
                payloadLength: 0,
                payloadHash: Crypto.HashAlgorithms.sha256([]).toString("hex"),
                transactions: [],
            },
            keys(n),
            Buffer.alloc(32, 5),
        );
        return JSON.parse(JSON.stringify(block.getHeader()));
    };
    const nonEmpty = (n, height) => {
        const data = {
            version: 0,
            generatorPublicKey: hex(n),
            timestamp: height * 8,
            previousBlock: "12".repeat(32),
            height,
            numberOfTransactions: 1,
            totalAmount: Utils.BigNumber.ZERO,
            totalFee: Utils.BigNumber.make(100),
            reward: Utils.BigNumber.ZERO,
            payloadLength: 32,
            payloadHash: Crypto.HashAlgorithms.sha256([Buffer.alloc(32, 1)]).toString("hex"),
        };
        const hash = Crypto.HashAlgorithms.sha256(Blocks.Serialiser.serialise(data, false));
        data.blockSignature = Crypto.Hash.signSchnorr(hash, keys(n), true, Buffer.alloc(32, 6));
        data.id = Blocks.Block.getId(data);
        return JSON.parse(JSON.stringify(data));
    };
    const over = (headers, quorum = {}) => state({}, { overHeight: Object.keys(headers).length, overHeightBlockHeaders: headers, ...quorum });
    await decide("over-height headers of others, one of them twice: forged", { networkState: over({ "192.0.2.1": made(2, 13), "192.0.2.2": made(2, 13), "192.0.2.3": made(3, 14) }) });
    await decide("over-height headers of others and no quorum", { networkState: over({ "192.0.2.1": made(2, 13), "192.0.2.3": made(3, 14) }, { hasQuorum: 10, noQuorum: 20 }) });
    await decide("refused: the delegate's own empty block over the height", { networkState: over({ "192.0.2.4": made(3, 13), "192.0.2.5": made(1, 13) }) });
    await decide("the delegate's own block with a transaction over the height", { networkState: over({ "192.0.2.6": nonEmpty(1, 13) }) });
    const unsigned = { ...made(1, 13), blockSignature: "00".repeat(64) };
    await decide("the delegate's own header, not signed, over the height", { networkState: over({ "192.0.2.7": unsigned }) });

    await decide("late: 1,999 ms left", { timeLeftInMs: 1999 });
    await decide("late: 1 ms left", { timeLeftInMs: 1 });
    await decide("late: the slot has ended", { timeLeftInMs: 0 });
    await decide("late: a slot and a half ago", { timeLeftInMs: -12000 });
    await decide("late: the relay is in the next slot, 6,000 ms left by the forger's clock", { slots: [13, 14, 12, 13] });
    await decide("same slot: the block's slot is the last block's", { slots: [13, 13, 12, 12] });

    for (const call of ["networkState", "roundSlot", "transactions", "status", "currentSlot", "lastBlockSlot", "blockSlot"]) {
        await decide(`error: ${call} fails`, { fail: { call, message: "Request timed out" } });
    }
    await decide("error: the relay is not ready", { fail: { call: "networkState", message: '{"error":"Unknown","message":"App is not ready"}' } });
    await decide("error: a network state without a height", { networkState: state({ nodeHeight: undefined }) });
    await decide("error: a network state without a status", { networkState: { nodeHeight: 12 } });
    await decide("error: a network state without a tip id", { networkState: state({ lastBlockId: undefined }) });
    await decide("error: a later attempt of a round without a current forger", { round: roundBody({ currentForger: undefined }), slots: [13, 13, 12, 13], firstAttempt: false });

    return out;
};
