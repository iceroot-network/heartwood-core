"use strict";
// Inputs for refusal and non-canonical cases: transaction and block bytes that the reference's
// builders will not produce. Everything is signed and serialised with the reference's own Signer,
// Serialiser and Hash wherever the reference can express the case; only the bytes it cannot write
// (a vote asset over 1,024 bytes, duplicate vote names, an invalid UTF-8 memo, a stray resignation
// byte, a block that carries such a transaction as sent) are spliced in by hand, and then signed
// with the reference's BIP340 signer over the chosen preimage. Nothing here decides a verdict: every
// crafted input goes back through an oracle operation.

const crypto = require("crypto");

const { FIXED_AUX } = require("./ops");

// A standard header: 0xff - headerType, version, network, typeGroup (u32), type (u16), nonce (u64),
// sender public key (33), fee (u64). An extended header adds the 21-byte sender address.
const STANDARD_HEADER = 58;
const EXTENDED_EXTRA = 21;
const SIGNATURE = 64;

function sha256(data) {
    return crypto.createHash("sha256").update(data).digest();
}

function makeCrafter(solar) {
    const { Blocks, Crypto, Identities, Transactions, Utils } = solar.lib;

    /** A builder's data after the calls and a signature, as the base of a crafted transaction. */
    function builderData(builder, calls, passphrase) {
        let b = Transactions.BuilderFactory[builder]();
        for (const [method, ...args] of calls) b = b[method](...args);
        b = b.sign(passphrase);
        return b.data;
    }

    /**
     * Sign data with the reference's Signer (first signature, then an optional second one) and
     * serialise it with the reference's Serialiser, without any schema check.
     */
    function signAndSerialise(data, passphrase, secondPassphrase) {
        const copy = { ...data };
        delete copy.signature;
        delete copy.secondSignature;
        delete copy.id;
        const keys = Identities.Keys.fromPassphrase(passphrase);
        copy.senderPublicKey = keys.publicKey.secp256k1;
        Transactions.Signer.sign(copy, keys);
        if (secondPassphrase !== undefined) Transactions.Signer.secondSign(copy, Identities.Keys.fromPassphrase(secondPassphrase));
        return Transactions.Serialiser.serialise(Transactions.TransactionTypeFactory.create(copy));
    }

    /** Where the asset starts in canonical bytes: after the header and the memo. */
    function assetOffset(bytes) {
        const header = STANDARD_HEADER + (0xff - bytes[0] === 1 ? EXTENDED_EXTRA : 0);
        return header + 1 + bytes[header];
    }

    /** The BIP340 signature (fixed aux) of SHA-256 over the preimage, as 64 bytes. */
    function signPreimage(preimage, passphrase) {
        const keys = Identities.Keys.fromPassphrase(passphrase);
        return Buffer.from(Crypto.Hash.signSchnorrBip340(sha256(preimage), keys, FIXED_AUX), "hex");
    }

    /**
     * Replace the unsigned part of a signed canonical transaction (one signature at the end) and
     * sign the new unsigned bytes. `edit` gets the unsigned bytes and returns the replacement.
     */
    function resign(hex, passphrase, edit) {
        const bytes = Buffer.from(hex, "hex");
        const unsigned = edit(bytes.subarray(0, bytes.length - SIGNATURE));
        return Buffer.concat([unsigned, signPreimage(unsigned, passphrase)]).toString("hex");
    }

    /**
     * A transaction with the canonical header and memo of `hex` and the given asset bytes, signed
     * over `signOver` ("wire": the bytes as sent; or a hex string: another preimage, for example
     * the canonical form the reference re-serialises the decoded data to).
     */
    function withAsset(hex, asset, passphrase, signOver = "wire") {
        const bytes = Buffer.from(hex, "hex");
        const unsigned = Buffer.concat([bytes.subarray(0, assetOffset(bytes)), asset]);
        const preimage = signOver === "wire" ? unsigned : Buffer.from(signOver, "hex");
        return Buffer.concat([unsigned, signPreimage(preimage, passphrase)]).toString("hex");
    }

    /** A type group 2 vote asset as the codec lays it out, from [name, basis points] entries (a name is text or raw bytes). */
    function voteAsset(entries) {
        const parts = [Buffer.from([entries.length])];
        for (const [name, basisPoints] of entries) {
            const nameBytes = Buffer.isBuffer(name) ? name : Buffer.from(name, "utf8");
            const bp = Buffer.alloc(2);
            bp.writeUInt16LE(basisPoints);
            parts.push(Buffer.from([nameBytes.length]), nameBytes, bp);
        }
        return Buffer.concat(parts);
    }

    /** Canonical bytes with the memo replaced by raw bytes (the length byte follows them). */
    function withMemoBytes(hex, memo, passphrase, signOver = "wire") {
        const bytes = Buffer.from(hex, "hex");
        const header = STANDARD_HEADER + (0xff - bytes[0] === 1 ? EXTENDED_EXTRA : 0);
        const rest = bytes.subarray(assetOffset(bytes), bytes.length - SIGNATURE);
        const unsigned = Buffer.concat([bytes.subarray(0, header), Buffer.from([memo.length]), memo, rest]);
        const preimage = signOver === "wire" ? unsigned : Buffer.from(signOver, "hex");
        return Buffer.concat([unsigned, signPreimage(preimage, passphrase)]).toString("hex");
    }

    /**
     * Block bytes that carry the given transaction bytes as they are, which BlockFactory cannot
     * write: it serialises every transaction again. The header is the reference's, signed with its
     * BIP340 signer and `aux`; its totals are the transactions' as fromBytesUnsafe reads them, and
     * its payload hash is SHA-256 over `ids`, one per transaction.
     */
    function blockBytes({ height, timestamp, previousBlock, reward, passphrase, aux, transactions, ids }) {
        const keys = Identities.Keys.fromPassphrase(passphrase);
        const txs = transactions.map((hex) => Transactions.TransactionFactory.fromBytesUnsafe(Buffer.from(hex, "hex")).data);
        let totalAmount = Utils.BigNumber.ZERO;
        let totalFee = Utils.BigNumber.ZERO;
        for (const t of txs) {
            totalAmount = totalAmount.plus(t.amount || Utils.BigNumber.ZERO);
            totalFee = totalFee.plus(t.fee);
        }
        const data = {
            version: 0,
            timestamp,
            height,
            previousBlock,
            numberOfTransactions: transactions.length,
            totalAmount,
            totalFee,
            reward: Utils.BigNumber.make(reward),
            payloadLength: 32 * transactions.length,
            payloadHash: sha256(Buffer.concat(ids.map((id) => Buffer.from(id, "hex")))).toString("hex"),
            generatorPublicKey: keys.publicKey.secp256k1,
        };
        data.blockSignature = Crypto.Hash.signSchnorrBip340(sha256(Blocks.Serialiser.serialise(data, false)), keys, Buffer.from(aux, "hex"));
        const lengths = transactions.map((hex) => {
            const length = Buffer.alloc(4);
            length.writeUInt32LE(hex.length / 2);
            return length;
        });
        return Buffer.concat([Blocks.Serialiser.serialise(data, true), ...lengths, ...transactions.map((hex) => Buffer.from(hex, "hex"))]).toString("hex");
    }

    /** Base58Check of a raw payload, with the reference's encoder. */
    function base58Check(payload) {
        return Utils.Base58.encodeCheck(payload);
    }

    return { assetOffset, base58Check, blockBytes, builderData, resign, signAndSerialise, signPreimage, voteAsset, withAsset, withMemoBytes };
}

module.exports = { SIGNATURE, makeCrafter, sha256 };
