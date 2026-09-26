"use strict";
// The devnet network configuration that every oracle record is produced under:
//   - the rule set at height 1 is Solar mainnet's merged milestone at height 5,000,000, as the
//     reference's own ConfigManager merges it, with a fixed epoch;
//   - leaf overrides on that milestone: five reward tiers, secondaryReward, minFee 6173 and the
//     delegate-registration addonBytes;
//   - network byte 90, 'd' addresses;
//   - donations of 5 % each to placeholder-1 and placeholder-2 from height 2, with the addresses
//     derived from the devnet key-derivation seed and tag (DEFAULT_SEED and DERIVATION_TAG below).
//
// Key format. Solar Core 4.3.1 uses {percent} donations and burn.feePercent. The patched reference
// (patches 22 and 23 of its series) changes both presets to {basisPoints} and burn.feeBasisPoints
// and refuses the old keys. The burn object is copied from the reference's own mainnet preset, so it
// is always in that build's format; the donations follow the same format, detected from the preset.
// The detected format is recorded in every vector file's meta record.
//
// The genesis block is empty: the oracle does not boot a chain, and the empty genesis keeps
// isGenesisTransaction() out of every record.

const crypto = require("crypto");

const RULES_HEIGHT = 5000000;
// The devnet key-derivation seed and tag. They are input to every derived key, so changing either
// changes the donation addresses and with them the milestones hash in the meta record.
const DEFAULT_SEED = "solar-devnet";
const DEFAULT_EPOCH = "2026-01-01T00:00:00.000Z";
const DERIVATION_TAG = "solar-devnet-v1/";
const PUB_KEY_HASH = 90;
// Any 64-hex value is accepted; a live network uses its genesis payload hash.
const NETHASH = "00".repeat(32);

// Block rewards by delegate rank: 1-10 1.8, 11-21 1.9, 22-32 2.0, 33-43 2.1, 44-53 2.2 ROOT.
const TIERS = [
    { from: 1, to: 10, reward: 180000000 },
    { from: 11, to: 21, reward: 190000000 },
    { from: 22, to: 32, reward: 200000000 },
    { from: 33, to: 43, reward: 210000000 },
    { from: 44, to: 53, reward: 220000000 },
];

function tierRanks() {
    const ranks = {};
    for (const t of TIERS) for (let r = t.from; r <= t.to; r++) ranks[String(r)] = t.reward;
    return ranks;
}

// Leaf overrides on the merged mainnet milestone.
const RULES = {
    "dynamicReward.ranks": tierRanks(),
    "dynamicReward.secondaryReward": 180000000,
    "dynamicFees.minFee": 6173,
    "dynamicFees.addonBytes.delegateRegistration": 1214968,
};

const DONATIONS = [
    { role: "placeholder-1", percent: 5, basisPoints: 500, purpose: "placeholder-1" },
    { role: "placeholder-2", percent: 5, basisPoints: 500, purpose: "placeholder-2" },
];

function clone(value) {
    return JSON.parse(JSON.stringify(value));
}

function setLeaf(obj, dotted, value) {
    const keys = dotted.split(".");
    let o = obj;
    for (const k of keys.slice(0, -1)) {
        if (o[k] === null || typeof o[k] !== "object" || Array.isArray(o[k])) o[k] = {};
        o = o[k];
    }
    o[keys[keys.length - 1]] = clone(value);
}

/** Devnet key derivation: the 12-word mnemonic of SHA-256(tag + role + "/" + seed)[0..16]. */
function deriveRoleKeys(solar, role, seed) {
    const entropy = crypto
        .createHash("sha256")
        .update(`${DERIVATION_TAG}${role}/${seed}`, "utf8")
        .digest()
        .subarray(0, 16);
    const passphrase = solar.bip39.entropyToMnemonic(entropy.toString("hex"));
    return solar.lib.Identities.Keys.fromPassphrase(passphrase);
}

/**
 * Build the devnet configuration and load it into the reference's ConfigManager.
 * Returns { config, info } where config is the object passed to setConfig().
 */
function buildDevnet(solar, { seed = DEFAULT_SEED, epoch = DEFAULT_EPOCH } = {}) {
    const { Identities, Managers } = solar.lib;
    const cm = Managers.configManager;

    cm.setFromPreset("mainnet");
    const merged = clone(cm.getMilestone(RULES_HEIGHT));
    const testnet = cm.getPreset("testnet").network;

    const sample = Object.values(merged.donations || {})[0];
    const donationFormat = sample && "basisPoints" in sample ? "basisPoints" : "percent";
    const burnFormat = merged.burn && "feeBasisPoints" in merged.burn ? "feeBasisPoints" : "feePercent";

    const milestone = { height: 1 };
    for (const [key, value] of Object.entries(merged)) {
        if (key !== "height") milestone[key] = clone(value);
    }
    milestone.epoch = epoch;
    milestone.donations = {};
    for (const [leaf, value] of Object.entries(RULES)) setLeaf(milestone, leaf, value);

    const donations = {};
    for (const d of DONATIONS) {
        const address = Identities.Address.fromPublicKey(deriveRoleKeys(solar, d.role, seed).publicKey.secp256k1, PUB_KEY_HASH);
        donations[address] =
            donationFormat === "basisPoints" ? { basisPoints: d.basisPoints, purpose: d.purpose } : { percent: d.percent, purpose: d.purpose };
    }
    const milestones = [milestone, { height: 2, donations }];

    const sampleAddress = Identities.Address.fromPublicKey(deriveRoleKeys(solar, "generator", seed).publicKey.secp256k1, PUB_KEY_HASH);
    const network = {
        name: "devnet",
        messagePrefix: "Solar devnet message:\n",
        addressCharacter: sampleAddress[0],
        bip32: clone(testnet.bip32),
        pubKeyHash: PUB_KEY_HASH,
        nethash: NETHASH,
        wif: testnet.wif,
        slip44: testnet.slip44,
        client: { token: "dROOT", symbol: "dRT", explorer: "" },
    };

    const config = { network, milestones, exceptions: {}, genesisBlock: { transactions: [] } };
    cm.setConfig(clone(config));
    cm.setHeight(1);

    const milestonesJson = JSON.stringify(milestones, null, 4) + "\n";
    return {
        config,
        info: {
            name: network.name,
            pubKeyHash: PUB_KEY_HASH,
            profile: "iceroot-devnet",
            seed,
            epoch,
            rulesHeight: RULES_HEIGHT,
            keyFormat: { donations: donationFormat, burn: burnFormat },
            milestonesSha256: crypto.createHash("sha256").update(milestonesJson).digest("hex"),
        },
    };
}

module.exports = { DEFAULT_EPOCH, DEFAULT_SEED, PUB_KEY_HASH, buildDevnet };
