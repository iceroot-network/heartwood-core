"use strict";
// V03 non-canonical and removed: wire forms the reference decodes differently from how it
// re-encodes them, forms it refuses, and the removed transaction types. Every case records the
// strict TransactionFactory.fromBytes verdict and, when it decodes, the id the reference computes
// (the SHA-256 of its re-encoding of the decoded data, which need not be the SHA-256 of the wire
// bytes). Removed and unknown types also record whether their transaction handler is active.

const crypto = require("crypto");

const { SIGNATURE } = require("../craft");
const { IPFS_HASH, OTHER, SENDER } = require("./constants");

const sha256Hex = (text) => crypto.createHash("sha256").update(text).digest("hex");

module.exports = async function nonCanonical({ solar, rec, craft, shared }) {
    const { Identities, Utils } = solar.lib;
    const H = shared.height;
    const out = [];
    const decode = async (name, hex, strict = true) => out.push({ ...(await rec("tx.fromBytes", H, strict ? { hex } : { hex, strict })), name });
    const unsigned = (hex) => hex.slice(0, hex.length - 2 * SIGNATURE);
    const edit = (hex, fn) => {
        const b = Buffer.from(hex, "hex");
        fn(b);
        return b.toString("hex");
    };

    const transfer = shared.tx["transfer-1"].hex;
    const withMemo = shared.tx["transfer-1-memo"].hex;
    const vote = shared.tx["vote-1"].hex; // {genesis_1: 100}
    const resignation0 = shared.tx["resignation-0"].hex;

    // Version byte: 0x00 decodes as version 3 (no version check for a falsy version, and the id
    // re-encodes it as 3); every other version but 3 is refused.
    await decode("version-0", edit(transfer, (b) => (b[1] = 0x00)));
    for (const v of [1, 2, 4, 255]) await decode(`version-${v}`, edit(transfer, (b) => (b[1] = v)));
    const patched = (fn) => (u) => {
        const b = Buffer.from(u);
        fn(b);
        return b;
    };
    await decode("version-2-signed-as-2", craft.resign(transfer, SENDER, patched((b) => (b[1] = 2))));
    // Network byte 0: the id is computed with the configured byte, and the rules refuse it.
    await decode("network-0", edit(transfer, (b) => (b[2] = 0)));

    // Vote assets the codec reads into an object: duplicate names collapse, "__proto__" is lost,
    // and the strict schema drops names that do not match the username pattern. "wire" signs the
    // bytes as sent; "canonical" signs what the reference re-encodes the decoded votes to.
    const canonicalVote = unsigned(vote);
    for (const [name, entries, signOvers] of [
        ["vote-duplicate-name-100", [["genesis_1", 10000], ["genesis_1", 10000]], ["wire", "canonical"]],
        ["vote-duplicate-name-50", [["genesis_1", 5000], ["genesis_1", 5000]], ["wire"]],
        ["vote-proto-and-name", [["__proto__", 10000], ["genesis_1", 10000]], ["wire", "canonical"]],
        ["vote-proto-50", [["__proto__", 5000], ["genesis_1", 5000]], ["wire"]],
        ["vote-uppercase-name", [["Genesis_1", 5000], ["genesis_1", 5000]], ["wire"]],
        ["vote-uppercase-only", [["Genesis_1", 10000]], ["wire"]],
        ["vote-empty-name", [["", 10000]], ["wire"]],
        ["vote-invalid-utf8-name", [[Buffer.from([0xff]), 10000]], ["wire"]],
        ["vote-zero-basis-points", [["genesis_1", 0], ["genesis_2", 10000]], ["wire"]],
        ["vote-10001-basis-points", [["genesis_1", 10001]], ["wire"]],
        ["vote-65535-basis-points", [["genesis_1", 65535]], ["wire"]],
    ]) {
        const asset = craft.voteAsset(entries);
        for (const signOver of signOvers) {
            await decode(`${name}-${signOver}-signed`, craft.withAsset(vote, asset, SENDER, signOver === "wire" ? "wire" : canonicalVote));
        }
    }
    // A count byte larger than the entries that follow.
    await decode("vote-count-too-large", craft.withAsset(vote, Buffer.concat([Buffer.from([2]), craft.voteAsset([["genesis_1", 10000]]).subarray(1)]), SENDER));

    // Resignation: type 0 is written as no byte; an explicit 0xff decodes to type 0 as well.
    const canonicalResignation = unsigned(resignation0);
    await decode("resignation-ff-wire-signed", craft.withAsset(resignation0, Buffer.from([0xff]), SENDER));
    await decode("resignation-ff-canonical-signed", craft.withAsset(resignation0, Buffer.from([0xff]), SENDER, canonicalResignation));
    await decode("resignation-10", craft.withAsset(resignation0, Buffer.from([0x10]), SENDER));
    await decode("resignation-0f", craft.withAsset(resignation0, Buffer.from([0x0f]), SENDER));

    // Memo bytes that are not valid UTF-8 decode with replacement characters, which re-encode to
    // other bytes.
    const replaced = craft.withMemoBytes(withMemo, Buffer.from("��", "utf8"), SENDER);
    for (const [name, bytes] of [
        ["memo-invalid-utf8", [0xff, 0xfe]],
        ["memo-overlong-nul", [0xc0, 0x80]],
        ["memo-surrogate", [0xed, 0xa0, 0x80]],
        ["memo-truncated", [0xe2, 0x98]],
    ]) {
        await decode(`${name}-wire-signed`, craft.withMemoBytes(withMemo, Buffer.from(bytes), SENDER));
    }
    await decode("memo-invalid-utf8-canonical-signed", craft.withMemoBytes(withMemo, Buffer.from([0xff, 0xfe]), SENDER, unsigned(replaced)));
    await decode("memo-nul-bytes", craft.withMemoBytes(withMemo, Buffer.from([0x00, 0x00]), SENDER));

    // Trailing bytes after the signature, and signatures of unusual lengths.
    for (const n of [1, 2, 63, 64, 65, 66, 128, 130]) await decode(`trailing-${n}`, transfer + "ab".repeat(n));
    await decode("trailing-64-non-strict", transfer + "ab".repeat(64), false);
    for (const n of [0, 32, 63]) await decode(`signature-${n}-bytes`, unsigned(transfer) + transfer.slice(transfer.length - 2 * SIGNATURE, transfer.length - 2 * SIGNATURE + 2 * n));

    // Extended header: the sender address travels in the header.
    const otherAddress = Identities.Address.fromPassphrase(OTHER, 90);
    const extended = { ...(await rec("tx.build", H, { builder: "transfer", calls: [["recipientId", shared.recipients[1]], ["amount", "7"], ["nonce", "31"], ["senderId", otherAddress]], sign: SENDER })), name: "extended-header-other-sender" };
    out.push(extended);
    if (extended.output) {
        await decode("extended-header-other-sender", extended.output.hex);
        const payload63 = Utils.Base58.decodeCheck(shared.addresses.addr63);
        await decode("extended-header-byte-63-sender", craft.resign(extended.output.hex, SENDER, patched((b) => payload63.copy(b, 58))));
    }
    const own = craft.builderData("transfer", [["recipientId", shared.recipients[1]], ["amount", "7"], ["nonce", "32"]], SENDER);
    own.headerType = 1;
    own.senderId = Identities.Address.fromPassphrase(SENDER, 90);
    await decode("extended-header-own-sender", craft.signAndSerialise(own, SENDER).toString("hex"));

    // Removed types: they still decode (the codecs stay registered); their handlers are inactive.
    const removed = [];
    const lockId = sha256Hex("lock transaction");
    const secret = sha256Hex("unlock secret");
    for (const [name, input] of [
        ["ipfs-1-5", { builder: "ipfs", calls: [["ipfsAsset", IPFS_HASH], ["nonce", "40"]], sign: SENDER }],
        ["htlc-lock-1-8", { builder: "htlcLock", calls: [["lockAsset", { secretHash: sha256Hex(Buffer.from(secret, "hex")), expiration: { type: 2, value: 1000 } }], ["recipientId", shared.recipients[2]], ["amount", "1000"], ["nonce", "41"]], sign: SENDER }],
        ["htlc-claim-1-9", { builder: "htlcClaim", calls: [["claimAsset", { lockTransactionId: lockId, unlockSecret: secret, hashType: 0 }], ["nonce", "42"]], sign: SENDER }],
        ["htlc-refund-1-10", { builder: "htlcRefund", calls: [["refundAsset", { lockTransactionId: lockId }], ["nonce", "43"]], sign: SENDER }],
    ]) {
        const r = { ...(await rec("tx.build", H, input)), name };
        out.push(r);
        if (r.output) removed.push([name, r.output.hex]);
    }
    const participants = ["multisig participant 1", "multisig participant 2", "multisig participant 3"];
    const publicKeys = participants.map((p) => Identities.PublicKey.fromPassphrase(p));
    const multi = {
        ...(await rec("tx.build", H, {
            builder: "multiSignature",
            calls: [["multiSignatureAsset", { min: 2, publicKeys }], ["senderPublicKey", publicKeys[0]], ["nonce", "44"], ...participants.map((p, i) => ["multiSign", p, i])],
            sign: participants[0],
        })),
        name: "multisignature-1-4",
    };
    out.push(multi);
    if (multi.output) removed.push(["multisignature-1-4", multi.output.hex]);

    const legacyTransfer = craft.builderData("transfer", [["nonce", "45"]], SENDER);
    Object.assign(legacyTransfer, { type: 0, amount: Utils.BigNumber.make("1000"), recipientId: shared.recipients[3], expiration: 0 });
    delete legacyTransfer.asset;
    removed.push(["legacy-transfer-1-0", craft.signAndSerialise(legacyTransfer, SENDER).toString("hex")]);
    const legacyVote = craft.builderData("transfer", [["nonce", "46"]], SENDER);
    Object.assign(legacyVote, { type: 3, asset: { votes: ["+genesis_1"] } });
    removed.push(["legacy-vote-1-3", craft.signAndSerialise(legacyVote, SENDER).toString("hex")]);
    for (const [name, hex] of removed) await decode(name, hex);

    // Unknown types do not decode.
    for (const [name, typeGroup, type] of [
        ["unknown-1-11", 1, 11],
        ["unknown-2-1", 2, 1],
        ["unknown-3-0", 3, 0],
        ["unknown-0-6", 0, 6],
    ]) {
        await decode(
            name,
            craft.resign(
                transfer,
                SENDER,
                patched((b) => {
                    b.writeUInt32LE(typeGroup, 3);
                    b.writeUInt16LE(type, 7);
                }),
            ),
        );
    }

    // Handler activation at the rule height: the six kept types, the removed ones, an unknown one.
    for (const [typeGroup, type] of [
        [1, 1], [1, 2], [1, 6], [1, 7], [2, 0], [2, 2],
        [1, 0], [1, 3], [1, 4], [1, 5], [1, 8], [1, 9], [1, 10],
        [1, 11],
    ]) {
        out.push({ ...(await rec("handler.activation", H, { typeGroup, type })), name: `${typeGroup}/${type}` });
    }
    return out;
};
