"use strict";
// V17-handlers: state.applyTransaction on wallet fixtures, at least one case per refusal of the
// handlers and per effect of apply and revert. Each record holds the fixture, the transaction
// bytes and the last block's height, and what the reference did: the #W section after the apply
// and after the revert, or the refusal's class and the #W section after the refused attempt.
//
// The cases cover the transfer (items, repeated recipients, a voter's recipient, the first
// transaction of a wallet without a key), the fee floor at the floor and one below it, the nonce
// on both sides, the balance, the second signature (missing, wrong, unexpected, correct), the
// second-signature and delegate registrations with their refusals, every resignation transition
// with the revoke delay at 105, 106 and 107 blocks and the delegate minimum, the burn at the
// minimum and one below it, and votes (single, split with a remainder, change, unvote, identical,
// empty, too many under a small active-delegate count, unknown and resigned delegates). One case
// is a refused first transaction of a wallet without a key: the reference binds the key before
// its checks, and the record shows the key bound after the refusal.
//
// Every transaction is built with the reference's builders and signed with the fixed aux.

const HEIGHT = 1000; // the block the transactions are applied in: lastBlockHeight is HEIGHT - 1
const BALANCE = "100000000000000"; // 1,000,000 coins
const DELEGATES = 54; // one more than the active delegates, so one may resign

