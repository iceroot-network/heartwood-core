"use strict";
// V21-pool: scripted pool cases over wallet fixtures (pool.run), each a sequence of steps with the
// reference's answer to every step and the pool's contents after it: every error code and
// message, the lists of a processor result, re-adds, clean-ups, removals and collations.
//
// The cases cover the duplicate and the cooldown (0 to 30 seconds, one processor per channel),
// the size limit, the network byte, the fee floor at the floor and one below it, the static-fee
// mode with the exact, a lower and a higher fee, broadcast apart from acceptance, a full pool
// (the fee at and below the lowest, an eviction, fee ties, and a refusal after an eviction), the
// per-sender limit with and without allowedSenders, a refusal of the handlers for every kept
// type, every pending and offline message, bad signatures and bytes that do not decode, the seven
// removed types as bytes, as JSON and as JSON with a null id, the removed types in the static-fee
// mode (a lower, a higher and the exact fee, then the exact fee in a full pool, over the
// per-sender limit, over the size limit and from another network), JSON entries with a correct,
// a wrong or no id, a sender
// spending a pending incoming transfer, nonce chains and gaps, re-adds after a flush and after a
// restart of the storage, age expiry, removal cascades, forged removal, and collations (fee and
// nonce order, a failed sender skipped, the byte budget, maxTransactions, exclude, validate on
// and off). Every transaction is built with the reference's builders and signed with the fixed
// aux.

const LAST = 999; // the fixtures' last block; the pool applies the rules of LAST + 1
const H = LAST + 1;
const BALANCE = "100000000000000"; // 1,000,000 coins
const FEE = "50000000";
const REG_FEE = "7600000000";
const T0 = 1800000000000; // the processor's clock, in milliseconds, a whole second

