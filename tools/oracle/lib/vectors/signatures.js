"use strict";
// V04 signatures. BIP340 with fixed aux over plain messages and over the preimages the reference
// signs (a transaction's first and second signature, a block header), with a verify matrix of
// mutated signatures, messages and keys, including the prefix-agnostic 33-byte key slice. The legacy
// Schnorr scheme of the P2P status over messages of 0 bytes to 4 KB and over real status preimages,
// with the same matrix and the R = infinity case.

const crypto = require("crypto");

const { FIXED_AUX } = require("../ops");
const { GENERATOR, OTHER, SECOND, SENDER } = require("./constants");

// secp256k1 group order n and field prime p.
const N = BigInt("0xfffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364141");
const P_HEX = "fffffffffffffffffffffffffffffffffffffffffffffffffffffffefffffc2f";
const N_HEX = N.toString(16);

function bytesHex(length, seed) {
    const b = Buffer.alloc(length);
    for (let i = 0; i < length; i++) b[i] = (i * 31 + seed) & 0xff;
    return b.toString("hex");
}

function flipByte(hex, index) {
    const b = Buffer.from(hex, "hex");
    const i = index < 0 ? b.length + index : index;
    b[i] ^= 0x01;
    return b.toString("hex");
}

function hex32(value) {
    return value.toString(16).padStart(64, "0");
}

/**
 * A legacy signature whose verification point R = sG - eP is the point at infinity: for a chosen
 * r, e = SHA-256(r || P || m) mod n (the scheme's challenge, P compressed) and s = e * d mod n.
 */
function infinitySignature(r, publicKey, privateKey, message) {
    const e =
        BigInt("0x" + crypto.createHash("sha256").update(Buffer.concat([Buffer.from(r, "hex"), Buffer.from(publicKey, "hex"), Buffer.from(message, "hex")])).digest("hex")) % N;
    const s = (e * BigInt("0x" + privateKey)) % N;
    return r + hex32(s);
}

