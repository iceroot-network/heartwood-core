"use strict";
// V18-genesis-state: state.digest of the state after the reference's first start on the genesis
// blocks of V10 (all five sections and their hashes); on variants of the first of them that add
// transactions the devnet generator never writes (fees and burns, second keys, resignations and a
// revoke, split votes, a transfer before the issuance record, more issuance records, a generator
// that registers and votes, votes and resignations before their registrations) or that the
// reference refuses to start on (a negative balance, too few delegates for round 1); and of
// wallet fixtures that between them take every shape of a #W record: listed and unlisted
// wallets, keys, second keys, delegates with and without a last block, both resignations and a
// revoke, voters with one delegate and with a split and its remainder, and amounts near
// 2^63 - 1. Needs V10's chain definitions: without the devnet generator the class is not written.

const crypto = require("crypto");

const { FIXED_AUX } = require("../ops");

const CRYPTO_FILES = ["network", "milestones", "genesisBlock", "exceptions"];

/**
 * Variants of the chain definition `definition`: its genesis block with more transactions, each
 * signed with the passphrase of its sender (the generator, a delegate or a wallet of the
 * definition), the block signed again by the generator, and the nethash set to the new payload
 * hash. Returns [name, crypto files, whether the reference starts on it].
 */
function genesisVariants(solar, devnet, definition) {
    const { Blocks, Crypto, Identities, Managers, Transactions, Utils } = solar.lib;
    const files = Object.fromEntries(CRYPTO_FILES.map((f) => [f, JSON.parse(definition.files[`crypto/${f}.json`])]));
    const passphrase = (name) => {
        const text = definition.passphrases[name];
        if (text === undefined) throw new Error(`V18: the chain definition has no key for ${name}`);
        return text;
    };
    const address = (name) => Identities.Address.fromPassphrase(passphrase(name), files.network.pubKeyHash);

    const spec = (builder, calls, sender, nonce, fee) => ({ builder, calls: [...calls, ["nonce", String(nonce)], ["fee", String(fee)]], sender });
    const transfer = (sender, nonce, fee, items) =>
        spec("transfer", items.map(([to, amount]) => ["addTransfer", address(to), String(amount)]), sender, nonce, fee);
    const burn = (sender, nonce, fee, amount) => spec("burn", [["amount", String(amount)]], sender, nonce, fee);
    const secondKey = (sender, nonce, fee, secret) => spec("secondSignature", [["signatureAsset", secret]], sender, nonce, fee);
    const register = (sender, nonce, fee, username) => spec("delegateRegistration", [["usernameAsset", username]], sender, nonce, fee);
    const resign = (sender, nonce, fee, type) => spec("delegateResignation", [["resignationTypeAsset", type]], sender, nonce, fee);
    const vote = (sender, nonce, fee, votes) => spec("vote", [["votesAsset", votes]], sender, nonce, fee);

    // A transaction as a genesis file holds it: the generator's keys, in alphabetical order, with
    // the type group written out and the amount only where a type has one.
    const toJson = (data) => {
        const json = {};
        if (data.amount !== undefined && !Utils.BigNumber.make(data.amount).isZero()) json.amount = data.amount.toString();
        if (data.asset) {
            json.asset = data.asset.transfers
                ? { transfers: data.asset.transfers.map((t) => ({ amount: t.amount.toString(), recipientId: t.recipientId })) }
                : JSON.parse(JSON.stringify(data.asset));
        }
        json.fee = data.fee.toString();
        json.id = data.id;
        json.network = data.network;
        json.nonce = data.nonce.toString();
        json.senderPublicKey = data.senderPublicKey;
        json.signature = data.signature;
        json.type = data.type;
        json.typeGroup = data.typeGroup;
        json.version = data.version;
        return json;
    };
    // The signed data and its id, without the builder's schema check, which refuses a fee of 0
    // outside the configured genesis block (as the generator writes it); the reference's start
    // checks every transaction in its block.
    const build = (s) => {
        let b = Transactions.BuilderFactory[s.builder]();
        for (const [method, ...args] of s.calls) b = b[method](...args);
        const data = b.sign(passphrase(s.sender)).data;
        return toJson({ ...data, id: Transactions.Utils.getId(data).toString() });
    };

    const variant = (milestones, before, after) => {
        try {
            Managers.configManager.setConfig({ network: files.network, milestones: JSON.parse(JSON.stringify(milestones)), exceptions: {}, genesisBlock: { transactions: [] } });
            Managers.configManager.setHeight(1);
            const transactions = [...before.map(build), ...files.genesisBlock.transactions, ...after.map(build)];
            const ids = transactions.map((t) => Buffer.from(t.id, "hex"));
            const payloadHash = crypto.createHash("sha256").update(Buffer.concat(ids)).digest("hex");
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
                generatorPublicKey: Identities.PublicKey.fromPassphrase(passphrase("generator")),
            };
            const hash = crypto.createHash("sha256").update(Blocks.Serialiser.serialise(header, false)).digest();
            header.blockSignature = Crypto.Hash.signSchnorrBip340(hash, Identities.Keys.fromPassphrase(passphrase("generator")), FIXED_AUX);
            const genesisBlock = {
                blockSignature: header.blockSignature,
                generatorPublicKey: header.generatorPublicKey,
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
            return { network: { ...files.network, nethash: payloadHash }, milestones, genesisBlock, exceptions: {} };
        } finally {
            devnet.restore();
        }
    };
    const withActive = (n) => files.milestones.map((m, i) => (i === 0 ? { ...m, activeDelegates: n } : m));

    return [
        [
            "fees, a burn, a second key, split votes and a transfer before the issuance record",
            variant(
                files.milestones,
                [transfer("genesis-1", 1, 11, [["wallet/1", 123456789], ["genesis_1", 1], ["wallet/1", 5]])],
                [
                    burn("genesis-2", 1, 13, 2000000),
                    secondKey("genesis-1", 2, 17, "v18 second secret"),
                    vote("genesis-1", 3, 19, { genesis_1: 33.33, genesis_2: 33.33, genesis_3: 33.34 }),
                    vote("wallet/1", 1, 23, { genesis_5: 50, genesis_4: 50 }),
                ],
            ),
            true,
        ],
        [
            "a second issuance record, both resignations and a revoke, 50 active delegates",
            variant(withActive(50), [], [
                transfer("generator", 2, 0, [["genesis_1", 1000], ["genesis_2", 1000], ["genesis_3", 1000]]),
                resign("genesis_1", 3, 7, 0),
                resign("genesis_2", 3, 7, 1),
                resign("genesis_3", 3, 7, 2),
            ]),
            true,
        ],
        ["the generator registers and votes", variant(files.milestones, [], [register("generator", 2, 0, "zz_generator"), vote("generator", 3, 0, { zz_generator: 100 })]), true],
        [
            "a vote and a resignation before their registration, and a second issuance record",
            variant(
                files.milestones,
                [vote("wallet/1", 1, 1, { latecomer: 100 }), resign("wallet/4", 1, 0, 0)],
                [register("wallet/4", 2, 1, "latecomer"), transfer("generator", 2, 0, [["wallet/1", 1000], ["wallet/4", 1000]])],
            ),
            true,
        ],
        ["refused: a wallet that pays more than it receives", variant(files.milestones, [], [transfer("wallet/3", 1, 1, [["wallet/1", 10]])]), false],
        ["refused: a resignation that leaves too few delegates for round 1", variant(files.milestones, [], [resign("genesis_1", 3, 0, 0)]), false],
    ];
}