module.exports = async function pool({ solar, devnet, rec, must, craft }) {
    const { Identities, Transactions, Utils } = solar.lib;
    const out = [];
    const key = (passphrase) => Identities.Keys.fromPassphrase(passphrase).publicKey.secp256k1;
    const address = (passphrase) => Identities.Address.fromPublicKey(key(passphrase), 90);

    const wallet = (passphrase, extra = {}) => ({ a: address(passphrase), pk: key(passphrase), bal: BALANCE, n: "5", ...extra });
    const delegate = (i, extra = {}) =>
        wallet(`v21 delegate ${i}`, { n: "1", bal: "1000000000", u: `v21d${i}`, pb: 3, ff: "300", fr: "600000000", bf: "270", dn: "60000000", lb: "ab".repeat(32), ...extra });
    const A = "v21 a";
    const B = "v21 b";
    const C = "v21 c";
    const base = () => [wallet(A), wallet(B), wallet(C), delegate(1), delegate(2), delegate(3)];
    // One delegate more than the active delegates, so that one may resign.
    const many = (overrides = {}) => [wallet(A), wallet(B), wallet(C), ...Array.from({ length: 54 }, (_, i) => delegate(i + 1, overrides[i + 1] || {}))];

    const build = (builder, calls, passphrase, secondPassphrase) => {
        let b = Transactions.BuilderFactory[builder]();
        for (const [method, ...args] of calls) b = b[method](...args);
        b = b.sign(passphrase);
        if (secondPassphrase !== undefined) b = b.secondSign(secondPassphrase);
        return b.build().serialised.toString("hex");
    };
    const R = address("v21 recipient");
    const transfer = (from, nonce, fee = FEE, amount = "1000", to = R) => build("transfer", [["addTransfer", to, amount], ["nonce", String(nonce)], ["fee", String(fee)]], from);
    const vote = (from, votes, nonce, fee = FEE) => build("vote", [["votesAsset", votes], ["nonce", String(nonce)], ["fee", fee]], from);
    const resign = (i, type, nonce = 2) => build("delegateResignation", [["resignationTypeAsset", type], ["nonce", String(nonce)], ["fee", FEE]], `v21 delegate ${i}`);
    const json = (hex) => JSON.parse(JSON.stringify(Transactions.TransactionFactory.fromBytesUnsafe(Buffer.from(hex, "hex")).toJson()));
    const idOf = (hex) => Transactions.TransactionFactory.fromBytesUnsafe(Buffer.from(hex, "hex")).id;
    const minimumFee = async (hex, milestones) => BigInt(must(await rec("fee.minimum", H, milestones === undefined ? { hex } : { hex, milestones })).output.minimumFee);
    const process = (entries, now = T0, channel) => ({ op: "process", entries: entries.map((e) => (typeof e === "string" ? { hex: e } : e)), now, ...(channel ? { channel } : {}) });
    const milestonesWith = (change) => JSON.stringify(devnet.config.milestones.map((m, i) => (i === 0 ? change(JSON.parse(JSON.stringify(m))) : m)));

    const add = async (name, input) => {
        const r = must(await rec("pool.run", H, { fixture: base(), lastBlockHeight: LAST, ...input }));
        out.push({ ...r, name });
    };

    // Duplicates and the cooldown.
    const t6 = transfer(A, 6);
    await add("duplicate after the cooldown", { steps: [process([t6]), process([t6], T0 + 31000)] });
    await add("cooldown from 0 to 30 seconds", {
        steps: [
            process([t6, t6]),
            process([t6], T0),
            process([t6], T0 + 1000),
            process([t6], T0 + 29000),
            process([t6], T0 + 29001),
            process([t6], T0 + 30000),
        ],
    });
    await add("one cooldown per channel", { steps: [process([t6]), process([t6], T0 + 1000, "p2p"), process([t6], T0 + 2000, "p2p"), process([t6], T0 + 3000)] });

    // The size limit and the network byte.
    await add("size limit", { options: { maxTransactionBytes: 100 }, steps: [process([t6])] });
    await add("size limit of one byte", { options: { maxTransactionBytes: 1 }, steps: [process([t6])] });
    const withNetwork = (byte) =>
        craft.resign(t6, A, (u) => {
            const b = Buffer.from(u);
            b[2] = byte;
            return b;
        });
    await add("another network byte, and byte 0", { steps: [process([withNetwork(30), withNetwork(0)])] });

    // The fee floor, and the static-fee mode.
    const probe = transfer(A, 6, 1);
    const floor = await minimumFee(probe);
    await add("fee one below the floor and at it", { steps: [process([transfer(A, 6, floor - 1n)]), process([transfer(A, 6, floor)], T0 + 1000)] });
    const regProbe = build("delegateRegistration", [["usernameAsset", "v21new"], ["nonce", "6"], ["fee", "1"]], A);
    const regFloor = await minimumFee(regProbe);
    const registration = (fee) => build("delegateRegistration", [["usernameAsset", "v21new"], ["nonce", "6"], ["fee", String(fee)]], A);
    await add("registration one below the floor and at it", { steps: [process([registration(regFloor - 1n)]), process([registration(regFloor)], T0 + 1000)] });
    const feesOff = milestonesWith((m) => ({ ...m, dynamicFees: { ...m.dynamicFees, enabled: false } }));
    const staticFee = BigInt(devnet.config.milestones[0].fees.staticFees.transfer);
    await add("static fee: exact, lower and higher", {
        milestones: feesOff,
        options: { dynamicFees: { enabled: false } },
        steps: [process([transfer(A, 6, staticFee - 1n)]), process([transfer(A, 6, staticFee + 1n)], T0 + 1000), process([transfer(A, 6, staticFee)], T0 + 2000)],
    });
    await add("pool fees when the milestone's are off, broadcast apart", {
        milestones: feesOff,
        options: { dynamicFees: { enabled: true, minFeePool: 1000, minFeeBroadcast: 100000 } },
        steps: [process([transfer(A, 6, 1000000), transfer(B, 6, 100000000), transfer(C, 6, 1)])],
    });

    // A full pool.
    const full = { options: { maxTransactionsInPool: 3 } };
    const fill = process([transfer(A, 6, 20000000), transfer(B, 6, 30000000), transfer(C, 6, 40000000)]);
    await add("full pool: a fee at and below the lowest", { ...full, steps: [fill, process([transfer(A, 7, 20000000), transfer(B, 7, 10000000)], T0 + 1000)] });
    await add("full pool: a higher fee evicts the lowest", {
        ...full,
        steps: [
            process([transfer(A, 6, 20000000), transfer(A, 7, 20000000), transfer(B, 6, 30000000)]),
            process([transfer(C, 6, 25000000)], T0 + 1000),
            process([transfer(C, 7, 30000001)], T0 + 2000),
        ],
    });
    const ties = process([transfer(A, 6, 20000000), transfer(B, 6, 20000000), transfer(C, 6, 20000000)]);
    await add("full pool: fee ties go to the earlier sender", {
        ...full,
        fixture: [...base(), wallet("v21 d")],
        steps: [ties, process([transfer("v21 d", 6, 20000001)], T0 + 1000), process([transfer("v21 d", 7, 20000001)], T0 + 2000)],
    });
    await add("full pool: an eviction of the sender's own earlier transaction", {
        ...full,
        steps: [ties, process([transfer(A, 7, 20000001)], T0 + 1000)],
    });
    await add("full pool: a refused transaction after an eviction", {
        ...full,
        steps: [fill, process([transfer(B, 9, 90000000), transfer("v21 poor", 1, 90000000)], T0 + 1000)],
        fixture: [...base(), wallet("v21 poor", { bal: "1000", n: "0" })],
    });

    // The per-sender limit.
    await add("per-sender limit", { options: { maxTransactionsPerSender: 2 }, steps: [process([transfer(A, 6), transfer(A, 7), transfer(A, 8), transfer(B, 6)])] });
    await add("per-sender limit of one", { options: { maxTransactionsPerSender: 1 }, steps: [process([transfer(A, 6), transfer(A, 7)])] });
    await add("per-sender limit with an allowed sender", { options: { maxTransactionsPerSender: 2, allowedSenders: [address(A)] }, steps: [process([transfer(A, 6), transfer(A, 7), transfer(A, 8)])] });

    // The handlers' refusals, through ERR_APPLY.
    const poor = [...base(), wallet("v21 poor", { bal: "60000000", n: "0" })];
    await add("insufficient balance", { fixture: poor, steps: [process([transfer("v21 poor", 1, FEE, "10000001")])] });
    await add("nonce gaps and chains", {
        steps: [process([transfer(A, 7), transfer(A, 6)]), process([transfer(A, 7), transfer(A, 9), transfer(A, 8), transfer(A, 5), transfer(A, 9)], T0 + 31000)],
    });
    const second = [...base(), wallet("v21 second", { spk: key("v21 second secret") })];
    await add("second signatures", {
        fixture: second,
        steps: [
            process([
                build("transfer", [["addTransfer", R, "1"], ["nonce", "6"], ["fee", FEE]], "v21 second"),
                build("transfer", [["addTransfer", R, "1"], ["nonce", "6"], ["fee", FEE]], "v21 second", "v21 wrong secret"),
                build("transfer", [["addTransfer", R, "1"], ["nonce", "6"], ["fee", FEE]], A, "v21 unexpected secret"),
                build("transfer", [["addTransfer", R, "1"], ["nonce", "6"], ["fee", FEE]], "v21 second", "v21 second secret"),
            ]),
        ],
    });
    await add("second-signature registration refusals", {
        fixture: second,
        steps: [process([build("secondSignature", [["signatureAsset", "v21 new secret"], ["nonce", "6"], ["fee", FEE]], "v21 second")])],
    });
    await add("delegate registration refusals", {
        steps: [
            process([
                build("delegateRegistration", [["usernameAsset", "v21d2"], ["nonce", "6"], ["fee", REG_FEE]], A),
                build("delegateRegistration", [["usernameAsset", "v21other"], ["nonce", "2"], ["fee", REG_FEE]], "v21 delegate 1"),
            ]),
        ],
    });
    await add("resignation refusals", {
        fixture: many({ 2: { rs: 1, resignedAt: 500 } }),
        steps: [
            process([build("delegateResignation", [["resignationTypeAsset", 0], ["nonce", "6"], ["fee", FEE]], A), resign(1, 2)]),
            { op: "setOnline", delegates: [{ publicKey: key("v21 delegate 1"), round: 19 }, { publicKey: key("v21 delegate 2"), round: 19 }] },
            process([resign(1, 2), resign(2, 2), resign(2, 0)], T0 + 31000),
        ],
    });
    const burnAmount = String(devnet.config.milestones[0].burn.txAmount);
    await add("burn refusals", {
        steps: [process([build("burn", [["amount", String(BigInt(burnAmount) - 1n)], ["nonce", "6"], ["fee", FEE]], A), build("burn", [["amount", BALANCE], ["nonce", "6"], ["fee", FEE]], B)])],
    });
    const online = [1, 2, 3].map((i) => ({ publicKey: key(`v21 delegate ${i}`), round: 19 }));
    const voter = wallet("v21 voter", { v: [["v21d2", 10000]] });
    await add("vote refusals", {
        online,
        fixture: [wallet(A), wallet(B), wallet(C), voter, delegate(1, { rs: 0, resignedAt: 100 }), delegate(2), delegate(3)],
        steps: [process([vote(A, { v21nobody: 100 }, 6), vote(B, { v21d1: 100 }, 6), vote(C, {}, 6), vote("v21 voter", { v21d2: 100 }, 6), vote(A, { v21d2: 100 }, 5)])],
    });
    const bigVote = {};
    for (let i = 0; i < 60; i++) bigVote[`v21name${String(i).padStart(12, "0")}`] = i < 50 ? 2 : 0;
    const bigAsset = craft.voteAsset(Object.entries(bigVote).filter(([, v]) => v > 0).map(([name, v]) => [name, v * 100]));
    const smallVote = vote(A, { v21d1: 100 }, 6);
    await add("a vote asset over 1,024 bytes", { online, steps: [process([craft.withAsset(smallVote, bigAsset, A)])] });
    const arrayVote = { ...json(smallVote), asset: { votes: ["v21d1"] } };
    await add("a JSON vote asset given as an array and as a string", { online, steps: [process([{ json: arrayVote }, { json: { ...arrayVote, asset: { votes: "v21d1" } } }])] });
    await add("a first transaction from a wallet without a key", {
        fixture: [...base(), { a: address("v21 cold"), bal: BALANCE, n: "0" }],
        steps: [process([transfer("v21 cold", 1), transfer("v21 cold", 2)])],
    });

    // Pending and offline.
    await add("pending: a second vote, second signature and registration", {
        online,
        steps: [
            process([
                vote(A, { v21d1: 100 }, 6),
                vote(A, { v21d2: 100 }, 7),
                build("secondSignature", [["signatureAsset", "v21 s1"], ["nonce", "6"], ["fee", FEE]], B),
                build("secondSignature", [["signatureAsset", "v21 s2"], ["nonce", "7"], ["fee", FEE]], B),
                build("delegateRegistration", [["usernameAsset", "v21cname"], ["nonce", "6"], ["fee", REG_FEE]], C),
                build("delegateRegistration", [["usernameAsset", "v21cother"], ["nonce", "7"], ["fee", REG_FEE]], C),
            ]),
        ],
    });
    await add("pending: a name registered twice, a resignation twice", {
        fixture: many(),
        steps: [
            process([
                build("delegateRegistration", [["usernameAsset", "v21same"], ["nonce", "6"], ["fee", REG_FEE]], A),
                build("delegateRegistration", [["usernameAsset", "v21same"], ["nonce", "6"], ["fee", REG_FEE]], B),
                resign(1, 0, 2),
                resign(1, 1, 3),
            ]),
        ],
    });
    const resigned = [wallet(A), wallet(B), wallet(C), delegate(1, { rs: 0, resignedAt: 100 }), delegate(2), delegate(3)];
    await add("offline: votes and a revoke", {
        fixture: resigned,
        steps: [
            process([vote(A, { v21d2: 100 }, 6), vote(B, { v21d1: 100 }, 6), resign(1, 2)]),
            { op: "setOnline", delegates: [online[0], online[1]] },
            process([vote(A, { v21d2: 100 }, 6), vote(C, { v21d1: 50, v21d3: 50 }, 6), resign(1, 2)], T0 + 31000),
        ],
    });
    await add("online record expiry", {
        steps: [
            { op: "setOnline", delegates: [{ publicKey: key("v21 delegate 1"), round: 10 }, { publicKey: key("v21 delegate 2"), round: 14 }] },
            { op: "setOnline", delegates: [], expireAt: 20 },
            process([vote(A, { v21d1: 100 }, 6), vote(B, { v21d2: 100 }, 6)]),
        ],
    });

    // Bad signatures and bytes that do not decode.
    const flipped = Buffer.from(t6, "hex");
    flipped[flipped.length - 1] ^= 0x01;
    const truncated = t6.slice(0, 40);
    const unknownType = craft.resign(t6, A, (u) => {
        const b = Buffer.from(u);
        b.writeUInt16LE(11, 7);
        return b;
    });
    await add("bad signature and undecodable bytes", { steps: [process([flipped.toString("hex"), truncated, "", unknownType, "ff03"])] });

    // Removed types, as bytes and as JSON: one transaction of each, with fee(name) as its fee, the
    // name being the type's static-fee key.
    const participants = ["v21 multisig 1", "v21 multisig 2", "v21 multisig 3"];
    const publicKeys = participants.map((p) => key(p));
    const secretHash = "a".repeat(64);
    const REMOVED_NAMES = ["legacyTransfer", "legacyVote", "multiSignature", "ipfs", "htlcLock", "htlcClaim", "htlcRefund"];
    const REMOVED_SIGNERS = [A, B, participants[0], C, A, B, C];
    const removedTypes = (fee) => {
        const list = [];
        const legacyTransfer = craft.builderData("transfer", [["nonce", "6"]], A);
        Object.assign(legacyTransfer, { type: 0, amount: Utils.BigNumber.make("1000"), recipientId: R, expiration: 0, fee: Utils.BigNumber.make(fee("legacyTransfer")) });
        delete legacyTransfer.asset;
        list.push(craft.signAndSerialise(legacyTransfer, A).toString("hex"));
        const legacyVote = craft.builderData("transfer", [["nonce", "6"]], B);
        Object.assign(legacyVote, { type: 3, asset: { votes: ["+v21d1"] }, fee: Utils.BigNumber.make(fee("legacyVote")) });
        list.push(craft.signAndSerialise(legacyVote, B).toString("hex"));
        list.push(
            build(
                "multiSignature",
                [["multiSignatureAsset", { min: 2, publicKeys }], ["senderPublicKey", publicKeys[0]], ["nonce", "1"], ["fee", fee("multiSignature")], ...participants.map((p, i) => ["multiSign", p, i])],
                participants[0],
            ),
        );
        list.push(build("ipfs", [["ipfsAsset", "QmR45FmbVVrixReBwJkhEKde2qwHYaQzGxu4ZoDeswuF9w"], ["nonce", "6"], ["fee", fee("ipfs")]], C));
        list.push(build("htlcLock", [["lockAsset", { secretHash, expiration: { type: 2, value: 5000 } }], ["recipientId", R], ["amount", "1000"], ["nonce", "7"], ["fee", fee("htlcLock")]], A));
        list.push(build("htlcClaim", [["claimAsset", { lockTransactionId: "b".repeat(64), unlockSecret: "c".repeat(64), hashType: 0 }], ["nonce", "7"], ["fee", fee("htlcClaim")]], B));
        list.push(build("htlcRefund", [["refundAsset", { lockTransactionId: "b".repeat(64) }], ["nonce", "7"], ["fee", fee("htlcRefund")]], C));
        return list;
    };
    const removed = removedTypes(() => FEE);
    await add("removed types as bytes", { steps: [process(removed)] });
    await add("removed types as JSON", { steps: [process(removed.map((hex) => ({ json: json(hex) })))] });
    // With a null id the reference names each entry by the id it computes, Heartwood by its index;
    // the type group is written out, as the judge finds a removed type in JSON by it.
    await add("removed types as JSON with a null id", { steps: [process(removed.map((hex) => ({ json: { ...json(hex), typeGroup: 1, id: null } })))] });

    // Removed types in the static-fee mode: the reference compares the fee with the type's static
    // fee before the handler refuses the type, and a fee equal to it goes on to the full pool, the
    // per-sender limit and the size limit. Another network byte is refused when the bytes are read.
    // A multi-signature registration's static fee is the key's times its keys plus one.
    const staticOf = (name) => BigInt(devnet.config.milestones[0].fees.staticFees[name]) * (name === "multiSignature" ? BigInt(publicKeys.length + 1) : 1n);
    const withStaticFee = (delta) => removedTypes((name) => String(staticOf(name) + delta));
    const staticMode = { milestones: feesOff, options: { dynamicFees: { enabled: false } } };
    // A type whose static fee is 0 has no lower fee.
    const lowerFee = removedTypes((name) => String(staticOf(name) > 0n ? staticOf(name) - 1n : 0n)).filter((_, i) => staticOf(REMOVED_NAMES[i]) > 0n);
    await add("removed types in the static-fee mode: a lower, a higher and the exact fee", {
        ...staticMode,
        steps: [process(lowerFee), process(withStaticFee(1n), T0 + 1000), process(withStaticFee(0n), T0 + 2000)],
    });
    const exact = withStaticFee(0n);
    const transferAt = (from, nonce) => transfer(from, nonce, staticFee);
    await add("removed types at the static fee in a full pool", {
        ...staticMode,
        options: { ...staticMode.options, maxTransactionsInPool: 1 },
        steps: [process([transferAt(A, 6)]), process(exact, T0 + 1000)],
    });
    await add("removed types at the static fee over the per-sender limit", {
        ...staticMode,
        options: { ...staticMode.options, maxTransactionsPerSender: 1 },
        steps: [process([transferAt(A, 6), transferAt(B, 6), transferAt(C, 6), transferAt(participants[0], 1)]), process(exact, T0 + 1000)],
        fixture: [...base(), wallet(participants[0], { n: "0" })],
    });
    await add("removed types at the static fee over the size limit", { ...staticMode, options: { ...staticMode.options, maxTransactionBytes: 100 }, steps: [process(exact)] });
    const otherNetwork = (hex, i) =>
        craft.resign(hex, REMOVED_SIGNERS[i], (u) => {
            const b = Buffer.from(u);
            b[2] = 30;
            return b;
        });
    await add("removed types at the static fee from another network", { ...staticMode, steps: [process(exact.map(otherNetwork))] });

    // JSON entries.
    const withId = json(transfer(A, 6));
    const wrongId = { ...json(transfer(B, 6)), id: "00".repeat(32) };
    const noId = json(transfer(C, 6));
    delete noId.id;
    await add("JSON entries with a correct, a wrong and no id", { steps: [process([{ json: withId }, { json: wrongId }, { json: noId }])] });
    await add("JSON entries refused in both loops", { steps: [process([{ json: { ...withId, fee: "1" } }, { json: { ...noId, fee: "1" } }, { json: { type: 6 } }])] });

    // Per-sender views: a pending incoming transfer does not fund its recipient.
    await add("a pending incoming transfer does not fund its recipient", {
        fixture: poor,
        steps: [process([transfer(A, 6, FEE, "900000000", address("v21 poor")), transfer("v21 poor", 1, FEE, "500000000", R)])],
    });

    // Re-adds, restarts and age.
    const three = process([transfer(A, 6), transfer(A, 7), transfer(B, 6)]);
    await add("re-add after the storage is kept, after a flush", { steps: [three, { op: "readd" }, { op: "readd", recheck: true }, { op: "flush" }, { op: "readd" }] });
    await add("a restart of the storage", { steps: [three, { op: "restart" }, process([transfer(A, 7), transfer(C, 6)], T0 + 1000)] });
    await add("age expiry on clean-up and re-add", {
        options: { maxTransactionAge: 2 },
        steps: [three, { op: "tip", height: LAST + 1 }, process([transfer(C, 6)], T0 + 1000), { op: "tip", height: LAST + 2 }, { op: "cleanUp" }, { op: "tip", height: LAST + 3 }, { op: "readd", recheck: true }],
    });
    await add("re-add of previously forged transactions", { steps: [process([transfer(A, 7), transfer(B, 6)]), { op: "readd", forged: [transfer(A, 6), transfer(C, 6)] }] });

    // Removals.
    const chain = [transfer(A, 6), transfer(A, 7), transfer(A, 8), transfer(B, 6)];
    await add("removal cascades", { steps: [process(chain), { op: "remove", hex: chain[1] }, { op: "remove", hex: chain[3] }, { op: "remove", hex: transfer(C, 6) }] });
    await add("forged removal", { steps: [process(chain), { op: "removeForged", hex: chain[1] }, { op: "removeForged", hex: transfer(C, 6) }] });

    // Collation.
    const mixed = process([transfer(A, 6, 20000000), transfer(A, 7, 90000000), transfer(B, 6, 50000000), transfer(C, 6, 50000000), transfer(C, 7, 10000000)]);
    await add("collation: fee order, nonce order, ties", { steps: [mixed, { op: "collate", validate: false }, { op: "collate", validate: true }] });
    await add("collation: exclude", { steps: [mixed, { op: "collate", validate: true, exclude: [idOf(transfer(B, 6, 50000000)), idOf(transfer(A, 7, 90000000))] }] });
    await add("collation: maxTransactions", { milestones: milestonesWith((m) => ({ ...m, block: { ...m.block, maxTransactions: 2 } })), steps: [mixed, { op: "collate", validate: true }] });
    const size = Buffer.from(transfer(A, 6), "hex").length;
    // The budget is the tip's milestone, the rules those of the next height.
    const block = devnet.config.milestones[0].block;
    const budgetAtTip = JSON.stringify([
        ...devnet.config.milestones,
        { height: LAST, block: { ...block, maxPayload: 141 + 2 * (4 + size) + 3 } },
        { height: H, block },
    ]);
    await add("collation: the byte budget of the tip stops the collation", { milestones: budgetAtTip, steps: [mixed, { op: "collate", validate: false }] });
    // Each resignation passes on its sender's own view; on the one copy of the collation, the second
    // leaves too few delegates.
    const resignWithFee = (i, nonce, fee) => build("delegateResignation", [["resignationTypeAsset", 0], ["nonce", String(nonce)], ["fee", fee]], `v21 delegate ${i}`);
    await add("collation: a failed sender is skipped and removed", {
        fixture: many(),
        steps: [
            process([resignWithFee(1, 2, "50000000"), resignWithFee(2, 2, "40000000"), transfer("v21 delegate 2", 3, 90000000), transfer(B, 6, 30000000)]),
            { op: "collate", validate: false },
            { op: "collate", validate: true },
            { op: "collate", validate: true },
        ],
    });
    return out;
};
