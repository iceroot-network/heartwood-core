"use strict";
// Oracle operations: one function per Solar call, shared by gen-vectors.js (the golden vectors of
// level L0) and oracle-server.js (the per-case requests of level L1), so a golden-vector record and
// an oracle response for the same input are produced by the same code. Every operation calls the
// reference; nothing here re-implements Solar logic.
//
// Inputs and outputs are plain JSON: bytes are lowercase hex, amounts are decimal strings.

const FIXED_AUX = Buffer.alloc(32, 0x42);

/**
 * Make BIP340 signing deterministic inside this process only: Solar's transaction signer calls
 * Hash.signSchnorrBip340 without aux, and bcrypto then draws random aux. The wrapper supplies
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

function hexBuffer(value, name) {
    if (typeof value !== "string" || !/^([0-9a-f]{2})*$/.test(value)) {
        throw new TypeError(`${name} must be lowercase hex with an even length`);
    }
    return Buffer.from(value, "hex");
}

// Builder methods a tx.build call list may use. sign/secondSign are separate input fields.
const TX_BUILDERS = ["transfer", "secondSignature", "delegateRegistration", "delegateResignation", "burn", "vote", "ipfs"];
const TX_METHODS = new Set([
    "addTransfer",
    "amount",
    "fee",
    "ipfsAsset",
    "memo",
    "nonce",
    "recipientId",
    "resignationTypeAsset",
    "signatureAsset",
    "usernameAsset",
    "votesAsset",
]);

function makeOps(solar, devnet) {
    const { Blocks, Crypto, Identities, Managers, Transactions } = solar.lib;
    const cm = Managers.configManager;
    installFixedAux(solar);

    /**
     * Run fn with the rule height set. Solar reads a process-global height; the patched reference
     * (patch 25 of its series) adds configManager.runAtHeight, which is used when it exists.
     */
    function atHeight(height, fn) {
        cm.setHeight(height);
        return typeof cm.runAtHeight === "function" ? cm.runAtHeight(height, fn) : fn();
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
            return { ok: false, error: errorInfo(error) };
        }
    }

    /**
     * id, the signed header, the JSON form and the verification result. block.make adds the full
     * bytes; BlockFactory.fromBytes leaves block.serialised without the transactions, so the
     * decode direction reports the header only and the transactions through json.
     */
    function blockOutput(block, withFullBytes) {
        const verification = {
            verified: block.verification.verified,
            containsMultiSignatures: block.verification.containsMultiSignatures,
            errors: block.verification.errors.map((e) => {
                if (typeof e === "string") return e;
                const info = errorInfo(e);
                return `${info.class}: ${info.message}`;
            }),
        };
        const out = { id: block.data.id, headerHex: Blocks.Serialiser.serialise(block.data, true).toString("hex") };
        if (withFullBytes) out.hex = block.serialised;
        return { ...out, json: plain(block.toJson()), verification };
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

        // A 32-byte message is signed as given, any other length is SHA-256 hashed first (hash.ts).
        "sig.bip340.sign": ({ message, privateKey, aux }) => {
            const keys = Identities.Keys.fromPrivateKey(privateKey);
            return { signature: Crypto.Hash.signSchnorrBip340(hexBuffer(message, "message"), keys, hexBuffer(aux, "aux")) };
        },

        "sig.bip340.verify": ({ message, signature, publicKey }) => ({
            valid: Crypto.Hash.verifySchnorrBip340(hexBuffer(message, "message"), signature, publicKey),
        }),

        // Solar's pre-BIP340 Schnorr (bcrypto secp256k1.schnorrSign): deterministic, no aux.
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

        "tx.fromBytes": ({ hex, strict = true }) => {
            const tx = Transactions.TransactionFactory.fromBytes(hexBuffer(hex, "hex"), strict);
            return txOutput(tx);
        },

        // A block shaped exactly as the forger makes it (packages/forger/src/delegate.ts createBlock).
        "block.make": ({ height, timestamp, previousBlock, reward, passphrase, aux, transactions = [] }) => {
            const keys = Identities.Keys.fromPassphrase(passphrase);
            const txs = transactions.map((hex) => Transactions.TransactionFactory.fromBytes(hexBuffer(hex, "transaction"), true).data);
            let totalAmount = solar.lib.Utils.BigNumber.ZERO;
            let totalFee = solar.lib.Utils.BigNumber.ZERO;
            for (const t of txs) {
                totalAmount = totalAmount.plus(t.amount || solar.lib.Utils.BigNumber.ZERO);
                totalFee = totalFee.plus(t.fee);
            }
            const block = Blocks.BlockFactory.make(
                {
                    version: 0,
                    timestamp,
                    previousBlock,
                    height,
                    numberOfTransactions: txs.length,
                    totalAmount,
                    totalFee,
                    reward: solar.lib.Utils.BigNumber.make(reward),
                    payloadLength: 32 * txs.length,
                    payloadHash: Crypto.HashAlgorithms.sha256(txs.map((t) => Buffer.from(t.id, "hex"))).toString("hex"),
                    transactions: txs,
                },
                keys,
                hexBuffer(aux, "aux"),
            );
            return blockOutput(block, true);
        },

        "block.fromBytes": ({ hex }) => blockOutput(Blocks.BlockFactory.fromBytes(hexBuffer(hex, "hex")), false),
    };

    /** Run one operation at a height. Returns {output} or {error}; never throws for Solar errors. */
    function run(op, height, input) {
        const fn = Object.prototype.hasOwnProperty.call(ops, op) ? ops[op] : undefined;
        if (!fn) throw new Error(`unknown operation ${op}`);
        try {
            return { output: atHeight(height, () => fn(input || {})) };
        } catch (error) {
            return { error: errorInfo(error) };
        }
    }

    return { ops, run, atHeight };
}

module.exports = { FIXED_AUX, errorInfo, makeOps };