module.exports = async function signatures({ solar, rec, must, shared }) {
    const { Identities } = solar.lib;
    const H = shared.height;
    const out = [];
    const aux = FIXED_AUX.toString("hex");
    const keyOf = (passphrase) => {
        const k = Identities.Keys.fromPassphrase(passphrase);
        return { privateKey: k.privateKey, publicKey: k.publicKey.secp256k1 };
    };
    const sender = keyOf(SENDER);
    const generator = keyOf(GENERATOR);
    const other = keyOf(OTHER);
    const second = keyOf(SECOND);

    // ---- BIP340 ------------------------------------------------------------------------------------
    const bip340Verify = async (name, message, signature, publicKey) =>
        out.push({ ...(await rec("sig.bip340.verify", H, { message, signature, publicKey })), name });

    /** The verify matrix of one valid signature. */
    async function bip340Matrix(label, message, signature, key) {
        const pk = key.publicKey;
        await bip340Verify(`${label}: valid`, message, signature, pk);
        await bip340Verify(`${label}: s changed`, message, flipByte(signature, -1), pk);
        await bip340Verify(`${label}: r changed`, message, flipByte(signature, 0), pk);
        await bip340Verify(`${label}: message changed`, message === "" ? "00" : flipByte(message, 0), signature, pk);
        await bip340Verify(`${label}: other key`, message, signature, other.publicKey);
        await bip340Verify(`${label}: x-only key`, message, signature, pk.slice(2));
        for (const prefix of ["02", "03", "05", "00", "ff"]) {
            if (prefix !== pk.slice(0, 2)) await bip340Verify(`${label}: key prefix ${prefix}`, message, signature, prefix + pk.slice(2));
        }
    }

    const messages = [
        ["32 bytes, signed as given", crypto.createHash("sha256").update("heartwood difftest message").digest("hex")],
        ["empty", ""],
        ["1 byte", "00"],
        ["31 bytes", bytesHex(31, 6)],
        ["32 zero bytes", "00".repeat(32)],
        ["33 bytes", bytesHex(33, 1)],
        ["64 bytes", bytesHex(64, 7)],
        ["100 bytes", bytesHex(100, 2)],
    ];
    const auxes = [
        ["fixed aux", aux],
        ["zero aux", "00".repeat(32)],
        ["aux ff", "ff".repeat(32)],
    ];
    for (const [who, key] of [
        ["sender", sender],
        ["generator", generator],
    ]) {
        for (const [what, message] of messages) {
            for (const [auxName, a] of auxes) {
                const s = must(await rec("sig.bip340.sign", H, { message, privateKey: key.privateKey, aux: a }));
                out.push({ ...s, name: `${who}, ${what}, ${auxName}` });
                if (a === aux) await bip340Matrix(`${who}, ${what}`, message, s.output.signature, key);
            }
        }
    }
    // Signature values out of range and of the wrong length.
    const [, m0] = messages[0];
    const good = must(await rec("sig.bip340.sign", H, { message: m0, privateKey: sender.privateKey, aux })).output.signature;
    for (const [name, signature] of [
        ["s = n", good.slice(0, 64) + N_HEX],
        ["s = 0", good.slice(0, 64) + "00".repeat(32)],
        ["r = p", P_HEX + good.slice(64)],
        ["r = 0", "00".repeat(32) + good.slice(64)],
        ["all zero", "00".repeat(64)],
        ["63 bytes", good.slice(0, 126)],
        ["65 bytes", good + "00"],
        ["empty", ""],
    ]) {
        await bip340Verify(`sender, ${name}`, m0, signature, sender.publicKey);
    }
    await bip340Verify("sender, uncompressed key", m0, good, Identities.Keys.fromPrivateKey(sender.privateKey, false).publicKey.secp256k1);

    // The preimages a transaction's signatures cover: signing their hashes as 32-byte messages with
    // the fixed aux gives the transaction's own signatures.
    for (const [name, key, secondKey] of [
        ["transfer-1", sender],
        ["transfer-256", sender],
        ["delegate-registration", sender],
        ["resignation-0", sender],
        ["vote-53", sender],
        ["burn", sender],
        ["transfer-second-signed", keyOf("second signature sender"), second],
        ["vote-second-signed", keyOf("second signature sender"), second],
    ]) {
        const tx = shared.tx[name];
        const pre = must(await rec("tx.preimages", H, { hex: tx.hex }));
        out.push({ ...pre, name });
        const first = must(await rec("sig.bip340.sign", H, { message: pre.output.signature.hash, privateKey: key.privateKey, aux }));
        out.push({ ...first, name: `${name}: first signature` });
        if (first.output.signature !== tx.json.signature) throw new Error(`${name}: the first-signature preimage does not give the transaction's signature`);
        await bip340Matrix(`${name}: first signature`, pre.output.signature.hash, first.output.signature, key);
        if (secondKey) {
            const s2 = must(await rec("sig.bip340.sign", H, { message: pre.output.secondSignature.hash, privateKey: secondKey.privateKey, aux }));
            out.push({ ...s2, name: `${name}: second signature` });
            if (s2.output.signature !== tx.json.secondSignature) throw new Error(`${name}: the second-signature preimage does not give the transaction's second signature`);
            await bip340Matrix(`${name}: second signature`, pre.output.secondSignature.hash, s2.output.signature, secondKey);
        }
    }

    // A network byte 0 is written again as the configured network byte, in the id and in every
    // preimage.
    const zeroNetwork = Buffer.from(shared.tx["transfer-1"].hex, "hex");
    zeroNetwork[2] = 0;
    out.push({ ...must(await rec("tx.preimages", H, { hex: zeroNetwork.toString("hex") })), name: "transfer-1 with network byte 0" });

    // A block header: SHA-256 of the unsigned header (the first 141 bytes) is the signed message.
    const block = must(
        await rec("block.make", 3, { height: 3, timestamp: 16, previousBlock: "ab".repeat(32), reward: "180000000", passphrase: GENERATOR, aux, transactions: [shared.tx["transfer-1"].hex] }),
    );
    const headerHash = crypto.createHash("sha256").update(Buffer.from(block.output.headerHex.slice(0, 282), "hex")).digest("hex");
    const blockSignature = must(await rec("sig.bip340.sign", H, { message: headerHash, privateKey: generator.privateKey, aux }));
    out.push({ ...blockSignature, name: "block header" });
    if (blockSignature.output.signature !== block.output.json.blockSignature) throw new Error("the block header hash does not give the block's signature");
    await bip340Matrix("block header", headerHash, blockSignature.output.signature, generator);

    // ---- legacy Schnorr ----------------------------------------------------------------------------
    let legacyCount = 0;
    const legacy = async (op, name, input) => {
        legacyCount++;
        const r = { ...(await rec(op, H, input)), name };
        out.push(r);
        return r;
    };
    async function legacyCase(label, message, key) {
        const s = await legacy("sig.legacy.sign", label, { message, privateKey: key.privateKey });
        if (s.error) throw new Error(`legacy sign failed for ${label}: ${s.error.message}`);
        const signature = s.output.signature;
        await legacy("sig.legacy.verify", `${label}: valid`, { message, signature, publicKey: key.publicKey });
        await legacy("sig.legacy.verify", `${label}: s changed`, { message, signature: flipByte(signature, -1), publicKey: key.publicKey });
        await legacy("sig.legacy.verify", `${label}: other key`, { message, signature, publicKey: other.publicKey });
        await legacy("sig.legacy.verify", `${label}: message changed`, { message: message === "" ? "00" : flipByte(message, -1), signature, publicKey: key.publicKey });
        return signature;
    }

    const lengths = [];
    for (let n = 0; n <= 33; n++) lengths.push(n);
    lengths.push(63, 64, 65, 100, 127, 128, 129, 255, 256, 257, 511, 512, 513, 1000, 1023, 1024, 1025, 2047, 2048, 2049, 3000, 4095, 4096);
    const legacyKeys = [sender, generator, other];
    for (const [i, n] of lengths.entries()) {
        await legacyCase(`${n} bytes`, bytesHex(n, 11 + i), legacyKeys[i % legacyKeys.length]);
    }
    await legacyCase("32-byte hash", messages[0][1], sender);

    // Real P2P status preimages: the key-sorted status of a relay whose last block is one of these,
    // signed by two forging delegates.
    const blocks = [
        must(await rec("block.make", 2, { height: 2, timestamp: 8, previousBlock: "cd".repeat(32), reward: "180000000", passphrase: GENERATOR, aux, transactions: [] })),
        block,
    ];
    const statuses = [
        [blocks[0], true, 1],
        [blocks[0], false, 2],
        [blocks[1], true, 2],
        [blocks[1], false, 3],
        [blocks[1], true, 4000000],
    ];
    for (const [i, [b, forgingAllowed, currentSlot]] of statuses.entries()) {
        const pre = must(await rec("status.preimage", H, { block: b.output.hex, forgingAllowed, currentSlot, version: "4.3.1" }));
        out.push({ ...pre, name: `status ${i + 1}` });
        for (const [who, key] of [
            ["generator", generator],
            ["sender", sender],
        ]) {
            await legacyCase(`status ${i + 1}, ${who}`, pre.output.preimage, key);
        }
    }

    // Verification edge cases.
    const m = bytesHex(32, 99);
    const valid = (await rec("sig.legacy.sign", H, { message: m, privateKey: sender.privateKey })).output.signature;
    const uncompressed = Identities.Keys.fromPrivateKey(sender.privateKey, false).publicKey.secp256k1;
    const y = BigInt("0x" + uncompressed.slice(66));
    const hybrid = (y % 2n === 1n ? "07" : "06") + uncompressed.slice(2);
    for (const [name, signature, publicKey] of [
        ["R at infinity", infinitySignature(hex32(1n), sender.publicKey, sender.privateKey, m), sender.publicKey],
        ["R at infinity, r = x of G", infinitySignature("79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798", sender.publicKey, sender.privateKey, m), sender.publicKey],
        ["s = n", valid.slice(0, 64) + N_HEX, sender.publicKey],
        ["s = 0", valid.slice(0, 64) + "00".repeat(32), sender.publicKey],
        ["r = p", P_HEX + valid.slice(64), sender.publicKey],
        ["r = 0", "00".repeat(32) + valid.slice(64), sender.publicKey],
        ["uncompressed key", valid, uncompressed],
        ["hybrid key", valid, hybrid],
        ["x-only key", valid, sender.publicKey.slice(2)],
        ["key prefix flipped", valid, (sender.publicKey.startsWith("02") ? "03" : "02") + sender.publicKey.slice(2)],
        ["63-byte signature", valid.slice(0, 126), sender.publicKey],
        ["65-byte signature", valid + "00", sender.publicKey],
        ["empty signature", "", sender.publicKey],
    ]) {
        await legacy("sig.legacy.verify", name, { message: m, signature, publicKey });
    }
    if (legacyCount < 235) throw new Error(`only ${legacyCount} legacy Schnorr records; at least 235 are required`);
    shared.legacyCount = legacyCount;
    shared.blocks = { statusBlocks: blocks };
    return out;
};