module.exports = async function handlersState({ solar, devnet, rec, must }) {
    const { Identities, Transactions } = solar.lib;
    const out = [];
    const last = HEIGHT - 1;
    const key = (passphrase) => Identities.Keys.fromPassphrase(passphrase).publicKey.secp256k1;
    const address = (passphrase) => Identities.Address.fromPublicKey(key(passphrase), 90);

    // Wallets, by passphrase.
    const wallet = (passphrase, extra = {}) => ({ a: address(passphrase), pk: key(passphrase), bal: BALANCE, n: "5", ...extra });
    const delegate = (i, extra = {}) =>
        wallet(`v17 delegate ${i}`, { n: "1", bal: "1000000000", u: `v17d${i}`, pb: 3, ff: "300", fr: "600000000", bf: "270", dn: "60000000", lb: "ab".repeat(32), ...extra });
    const delegates = (n, overrides = {}) => Array.from({ length: n }, (_, i) => delegate(i + 1, overrides[i + 1] || {}));
    const sender = () => wallet("v17 sender");
    const voter = () => wallet("v17 voter", { v: [["v17d1", 10000]] });
    const second = () => wallet("v17 second", { spk: key("v17 second secret") });
    const fresh = () => ({ a: address("v17 fresh"), bal: BALANCE, n: "0" });
    const base = () => [sender(), voter(), second(), fresh(), ...delegates(3)];

    // A transaction built with the reference's builder: calls, then the signature(s).
    const build = (builder, calls, passphrase, secondPassphrase) => {
        let b = Transactions.BuilderFactory[builder]();
        for (const [method, ...args] of calls) b = b[method](...args);
        b = b.sign(passphrase);
        if (secondPassphrase !== undefined) b = b.secondSign(secondPassphrase);
        return b.build().serialised.toString("hex");
    };
    const FEE = ["fee", "50000000"];

    const add = async (name, fixture, hex, options = {}) => {
        const input = { fixture, hex, lastBlockHeight: options.lastBlockHeight ?? last };
        if (options.milestones !== undefined) input.milestones = options.milestones;
        const r = must(await rec("state.applyTransaction", (options.lastBlockHeight ?? last) + 1, input));
        out.push({ ...r, name });
    };
    const minimumFee = async (hex) => BigInt(must(await rec("fee.minimum", HEIGHT, { hex })).output.minimumFee);

    // Transfers.
    const R = [1, 2, 3].map((i) => address(`v17 recipient ${i}`));
    await add("transfer one item", base(), build("transfer", [["addTransfer", R[0], "1000"], ["nonce", "6"], FEE], "v17 sender"));
    await add(
        "transfer repeated recipients",
        base(),
        build("transfer", [["addTransfer", R[0], "1000"], ["addTransfer", R[0], "2000"], ["addTransfer", R[1], "3"], ["nonce", "6"], FEE], "v17 sender"),
    );
    await add("transfer to itself", base(), build("transfer", [["addTransfer", address("v17 sender"), "77"], ["nonce", "6"], FEE], "v17 sender"));
    await add("transfer to a voter", base(), build("transfer", [["addTransfer", address("v17 voter"), "123456789"], ["nonce", "6"], FEE], "v17 sender"));
    await add("transfer from a voter", base(), build("transfer", [["addTransfer", R[2], "987654321"], ["nonce", "6"], FEE], "v17 voter"));
    await add("transfer to a delegate", base(), build("transfer", [["addTransfer", address("v17 delegate 1"), "5"], ["nonce", "6"], FEE], "v17 sender"));
    await add(
        "transfer items over the balance",
        base(),
        build("transfer", [["addTransfer", R[0], BALANCE], ["nonce", "6"], FEE], "v17 sender"),
    );
    await add("transfer fee over the balance", [...base(), wallet("v17 poor", { bal: "49999999" })], build("transfer", [["addTransfer", R[0], "1"], ["nonce", "6"], FEE], "v17 poor"));
    await add("transfer nonce too high", base(), build("transfer", [["addTransfer", R[0], "1"], ["nonce", "7"], FEE], "v17 sender"));
    await add("transfer nonce not above the wallet's", base(), build("transfer", [["addTransfer", R[0], "1"], ["nonce", "5"], FEE], "v17 sender"));
    await add("first transaction of a wallet without a key", base(), build("transfer", [["addTransfer", R[0], "1"], ["nonce", "1"], FEE], "v17 fresh"));
    await add(
        "refused first transaction of a wallet without a key",
        base(),
        build("transfer", [["addTransfer", R[0], "1"], ["nonce", "2"], FEE], "v17 fresh"),
    );
    await add(
        "refused first transaction of a wallet without a key or balance",
        [...base(), { a: address("v17 empty"), bal: "0", n: "0" }],
        build("transfer", [["addTransfer", R[0], "1"], ["nonce", "1"], FEE], "v17 empty"),
    );

    // The fee floor, at the floor and one below it (the fee does not change the size).
    const probe = build("transfer", [["addTransfer", R[0], "1"], ["nonce", "6"], ["fee", "1"]], "v17 sender");
    const floor = await minimumFee(probe);
    await add("transfer at the fee floor", base(), build("transfer", [["addTransfer", R[0], "1"], ["nonce", "6"], ["fee", String(floor)]], "v17 sender"));
    await add("transfer one below the fee floor", base(), build("transfer", [["addTransfer", R[0], "1"], ["nonce", "6"], ["fee", String(floor - 1n)]], "v17 sender"));
    const regProbe = build("delegateRegistration", [["usernameAsset", "floorname"], ["nonce", "6"], ["fee", "1"]], "v17 sender");
    const regFloor = await minimumFee(regProbe);
    await add("registration at the fee floor", base(), build("delegateRegistration", [["usernameAsset", "floorname"], ["nonce", "6"], ["fee", String(regFloor)]], "v17 sender"));
    await add(
        "registration one below the fee floor",
        base(),
        build("delegateRegistration", [["usernameAsset", "floorname"], ["nonce", "6"], ["fee", String(regFloor - 1n)]], "v17 sender"),
    );

    // Second signatures.
    await add("unexpected second signature", base(), build("transfer", [["addTransfer", R[0], "1"], ["nonce", "6"], FEE], "v17 sender", "v17 other secret"));
    await add("missing second signature", base(), build("transfer", [["addTransfer", R[0], "1"], ["nonce", "6"], FEE], "v17 second"));
    await add("wrong second signature", base(), build("transfer", [["addTransfer", R[0], "1"], ["nonce", "6"], FEE], "v17 second", "v17 wrong secret"));
    await add("correct second signature", base(), build("transfer", [["addTransfer", R[0], "1"], ["nonce", "6"], FEE], "v17 second", "v17 second secret"));
    await add("second-signature registration", base(), build("secondSignature", [["signatureAsset", "v17 new secret"], ["nonce", "6"], FEE], "v17 sender"));
    await add(
        "second-signature registration by a wallet that has one",
        base(),
        build("secondSignature", [["signatureAsset", "v17 new secret"], ["nonce", "6"], FEE], "v17 second"),
    );
    await add("second key equal to the sender's key", base(), build("secondSignature", [["signatureAsset", "v17 sender"], ["nonce", "6"], FEE], "v17 sender"));

    // Delegate registrations.
    const REG_FEE = ["fee", "7600000000"];
    await add("delegate registration", base(), build("delegateRegistration", [["usernameAsset", "v17new"], ["nonce", "6"], REG_FEE], "v17 sender"));
    await add("delegate registration by a delegate", base(), build("delegateRegistration", [["usernameAsset", "v17other"], ["nonce", "2"], REG_FEE], "v17 delegate 1"));
    await add("delegate registration of a taken name", base(), build("delegateRegistration", [["usernameAsset", "v17d2"], ["nonce", "6"], REG_FEE], "v17 sender"));
    await add("delegate registration by a voter", base(), build("delegateRegistration", [["usernameAsset", "v17voter"], ["nonce", "6"], REG_FEE], "v17 voter"));

    // Resignations, with one delegate more than the active delegates.
    const TEMP = 0;
    const PERM = 1;
    const REVOKE = 2;
    const resign = (i, type, nonce = "2") => build("delegateResignation", [["resignationTypeAsset", type], ["nonce", nonce], FEE], `v17 delegate ${i}`);
    const many = (overrides = {}) => [sender(), voter(), ...delegates(DELEGATES, overrides)];
    await add("temporary resignation", many(), resign(1, TEMP));
    await add("permanent resignation", many(), resign(1, PERM));
    await add("resignation of a wallet that is no delegate", many(), build("delegateResignation", [["resignationTypeAsset", TEMP], ["nonce", "6"], FEE], "v17 sender"));
    await add("revoke without a resignation", many(), resign(1, REVOKE));
    const perm = { 2: { rs: PERM, resignedAt: 500 } };
    await add("permanent resignation of a permanently resigned delegate", many(perm), resign(2, PERM));
    await add("temporary resignation of a permanently resigned delegate", many(perm), resign(2, TEMP));
    await add("revoke of a permanent resignation", many(perm), resign(2, REVOKE));
    const temp = (at) => ({ 3: { rs: TEMP, resignedAt: at } });
    await add("temporary resignation of a temporarily resigned delegate", many(temp(500)), resign(3, TEMP));
    await add("permanent resignation of a temporarily resigned delegate", many(temp(500)), resign(3, PERM));
    for (const blocks of [105, 106, 107]) {
        await add(`revoke ${blocks} blocks after the resignation`, many(temp(last - blocks)), resign(3, REVOKE));
    }
    await add("revoke of a delegate that revoked before", many({ 4: { resignedAt: 100 } }), resign(4, REVOKE));
    await add("resignation that leaves too few delegates", [sender(), ...delegates(53)], resign(1, TEMP));
    await add(
        "resignation with a resigned delegate among the active count",
        [sender(), ...delegates(DELEGATES, { 5: { rs: TEMP, resignedAt: 10 } })],
        resign(1, TEMP),
    );

    // Burns.
    const burnAmount = String(devnet.config.milestones[0].burn.txAmount);
    const below = String(BigInt(burnAmount) - 1n);
    await add("burn at the minimum", base(), build("burn", [["amount", burnAmount], ["nonce", "6"], FEE], "v17 sender"));
    await add("burn one below the minimum", base(), build("burn", [["amount", below], ["nonce", "6"], FEE], "v17 sender"));
    await add("burn over the balance", base(), build("burn", [["amount", BALANCE], ["nonce", "6"], FEE], "v17 sender"));
    await add("burn by a voter", base(), build("burn", [["amount", "1000000000"], ["nonce", "6"], FEE], "v17 voter"));

    // Votes.
    const vote = (votes, passphrase = "v17 sender", nonce = "6") => build("vote", [["votesAsset", votes], ["nonce", nonce], FEE], passphrase);
    await add("vote for one delegate", base(), vote({ v17d1: 100 }));
    await add("vote split with a remainder", [...base(), wallet("v17 odd", { bal: "50000101" })], vote({ v17d3: 33.33, v17d1: 33.33, v17d2: 33.34 }, "v17 odd"));
    await add("vote change", base(), vote({ v17d2: 60, v17d3: 40 }, "v17 voter"));
    await add("unvote", base(), vote({}, "v17 voter"));
    await add("identical vote", base(), vote({ v17d1: 100 }, "v17 voter"));
    await add("same delegates with other shares", base(), vote({ v17d1: 50, v17d2: 50 }, "v17 voter"));
    await add("empty vote without votes", base(), vote({}));
    await add("vote for an unknown name", base(), vote({ v17nobody: 100 }));
    await add("vote for a resigned delegate", delegates(3, { 2: { rs: TEMP, resignedAt: 10 } }).concat([sender()]), vote({ v17d2: 100 }));
    await add("vote by a delegate for itself", base(), vote({ v17d1: 100 }, "v17 delegate 1", "2"));
    const small = JSON.stringify(devnet.config.milestones.map((m, i) => (i === 0 ? { ...m, activeDelegates: 3 } : m)));
    await add("vote for more delegates than are active", base(), vote({ v17d1: 25, v17d2: 25, v17d3: 25, v17d4: 25 }), { milestones: small });
    await add("vote for as many delegates as are active", base(), vote({ v17d1: 33.33, v17d2: 33.33, v17d3: 33.34 }), { milestones: small });
    return out;
};
