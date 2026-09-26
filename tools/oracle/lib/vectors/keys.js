"use strict";
// V01 keys and addresses: passphrase keys, addresses for network bytes 90, 63 and 30,
// Address.validate and Address.toBuffer (including the network-byte complaint), and the public-key
// encodings the reference accepts.

const { PASSPHRASES } = require("./constants");

// secp256k1's field prime p. x = p is not a field element; 02 || x and 03 || x are refused.
const FIELD_P = "fffffffffffffffffffffffffffffffffffffffffffffffffffffffefffffc2f";

function flipParity(prefix) {
    return prefix === "02" ? "03" : "02";
}

module.exports = async function keysAndAddresses({ solar, rec, must, craft, shared }) {
    const { Identities } = solar.lib;
    const out = [];
    const H = shared.height;

    const keys = [];
    for (const passphrase of PASSPHRASES) {
        const r = must(await rec("keys.fromPassphrase", H, { passphrase }));
        keys.push(r);
        out.push(r);
    }
    for (const k of keys) {
        for (const networkByte of [90, 63, 30]) {
            out.push(must(await rec("address.fromPublicKey", H, { publicKey: k.output.publicKey, networkByte })));
        }
    }
    const address = (networkByte, i = 0) =>
        out.find((r) => r.op === "address.fromPublicKey" && r.input.networkByte === networkByte && r.input.publicKey === keys[i].output.publicKey).output.address;
    const addr90 = address(90);
    const addr63 = address(63);
    const addr30 = address(30);
    const badChecksum = addr90.slice(0, -1) + (addr90.endsWith("1") ? "2" : "1");

    // Base58Check strings with a valid checksum and byte 90 whose payload is not 21 bytes: 20
    // bytes give 33 characters, 22 bytes give 35. Address.validate checks the checksum and the
    // first byte only.
    const payload90 = (length) => {
        const b = Buffer.alloc(length, 0x11);
        b[0] = 90;
        return b;
    };
    const short33 = craft.base58Check(payload90(20));
    const long35 = craft.base58Check(payload90(22));
    shared.addresses = { addr90, addr63, addr30, badChecksum, short33, long35 };

    for (const [name, address, networkByte] of [
        ["byte 90 as 90", addr90, 90],
        ["byte 90 as 63", addr90, 63],
        ["byte 63 as 63", addr63, 63],
        ["byte 63 as 90", addr63, 90],
        ["byte 30 as 30", addr30, 30],
        ["byte 30 as 90", addr30, 90],
        ["byte 90, configured byte", addr90, undefined],
        ["byte 63, configured byte", addr63, undefined],
        ["bad checksum", badChecksum, 90],
        ["33 characters, last one dropped", addr90.slice(0, -1), 90],
        ["35 characters, one appended", addr90 + "1", 90],
        ["33 characters, 20-byte payload with a valid checksum", short33, 90],
        ["35 characters, 22-byte payload with a valid checksum", long35, 90],
        ["empty", "", 90],
        ["not Base58 (0, O, I, l)", "d0OIl" + addr90.slice(5), 90],
    ]) {
        const input = networkByte === undefined ? { address } : { address, networkByte };
        out.push({ ...must(await rec("address.validate", H, input)), name });
    }

    for (const [name, address] of [
        ["byte 90", addr90],
        ["byte 63: network byte reported", addr63],
        ["byte 30: network byte reported", addr30],
        ["bad checksum", badChecksum],
        ["20-byte payload", short33],
        ["22-byte payload", long35],
    ]) {
        out.push({ ...(await rec("address.toBuffer", H, { address })), name });
    }

    // The public-key acceptance set: compressed, uncompressed and hybrid encodings, and refusals.
    const compressed = keys[0].output.publicKey;
    const uncompressed = Identities.Keys.fromPrivateKey(keys[0].output.privateKey, false).publicKey.secp256k1;
    const x = uncompressed.slice(2, 66);
    const y = uncompressed.slice(66);
    const yOdd = parseInt(y.slice(-1), 16) % 2 === 1;
    const hybridMatching = (yOdd ? "07" : "06") + x + y;
    const hybridWrong = (yOdd ? "06" : "07") + x + y;
    const yPlusOne = (BigInt("0x" + y) + 1n).toString(16).padStart(64, "0");
    const odd = keys.find((k) => k.output.publicKey.startsWith("03")) || keys[1];
    const candidates = [
        ["02 compressed", keys.find((k) => k.output.publicKey.startsWith("02")).output.publicKey],
        ["03 compressed", odd.output.publicKey],
        ["04 uncompressed", uncompressed],
        ["06/07 hybrid, matching parity", hybridMatching],
        ["06/07 hybrid, wrong parity", hybridWrong],
        ["04 uncompressed, y + 1 (off the curve)", "04" + x + yPlusOne],
        ["compressed, parity flipped", flipParity(compressed.slice(0, 2)) + compressed.slice(2)],
        ["00 prefix", "00" + compressed.slice(2)],
        ["05 prefix", "05" + compressed.slice(2)],
        ["01 prefix", "01" + compressed.slice(2)],
        ["ff prefix", "ff" + compressed.slice(2)],
        ["02, x = p", "02" + FIELD_P],
        ["02, x = 0", "02" + "00".repeat(32)],
        ["02, x = 5 (no point)", "02" + "00".repeat(31) + "05"],
        ["02, x = 7", "02" + "00".repeat(31) + "07"],
        ["x only (32 bytes)", compressed.slice(2)],
        ["00 alone", "00"],
        ["empty", ""],
        ["34 bytes", compressed + "00"],
        ["64 bytes, x and y", x + y],
    ];
    for (const [name, publicKey] of candidates) {
        out.push({ ...(await rec("publicKey.verify", H, { publicKey })), name });
        out.push({ ...(await rec("address.fromPublicKey", H, { publicKey, networkByte: 90 })), name });
    }
    return out;
};
