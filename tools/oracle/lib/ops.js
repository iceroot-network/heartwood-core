"use strict";
// Oracle operations: one function per call into the reference implementation, shared by
// gen-vectors.js (the golden vectors of level L0) and oracle-server.js (the per-case requests of
// level L1), so a golden-vector record and an oracle response for the same input are produced by
// the same code. Every operation calls the reference; nothing here re-implements its rules.
//
// Inputs and outputs are plain JSON: bytes are lowercase hex, amounts are decimal strings.
// Operations that take `milestones` (JSON text) run under those milestones on the devnet network
// and restore the devnet configuration afterwards.
//
// The operations of the node's other packages (kernel, logger, state, handlers, chains, the pool
// and the message formats) are in lib/ops/*.js, one file per area; makeOps loads every file there
// (see loadOpsFiles).

const crypto = require("crypto");
const fs = require("fs");
const path = require("path");

const FIXED_AUX = Buffer.alloc(32, 0x42);
const ORACLE_DIR = path.resolve(__dirname, "..");
const OPS_DIR = path.join(__dirname, "ops");

/**
 * Make BIP340 signing deterministic inside this process only: the reference's transaction signer
 * calls Hash.signSchnorrBip340 without aux, and bcrypto then draws random aux. The wrapper supplies
 * FIXED_AUX when the caller passed none. It exists only in the oracle, never in a node process.
 */
function installFixedAux(solar) {
    const Hash = solar.lib.Crypto.Hash;
    if (Hash.signSchnorrBip340.fixedAux) return;
    const original = Hash.signSchnorrBip340;
    const wrapped = (hash, keys, aux) => original.call(Hash, hash, keys, aux ?? FIXED_AUX);
    wrapped.fixedAux = true;
    Hash.signSchnorrBip340 = wrapped;
}

function errorInfo(error) {
    const name = error && error.constructor ? error.constructor.name : typeof error;
    return { class: name, message: error && error.message !== undefined ? String(error.message) : String(error) };
}

/** Plain JSON: BigNumber and Buffer values become what JSON.stringify makes of them. */
function plain(value) {
    return JSON.parse(JSON.stringify(value));
}

function sha256Hex(data) {
    return crypto.createHash("sha256").update(data).digest("hex");
}

/** JSON text with the keys of every object sorted by UTF-16 code units, and no whitespace. */
function canonicalJson(value) {
    if (Array.isArray(value)) return `[${value.map(canonicalJson).join(",")}]`;
    if (value !== null && typeof value === "object") {
        const keys = Object.keys(value).sort();
        return `{${keys.map((k) => `${JSON.stringify(k)}:${canonicalJson(value[k])}`).join(",")}}`;
    }
    return JSON.stringify(value);
}

function hexBuffer(value, name) {
    if (typeof value !== "string" || !/^([0-9a-f]{2})*$/.test(value)) {
        throw new TypeError(`${name} must be lowercase hex with an even length`);
    }
    return Buffer.from(value, "hex");
}

function safeInteger(value, name) {
    if (!Number.isSafeInteger(value)) throw new TypeError(`${name} must be a safe integer`);
    return value;
}

function amount(value, name) {
    if (typeof value !== "string" || !/^(0|[1-9][0-9]*)$/.test(value)) {
        throw new TypeError(`${name} must be a decimal integer string`);
    }
    return value;
}

// Builder methods a tx.build call list may use. sign/secondSign are separate input fields.
const TX_BUILDERS = [
    "transfer",
    "secondSignature",
    "delegateRegistration",
    "delegateResignation",
    "burn",
    "vote",
    "ipfs",
    "multiSignature",
    "htlcLock",
    "htlcClaim",
    "htlcRefund",
];
const TX_METHODS = new Set([
    "addTransfer",
    "amount",
    "claimAsset",
    "expiration",
    "fee",
    "ipfsAsset",
    "lockAsset",
    "memo",
    "multiSign",
    "multiSignatureAsset",
    "network",
    "nonce",
    "recipientId",
    "refundAsset",
    "resignationTypeAsset",
    "senderId",
    "senderPublicKey",
    "signatureAsset",
    "usernameAsset",
    "votesAsset",
]);