module.exports = async function genesisState({ solar, devnet, rec, must, shared }) {
    if (!shared.genesisDefinitions) return null;
    const { Identities } = solar.lib;
    const out = [];
    for (const definition of shared.genesisDefinitions) {
        const crypto = Object.fromEntries(CRYPTO_FILES.map((f) => [f, JSON.parse(definition.files[`crypto/${f}.json`])]));
        out.push({ ...must(await rec("state.digest", 1, { crypto })), name: `genesis ${definition.name}` });
    }
    for (const [name, crypto, starts] of genesisVariants(solar, devnet, shared.genesisDefinitions[0])) {
        const r = await rec("state.digest", 1, { crypto });
        if (Boolean(r.error) === starts) {
            throw new Error(`V18 variant "${name}": the reference ${starts ? "refused it" : "started on it"}: ${JSON.stringify(r.error || {})}`);
        }
        out.push({ ...r, name: `genesis variant: ${name}` });
    }

    const key = (passphrase) => Identities.Keys.fromPassphrase(passphrase).publicKey.secp256k1;
    const address = (passphrase) => Identities.Address.fromPublicKey(key(passphrase), 90);
    const keyed = (passphrase, extra) => ({ a: address(passphrase), pk: key(passphrase), bal: "0", n: "0", ...extra });
    const delegate = (name, extra = {}) =>
        keyed(`v18 ${name}`, { bal: "1000", n: "1", u: name, pb: 0, ff: "0", fr: "0", bf: "0", dn: "0", ...extra });
    const max = "9223372036854775807";

    const fixtures = [
        ["no wallets", []],
        [
            "listing",
            [
                { a: address("v18 balance only"), bal: "1", n: "0" },
                { a: address("v18 nonce only"), bal: "0", n: "1" },
                { a: address("v18 nothing"), bal: "0", n: "0" },
                keyed("v18 key only", {}),
                keyed("v18 second key", { bal: "5", n: "2", spk: key("v18 second secret") }),
            ],
        ],
        [
            "delegates",
            [
                delegate("plain"),
                delegate("forged", { pb: 12, ff: "4000", fr: "2160000000", bf: "3600", dn: "216000000", lb: "cd".repeat(32) }),
                delegate("temporary", { rs: 0, resignedAt: 70 }),
                delegate("permanent", { rs: 1, resignedAt: 71 }),
                delegate("revoked", { resignedAt: 72 }),
                delegate("zero", { bal: "0", n: "0" }),
            ],
        ],
        [
            "votes",
            [
                delegate("alpha"),
                delegate("beta"),
                delegate("gamma"),
                keyed("v18 single", { bal: "1000000", n: "1", v: [["beta", 10000]] }),
                keyed("v18 split", { bal: "101", n: "1", v: [["gamma", 3333], ["alpha", 3333], ["beta", 3334]] }),
                keyed("v18 thirds", { bal: "100", n: "1", v: [["alpha", 3334], ["beta", 3333], ["gamma", 3333]] }),
                delegate("delta", { v: [["alpha", 5000], ["delta", 5000]] }),
            ],
        ],
        [
            "large amounts",
            [
                delegate("big", { bal: "9223372036854775000", ff: max, fr: max, bf: max, dn: max }),
                keyed("v18 big voter", { bal: "807", n: max, v: [["big", 10000]] }),
            ],
        ],
    ];
    for (const [name, fixture] of fixtures) {
        out.push({ ...must(await rec("state.digest", 2, { fixture })), name: `fixture ${name}` });
    }
    return out;
};
