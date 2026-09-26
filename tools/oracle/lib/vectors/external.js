"use strict";
// V11 external conformance: published vector files, committed next to the vectors with their
// source URL and SHA-256 (external/SOURCES.json), and the reference's answers on their inputs.
//
//   bip340      the official BIP340 test vectors. The reference signs a 32-byte message as given
//               and hashes any other length first, so only the 32-byte rows must match the file;
//               the other rows record what the reference computes.
//   draft       the bip-schnorr draft vectors at d194620, which the reference's legacy Schnorr
//               scheme implements (16 verify rows, 3 of them with a secret key to sign).
//   bip39       the BIP39 English vectors (entropy to mnemonic), with the bip39 package the
//               reference depends on.
//
// gen-vectors.js fails when a file does not have its recorded SHA-256, or when the reference
// disagrees with a row it must match.

const crypto = require("crypto");
const fs = require("fs");
const path = require("path");

function sha256Hex(data) {
    return crypto.createHash("sha256").update(data).digest("hex");
}

function csvRows(text) {
    const [header, ...lines] = text.split(/\r?\n/).filter((l) => l.trim() !== "");
    const keys = header.split(",");
    return lines.map((line) => {
        const cells = line.split(",");
        const row = {};
        keys.forEach((k, i) => (row[k] = cells.slice(i, i === keys.length - 1 ? undefined : i + 1).join(",").trim()));
        return row;
    });
}

/** Load the external files and check them against SOURCES.json. */
function loadSources(dir) {
    const sources = JSON.parse(fs.readFileSync(path.join(dir, "SOURCES.json"), "utf8"));
    const files = {};
    for (const s of sources.files) {
        const bytes = fs.readFileSync(path.join(dir, s.file));
        const actual = sha256Hex(bytes);
        if (actual !== s.sha256) throw new Error(`external/${s.file}: SHA-256 ${actual}, SOURCES.json says ${s.sha256}`);
        files[s.id] = { ...s, text: bytes.toString("utf8"), length: bytes.length };
    }
    return files;
}

module.exports = async function external({ rec, shared, options }) {
    const files = loadSources(options.externalDir);
    shared.external = Object.fromEntries(Object.values(files).map((f) => [f.file, f.sha256]));
    const H = shared.height;
    const out = [];
    const lower = (hex) => hex.toLowerCase();

    for (const f of Object.values(files)) {
        out.push({ op: "external.file", network: "devnet", height: H, name: f.id, input: { file: `external/${f.file}`, url: f.url }, output: { sha256: f.sha256, length: f.length } });
    }

    // BIP340.
    for (const row of csvRows(files.bip340.text)) {
        const message = lower(row.message);
        const expected = row["verification result"] === "TRUE";
        const exact = message.length === 64;
        const name = exact ? `bip340 ${row.index}` : `bip340 ${row.index} (${message.length / 2}-byte message, hashed first)`;
        if (row["secret key"]) {
            const s = { ...(await rec("sig.bip340.sign", H, { message, privateKey: lower(row["secret key"]), aux: lower(row.aux_rand) })), name };
            out.push(s);
            if (exact && (s.error || s.output.signature !== lower(row.signature))) throw new Error(`${name}: the reference's signature differs from the file`);
        }
        const v = { ...(await rec("sig.bip340.verify", H, { message, signature: lower(row.signature), publicKey: lower(row["public key"]) })), name };
        out.push(v);
        if (exact && !v.error && v.output.valid !== expected) throw new Error(`${name}: the reference's verdict differs from the file`);
        if (exact && v.error && expected) throw new Error(`${name}: the reference refused a valid row`);
    }

    // bip-schnorr draft (legacy Schnorr).
    for (const row of csvRows(files.draft.text)) {
        const name = `draft ${row.index}`;
        const message = lower(row.message);
        const signature = lower(row.signature);
        if (row["secret key"]) {
            const s = { ...(await rec("sig.legacy.sign", H, { message, privateKey: lower(row["secret key"]) })), name };
            out.push(s);
            if (s.error || s.output.signature !== signature) throw new Error(`${name}: the reference's legacy signature differs from the file`);
        }
        const v = { ...(await rec("sig.legacy.verify", H, { message, signature, publicKey: lower(row["public key"]) })), name };
        out.push(v);
        const expected = row["verification result"] === "TRUE";
        if ((v.error ? false : v.output.valid) !== expected) throw new Error(`${name}: the reference's legacy verdict differs from the file`);
    }

    // BIP39 English: entropy to mnemonic.
    for (const [i, [entropy, mnemonic]] of JSON.parse(files.bip39.text).english.entries()) {
        const r = { ...(await rec("bip39.entropyToMnemonic", H, { entropy })), name: `bip39 english ${i}` };
        out.push(r);
        if (r.error || r.output.mnemonic !== mnemonic) throw new Error(`bip39 english ${i}: the reference's mnemonic differs from the file`);
    }
    return out;
};