// The P2P status of the reference signs this plugin list for a node that serves its API on the
// default port, as the reference's P2P layer builds it from the node's app.json.
const DEFAULT_STATUS_PLUGINS = { "@solar-network/api": { enabled: true, port: 6003, estimateTotalCount: true } };

function makeOps(solar, devnet) {
    const { Blocks, Crypto, Errors, Identities, Managers, Transactions, Utils } = solar.lib;
    const cm = Managers.configManager;
    installFixedAux(solar);

    /** Error info with the reference's and the oracle's install paths replaced, so runs agree. */
    function describe(error) {
        const info = errorInfo(error);
        info.message = info.message.split(solar.solarDir).join("<reference>").split(ORACLE_DIR).join("<oracle>");
        return info;
    }

    /**
     * Run fn with the rule height set. The reference reads a process-global height; the patched
     * reference also has configManager.runAtHeight, which scopes it to fn's async chain.
     */
    function atHeight(height, fn) {
        cm.setHeight(height);
        return typeof cm.runAtHeight === "function" ? cm.runAtHeight(height, fn) : fn();
    }

    /** Run fn under the milestones of `text` when it is given, else under the devnet milestones. */
    function withConfig(text, fn) {
        return text === undefined ? fn() : devnet.withMilestones(text, fn);
    }

    function txOutput(tx) {
        return { id: tx.id, hex: tx.serialised.toString("hex"), json: plain(tx.toJson()), isVerified: tx.isVerified };
    }

    /** Strict TransactionFactory.fromBytes: every accept/reject verdict is recorded from it. */
    function strictVerdict(bytes) {
        try {
            const tx = Transactions.TransactionFactory.fromBytes(bytes, true);
            return { ok: true, id: tx.id, isVerified: tx.isVerified };
        } catch (error) {
            return { ok: false, error: describe(error) };
        }
    }

    /** SHA-256 and length of bytes, for outputs too large to keep in a vector file. */
    function digest(bytes) {
        return { sha256: sha256Hex(bytes), length: bytes.length };
    }

    /**
     * id, the signed header, the full bytes (re-encoded from the block's data), the JSON form and
     * the verification result. With `summary`, the full bytes and the JSON form are replaced by
     * their SHA-256 and length: the bytes as bytes, the JSON as canonical text (keys sorted at
     * every level, no whitespace).
     */
    function blockOutput(block, summary) {
        const verification = {
            verified: block.verification.verified,
            containsMultiSignatures: block.verification.containsMultiSignatures,
            errors: block.verification.errors.map((e) => {
                if (typeof e === "string") return e;
                const info = describe(e);
                return `${info.class}: ${info.message}`;
            }),
        };
        const full = Blocks.Serialiser.serialiseWithTransactions({ ...block.data, transactions: block.transactions.map((t) => t.data) });
        const json = plain(block.toJson());
        const out = { id: block.data.id, headerHex: Blocks.Serialiser.serialise(block.data, true).toString("hex") };
        if (summary) {
            out.bytes = digest(full);
            out.jsonText = digest(Buffer.from(canonicalJson(json), "utf8"));
            out.transactions = block.transactions.length;
        } else {
            out.hex = full.toString("hex");
            out.json = json;
        }
        return { ...out, verification };
    }

    // The transaction handlers, as a registry of bare instances (the handlers' activation and fee
    // methods read only the configuration manager).
    let registry;
    function handlerRegistry() {
        if (!registry) {
            const { Handlers } = solar.transactions();
            registry = Object.create(Handlers.Registry.prototype);
            registry.provider = { isRegistrationRequired: () => false, registerHandlers: () => {} };
            registry.handlers = [...Object.values(Handlers.Core), ...Object.values(Handlers.Solar)].map((H) => Object.create(H.prototype));
        }
        return registry;
    }

    // A signed type group 2 vote struct; vote.validate, vote.splits and vote.shares swap in the
    // votes under test and run the strict transaction schema, as TransactionFactory does when it
    // decodes a vote.
    let voteStruct;
    function validateVotes(votes) {
        if (!voteStruct) {
            voteStruct = Transactions.BuilderFactory.vote().votesAsset({ template: 100 }).nonce("1").sign("vote schema template").getStruct();
        }
        const data = { ...voteStruct, asset: { votes } };
        const { error } = Transactions.Verifier.verifySchema(data, true);
        return { error, votes: data.asset.votes };
    }

    function voteAssetBytes(votes) {
        const tx = Transactions.TransactionTypeFactory.create({ version: 3, typeGroup: 2, type: 2, asset: { votes } });
        return tx.serialise().getResult();
    }

    function voteObject(votes) {
        if (votes === null || typeof votes !== "object" || Array.isArray(votes)) throw new TypeError("votes must be an object");
        return votes;
    }

    /** The crypto files of `config.load`, loaded as the configuration driver loads them. */
    function loadCryptoFiles(files) {
        if (files === null || typeof files !== "object" || Array.isArray(files)) throw new TypeError("files must be an object");
        const names = ["genesisBlock", "exceptions", "milestones", "network"];
        for (const [file, text] of Object.entries(files)) {
            if (!names.some((name) => file === `${name}.json`)) throw new TypeError(`files.${file} is not a crypto file`);
            if (typeof text !== "string") throw new TypeError(`files.${file} must be text`);
        }
        if (names.some((name) => files[`${name}.json`] === undefined)) return { fallback: true };
        const parsed = Object.fromEntries(names.map((name) => [name, JSON.parse(files[`${name}.json`])]));
        try {
            cm.setConfig({ network: parsed.network, exceptions: parsed.exceptions, milestones: parsed.milestones, genesisBlock: parsed.genesisBlock });
            return { accepted: true, milestones: plain(cm.getMilestones()) };
        } finally {
            devnet.restore();
        }
    }

    const ops = {
        ping: () => ({
            pong: true,
            solar: { version: solar.version, ...solar.provenance, seriesSha256: solar.seriesSha256 },
            node: process.version,
            icu: process.versions.icu,
            bcrypto: solar.bcryptoBackend,
            network: devnet.info,
        }),

        "keys.fromPassphrase": ({ passphrase }) => {
            const keys = Identities.Keys.fromPassphrase(passphrase);
            return { privateKey: keys.privateKey, publicKey: keys.publicKey.secp256k1 };
        },

        "address.fromPublicKey": ({ publicKey, networkByte }) => ({
            address: Identities.Address.fromPublicKey(publicKey, networkByte),
        }),

        "address.validate": ({ address, networkByte }) => ({ valid: Identities.Address.validate(address, networkByte) }),

        // Base58Check decode against the configured network byte: the payload and the network-byte
        // complaint the serialiser turns into AddressNetworkError. A bad checksum throws.
        "address.toBuffer": ({ address }) => {
            const { addressBuffer, addressError } = Identities.Address.toBuffer(address);
            return { addressBuffer: addressBuffer.toString("hex"), addressError: addressError ?? null };
        },

        // bcrypto's secp256k1.publicKeyVerify: the key encodings the reference accepts.
        "publicKey.verify": ({ publicKey }) => {
            hexBuffer(publicKey, "publicKey");
            return { valid: Identities.PublicKey.verify(publicKey) };
        },

        // A 32-byte message is signed as given, any other length is SHA-256 hashed first.
        "sig.bip340.sign": ({ message, privateKey, aux }) => {
            const keys = Identities.Keys.fromPrivateKey(privateKey);
            return { signature: Crypto.Hash.signSchnorrBip340(hexBuffer(message, "message"), keys, hexBuffer(aux, "aux")) };
        },

        "sig.bip340.verify": ({ message, signature, publicKey }) => ({
            valid: Crypto.Hash.verifySchnorrBip340(hexBuffer(message, "message"), signature, publicKey),
        }),

        // The reference's pre-BIP340 Schnorr (bcrypto secp256k1.schnorrSign): deterministic, no aux.
        "sig.legacy.sign": ({ message, privateKey }) => {
            const keys = Identities.Keys.fromPrivateKey(privateKey);
            return { signature: Crypto.Hash.signSchnorrLegacy(hexBuffer(message, "message"), keys) };
        },

        "sig.legacy.verify": ({ message, signature, publicKey }) => ({
            valid: Crypto.Hash.verifySchnorrLegacy(hexBuffer(message, "message"), signature, publicKey),
        }),

        // Build a transaction the way a wallet does: BuilderFactory.<builder>(), the listed calls
        // in order, then sign (and secondSign). build() uses the non-strict schema (the builders
        // disable the version check), so the strict verdict is recorded separately.
        "tx.build": ({ builder, calls = [], sign, secondSign }) => {
            if (!TX_BUILDERS.includes(builder)) throw new TypeError(`unknown builder ${builder}`);
            let b = Transactions.BuilderFactory[builder]();
            for (const [method, ...args] of calls) {
                if (!TX_METHODS.has(method)) throw new TypeError(`builder method ${method} is not allowed`);
                b = b[method](...args);
            }
            b = b.sign(sign);
            if (secondSign !== undefined) b = b.secondSign(secondSign);
            const tx = b.build();
            return { ...txOutput(tx), strict: strictVerdict(tx.serialised) };
        },

        "tx.fromBytes": ({ hex, strict = true, milestones }) =>
            withConfig(milestones, () => txOutput(Transactions.TransactionFactory.fromBytes(hexBuffer(hex, "hex"), strict))),

        // TransactionFactory.fromJson (strict): the JSON form back to bytes.
        "tx.fromJson": ({ json }) => txOutput(Transactions.TransactionFactory.fromJson(plain(json))),

        // The preimages the three signatures of a transaction cover, from its decoded data: the
        // first signature (signatures excluded), the second signature (the first included) and a
        // multi-signature entry (all signatures excluded). Each hash is SHA-256 of its bytes.
        "tx.preimages": ({ hex }) => {
            const tx = Transactions.Deserialiser.deserialise(hexBuffer(hex, "hex"));
            const bytes = (options) => Transactions.Serialiser.getBytes(tx.data, options);
            const entry = (options) => {
                const b = bytes(options);
                return { bytes: b.toString("hex"), hash: sha256Hex(b) };
            };
            return {
                id: Transactions.Utils.getId(tx.data),
                signature: entry({ excludeSignature: true, excludeSecondSignature: true }),
                secondSignature: entry({ excludeSecondSignature: true }),
                multiSignature: entry({ excludeSignature: true, excludeSecondSignature: true, excludeMultiSignature: true }),
            };
        },

        // Whether the transaction handler of a type is active at the height: the handler registry's
        // getActivatedHandlerForData, which throws DeactivatedTransactionHandlerError otherwise.
        "handler.activation": async ({ typeGroup, type }) => {
            const handler = await handlerRegistry().getActivatedHandlerForData({ typeGroup: safeInteger(typeGroup, "typeGroup"), type: safeInteger(type, "type") });
            return { handler: handler.constructor.name };
        },

        // The dynamic fee floor of the transaction's handler under the milestone at the height:
        // (addonBytes[key] + round(size / 2)) x minFee, where size is the byte length. With
        // `milestones`, under those milestones instead of the devnet's.
        "fee.minimum": ({ hex, milestones }, height) =>
            withConfig(milestones, () =>
                atHeight(height, () => {
                    const tx = Transactions.TransactionFactory.fromBytes(hexBuffer(hex, "hex"), true);
                    const { InternalTransactionType } = Transactions;
                    const handler = handlerRegistry().getRegisteredHandlerByType(InternalTransactionType.from(tx.data.type, tx.data.typeGroup));
                    const minimumFee = handler.getMinimumFee(tx, cm.getMilestone(height).dynamicFees);
                    return { key: tx.key, size: tx.serialised.length, minimumFee: minimumFee.toString() };
                }),
            ),

        // Transaction.setBurnedFee at the height, for a transaction with this fee.
        "fee.burned": ({ fee }, height) => {
            const tx = Transactions.TransactionTypeFactory.create({ version: 3, typeGroup: 1, type: 6, fee: Utils.BigNumber.make(amount(fee, "fee")) });
            tx.setBurnedFee(height);
            return { burnedFee: tx.data.burnedFee.toString() };
        },

        // Utils.calculateReward(height, rank): the rank table's entry or the milestone's reward.
        "reward.calculate": ({ rank }, height) => ({ reward: String(Utils.calculateReward(height, safeInteger(rank, "rank"))) }),

        // Utils.calculateDonations(height, reward): each recipient's share of a block reward.
        "reward.donations": ({ reward }, height) => {
            const donations = Utils.calculateDonations(height, Utils.BigNumber.make(amount(reward, "reward")));
            return { donations: Object.entries(donations).map(([address, value]) => [address, value.toString()]) };
        },

        // The strict schema of a type group 2 vote with these votes. Accepted votes are returned as
        // the schema leaves them (it drops names that do not match the username pattern).
        "vote.validate": ({ votes }) => {
            const { error, votes: after } = validateVotes(voteObject(votes));
            if (error) throw new Errors.TransactionSchemaError(error);
            return { votes: Object.entries(after) };
        },

        // Utils.sortVotes: the order a vote's entries are kept in.
        "vote.sort": ({ votes }) => ({ votes: Object.entries(Utils.sortVotes(voteObject(votes))) }),

        // Every two-way split of 100 in cents, compactly: for a = from..to the votes
        // {names[0]: a / 100, names[1]: (10000 - a) / 100} (JavaScript numbers, as JSON.parse reads
        // the two-decimal text). Output: the refused values of a, and SHA-256 over the verdicts (one
        // character per split, "1" accepted, "0" refused) and over the concatenated vote assets as
        // the vote codec writes them.
        "vote.splits": ({ names, from, to }) => {
            if (!Array.isArray(names) || names.length !== 2 || names[0] === names[1]) throw new TypeError("names must be two different names");
            safeInteger(from, "from");
            safeInteger(to, "to");
            if (from < 1 || to > 9999 || from > to) throw new RangeError("1 <= from <= to <= 9999");
            const verdicts = crypto.createHash("sha256");
            const assets = crypto.createHash("sha256");
            const refused = [];
            for (let a = from; a <= to; a++) {
                const votes = { [names[0]]: a / 100, [names[1]]: (10000 - a) / 100 };
                const bytes = voteAssetBytes({ ...votes });
                const { error } = validateVotes(votes);
                if (error) refused.push(a);
                verdicts.update(error ? "0" : "1");
                assets.update(bytes);
            }
            return {
                splits: to - from + 1,
                accepted: to - from + 1 - refused.length,
                refused,
                verdictsSha256: verdicts.digest("hex"),
                assetsSha256: assets.digest("hex"),
            };
        },

        // The vote schema on shares near every cent: for c = from..to and each k in -ulps..ulps, the
        // double k units in the last place from c / 100 (JavaScript numbers). Each share x is
        // validated as {a: x, b: (10000 - round(x * 100)) / 100}, a vote whose other share is an
        // exact cent, so the verdict is x's: its multipleOf, minimum and maximum (and the sum);
        // when round(x * 100) is 10000 or more, as {a: x}. Output: the accepted count; the accepted
        // shares that are not exact cents, as JavaScript prints them (their count, the first 20,
        // and SHA-256 over all of them, one per line); and SHA-256 over the verdicts (one
        // character per share, "1" accepted, "0" refused).
        "vote.shares": ({ from, to, ulps }) => {
            safeInteger(from, "from");
            safeInteger(to, "to");
            safeInteger(ulps, "ulps");
            if (from < 1 || to > 10000 || from > to || ulps < 0 || ulps > 1000) throw new RangeError("1 <= from <= to <= 10000, 0 <= ulps <= 1000");
            const view = new DataView(new ArrayBuffer(8));
            const step = (x, k) => {
                view.setFloat64(0, x);
                view.setBigUint64(0, view.getBigUint64(0) + BigInt(k));
                return view.getFloat64(0);
            };
            const verdicts = crypto.createHash("sha256");
            let accepted = 0;
            const offCent = [];
            for (let c = from; c <= to; c++) {
                for (let k = -ulps; k <= ulps; k++) {
                    const x = step(c / 100, k);
                    const bp = Math.round(x * 100);
                    const { error } = validateVotes(bp >= 10000 ? { a: x } : { a: x, b: (10000 - bp) / 100 });
                    verdicts.update(error ? "0" : "1");
                    if (!error) {
                        accepted++;
                        if (k !== 0) offCent.push(String(x));
                    }
                }
            }
            return {
                shares: (to - from + 1) * (2 * ulps + 1),
                accepted,
                offCent: { count: offCent.length, first: offCent.slice(0, 20), sha256: sha256Hex(offCent.map((x) => `${x}\n`).join("")) },
                verdictsSha256: verdicts.digest("hex"),
            };
        },

        // The configuration manager's start-up checks and merge (setConfig) on a milestone file,
        // given as JSON text and read with JSON.parse. Accepted: the merged milestones, and the
        // reward (rank table or reward) and donations at the listed heights.
        //
        // With `files` instead, the crypto files of a network as the kernel's configuration
        // driver loads them: `files` maps each of network.json, milestones.json, genesisBlock.json
        // and exceptions.json to its text, and a file left out is missing. When one is missing,
        // the driver loads none of them and the node falls back to its built-in preset:
        // {fallback: true}. Otherwise each text is read with JSON.parse, in the driver's order,
        // and the four go to setConfig as LoadCryptography passes them; accepted: {accepted: true,
        // milestones} with the merged milestones.
        "config.load": ({ files, milestones, rewards = [], donations = [] }) =>
            files !== undefined ? loadCryptoFiles(files) : devnet.withMilestones(milestones, () => ({
                milestones: plain(cm.getMilestones()),
                rewards: rewards.map(([height, rank]) => {
                    try {
                        return [height, rank, String(Utils.calculateReward(safeInteger(height, "height"), safeInteger(rank, "rank")))];
                    } catch (error) {
                        return [height, rank, { error: describe(error) }];
                    }
                }),
                donations: donations.map(([height, reward]) => {
                    const shares = Utils.calculateDonations(safeInteger(height, "height"), Utils.BigNumber.make(amount(reward, "reward")));
                    return [height, reward, Object.entries(shares).map(([address, value]) => [address, value.toString()])];
                }),
            })),

        // A block shaped as the reference's forger makes it, with optional header overrides for
        // refusal cases (payloadLength, totalAmount, version). The header is signed with BIP340 by
        // BlockFactory.make, or with the legacy Schnorr scheme (scheme "legacy") over the same
        // hash, then built with BlockFactory.fromData. The transactions are read without checks
        // (fromBytesUnsafe) only to fill the header; the block factory then validates and decodes
        // them itself at the block's height.
        "block.make": ({ height, timestamp, previousBlock, reward, passphrase, aux, transactions = [], scheme = "bip340", overrides = {}, milestones, summary = false }) =>
            withConfig(milestones, () =>
                atHeight(height, () => {
                    if (scheme !== "bip340" && scheme !== "legacy") throw new TypeError("scheme must be bip340 or legacy");
                    const keys = Identities.Keys.fromPassphrase(passphrase);
                    const txs = transactions.map((hex) => Transactions.TransactionFactory.fromBytesUnsafe(hexBuffer(hex, "transaction")).data);
                    let totalAmount = Utils.BigNumber.ZERO;
                    let totalFee = Utils.BigNumber.ZERO;
                    for (const t of txs) {
                        totalAmount = totalAmount.plus(t.amount || Utils.BigNumber.ZERO);
                        totalFee = totalFee.plus(t.fee);
                    }
                    const data = {
                        version: overrides.version ?? 0,
                        timestamp,
                        previousBlock,
                        height,
                        numberOfTransactions: txs.length,
                        totalAmount: overrides.totalAmount !== undefined ? Utils.BigNumber.make(amount(overrides.totalAmount, "totalAmount")) : totalAmount,
                        totalFee,
                        reward: Utils.BigNumber.make(reward),
                        payloadLength: overrides.payloadLength ?? 32 * txs.length,
                        payloadHash: Crypto.HashAlgorithms.sha256(txs.map((t) => Buffer.from(t.id, "hex"))).toString("hex"),
                        transactions: txs,
                    };
                    let block;
                    if (scheme === "bip340") {
                        block = Blocks.BlockFactory.make(data, keys, hexBuffer(aux, "aux"));
                    } else {
                        data.generatorPublicKey = keys.publicKey.secp256k1;
                        const hash = Crypto.HashAlgorithms.sha256(Blocks.Serialiser.serialise(data, false));
                        data.blockSignature = Crypto.Hash.signSchnorrLegacy(hash, keys);
                        data.id = Blocks.Block.getId(data);
                        block = Blocks.BlockFactory.fromData(data);
                    }
                    if (!block) throw new Error("BlockFactory returned no block");
                    return blockOutput(block, summary);
                }),
            ),

        "block.fromBytes": ({ hex, milestones, summary = false }) =>
            withConfig(milestones, () => blockOutput(Blocks.BlockFactory.fromBytes(hexBuffer(hex, "hex")), summary)),

        // A block as a node accepts it from another node, pushed or downloaded: the P2P layer
        // decodes the bytes with Blocks.Deserialiser (each transaction with the strict
        // TransactionFactory.fromBytes) and passes on the header with the transactions' data, and
        // the blockchain builds the block again with BlockFactory.fromData, which serialises every
        // transaction again from its data and decodes the result. The output is fromData's block.
        "block.receive": ({ hex, milestones, summary = false }) =>
            withConfig(milestones, () => {
                const deserialised = Blocks.Deserialiser.deserialise(hexBuffer(hex, "hex"));
                const data = { ...deserialised.data, transactions: deserialised.transactions.map((t) => t.data) };
                const block = Blocks.BlockFactory.fromData(data);
                if (!block) throw new Error("BlockFactory returned no block");
                return blockOutput(block, summary);
            }),

        // The bytes a relay signs for its P2P status: the kernel's key-sorted stringify of
        // {state, config}, with the block's header as getHeader(false) gives it and the peer config
        // of the devnet network.
        "status.preimage": ({ block, forgingAllowed, currentSlot, version, plugins = DEFAULT_STATUS_PLUGINS }) => {
            const decoded = Blocks.BlockFactory.fromBytes(hexBuffer(block, "block"));
            const status = {
                state: { height: decoded.data.height, forgingAllowed, currentSlot, header: decoded.getHeader(false) },
                config: {
                    version,
                    network: {
                        version: cm.get("network.pubKeyHash"),
                        name: cm.get("network.name"),
                        nethash: cm.get("network.nethash"),
                        explorer: cm.get("network.client.explorer"),
                        token: { name: cm.get("network.client.token"), symbol: cm.get("network.client.symbol") },
                    },
                    plugins,
                },
            };
            return { preimage: Buffer.from(solar.stringify(status), "utf8").toString("hex") };
        },

        // The BIP39 English mnemonic of the entropy (the bip39 package the reference depends on).
        "bip39.entropyToMnemonic": ({ entropy }) => {
            hexBuffer(entropy, "entropy");
            return { mnemonic: solar.bip39.entropyToMnemonic(entropy) };
        },
    };

    // The operations of lib/ops/*.js, with what they share with the operations above.
    const context = {
        solar,
        devnet,
        cm,
        describe,
        atHeight,
        withConfig,
        plain,
        hexBuffer,
        safeInteger,
        amount,
        sha256Hex,
        canonicalJson,
        handlerRegistry,
        txOutput,
        blockOutput,
    };
    loadOpsFiles(ops, context);

    /**
     * Run one operation at a height. Resolves to {output} or {error}; never rejects for errors the
     * reference raises.
     */
    async function run(op, height, input) {
        const fn = Object.prototype.hasOwnProperty.call(ops, op) ? ops[op] : undefined;
        if (!fn) throw new Error(`unknown operation ${op}`);
        // Operations get a copy, so a record's input is never changed by the call it describes.
        const args = input === undefined ? {} : JSON.parse(JSON.stringify(input));
        try {
            return { output: await atHeight(height, () => fn(args, height)) };
        } catch (error) {
            return { error: describe(error) };
        }
    }

    return { ops, run, atHeight, describe };
}

/**
 * Add the operations of every lib/ops/*.js file to `ops`, in file-name order. Each file exports a
 * function that takes `context` (the reference, the devnet and the helpers of makeOps) and returns
 * an object of operations of the same shape as those of makeOps: (input, height) => output, or a
 * promise of it. An operation name may be defined only once.
 */
function loadOpsFiles(ops, context) {
    if (!fs.existsSync(OPS_DIR)) return;
    const files = fs
        .readdirSync(OPS_DIR)
        .filter((f) => f.endsWith(".js"))
        .sort();
    for (const file of files) {
        const extra = require(path.join(OPS_DIR, file))(context);
        for (const [name, fn] of Object.entries(extra)) {
            if (Object.prototype.hasOwnProperty.call(ops, name)) throw new Error(`operation ${name} of lib/ops/${file} is already defined`);
            if (typeof fn !== "function") throw new TypeError(`operation ${name} of lib/ops/${file} is not a function`);
            ops[name] = fn;
        }
    }
}

module.exports = { FIXED_AUX, errorInfo, makeOps, plain, sha256Hex };
