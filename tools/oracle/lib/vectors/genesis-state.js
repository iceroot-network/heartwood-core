"use strict";
// V18-genesis-state: state.digest of the state after the reference's first start on the genesis
// blocks of V10 (all five sections and their hashes), and of wallet fixtures that between them
// take every shape of a #W record: listed and unlisted wallets, keys, second keys, delegates with
// and without a last block, both resignations and a revoke, voters with one delegate and with a
// split and its remainder, and amounts near 2^63 - 1. Needs V10's chain definitions: without the
// devnet generator the class is not written.

const CRYPTO_FILES = ["network", "milestones", "genesisBlock", "exceptions"];

module.exports = async function genesisState({ solar, rec, must, shared }) {
    if (!shared.genesisDefinitions) return null;
    const { Identities } = solar.lib;
    const out = [];
    for (const definition of shared.genesisDefinitions) {
        const crypto = Object.fromEntries(CRYPTO_FILES.map((f) => [f, JSON.parse(definition.files[`crypto/${f}.json`])]));
        out.push({ ...must(await rec("state.digest", 1, { crypto })), name: `genesis ${definition.name}` });
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
