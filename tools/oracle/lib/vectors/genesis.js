"use strict";
// V10 genesis: the devnet chain definition that the reference implementation's devnet generator
// (gen-network.js, its default profile) writes for three seeds with a fixed epoch. Each record
// holds the generator's inputs (seed, epoch and the profile it read) and what it wrote: the exact
// text of the four crypto files, the delegate keys and the genesis, donation and generator wallets,
// the nethash, the genesis id and the issued amount. A fresh process then rebuilds the genesis block
// with BlockFactory.fromJson under those files and records its id and verification.
//
// The generator is not part of the oracle image; gen-vectors.js is given its path
// (--devnet-generator), and in the container the generator's directory is mounted read-only.

const crypto = require("crypto");
const fs = require("fs");
const os = require("os");
const path = require("path");
const { spawnSync } = require("child_process");

const { DEFAULT_EPOCH, DEFAULT_SEED } = require("../network");

// The generator's default seed, a second text seed and a fixed 32-byte hex seed (arbitrary and
// public; every seed here is a test value).
const SEEDS = [DEFAULT_SEED, "difftest-S-00", "f43d5aeaedcedf2d73b9033bb2eb110263d208ebcb119764a24a79bf9ddde80d"];
const CRYPTO_FILES = ["network.json", "milestones.json", "genesisBlock.json", "exceptions.json"];

function sha256Hex(data) {
    return crypto.createHash("sha256").update(data).digest("hex");
}

/** JSON-safe copy: BigInt amounts become decimal strings. */
function jsonSafe(value) {
    return JSON.parse(JSON.stringify(value, (_, v) => (typeof v === "bigint" ? v.toString() : v)));
}

/** The generator's files, and a digest of each, so a record says which generator wrote it. */
function generatorFiles(generator) {
    const dir = path.dirname(generator);
    const files = [path.basename(generator), "verify-network.js", ...fs.readdirSync(path.join(dir, "lib")).filter((f) => f.endsWith(".js")).map((f) => `lib/${f}`)];
    return Object.fromEntries(files.sort().map((f) => [f, sha256Hex(fs.readFileSync(path.join(dir, f)))]));
}

/** The profile values that decide the chain definition (the generator's descriptive fields left out). */
function profileInput(profile) {
    return jsonSafe({
        name: profile.name,
        network: profile.network,
        rules: profile.rules,
        genesis: profile.genesis,
        supply: profile.supply,
        donations: profile.donations,
        donationsFromHeight: profile.donationsFromHeight,
        delegateBalance: profile.delegateBalance,
    });
}

function run(args, what) {
    const child = spawnSync(process.execPath, args, { encoding: "utf8", env: process.env, maxBuffer: 64 * 1024 * 1024 });
    if (child.status !== 0) throw new Error(`${what} failed (${child.status}):\n${child.stdout}${child.stderr}`);
    return child.stdout;
}

module.exports = async function genesis({ solar, shared, options }) {
    const generator = options.devnetGenerator;
    if (!generator) return null;
    const dir = path.dirname(path.resolve(generator));
    const { DEFAULT_PROFILE, PROFILES } = require(path.join(dir, "lib", "profiles.js"));
    const { DERIVATION_TAG } = require(path.join(dir, "lib", "common.js"));
    const profile = PROFILES[DEFAULT_PROFILE];
    shared.devnetGenerator = { files: generatorFiles(path.resolve(generator)), profile: DEFAULT_PROFILE };

    const out = [];
    const work = fs.mkdtempSync(path.join(os.tmpdir(), "heartwood-genesis-"));
    try {
        for (const [i, seed] of SEEDS.entries()) {
            const target = path.join(work, `seed-${i}`);
            run(
                [path.resolve(generator), "--name", `vectors-${i}`, "--mode", "native", "--profile", DEFAULT_PROFILE, "--seed", seed, "--epoch", DEFAULT_EPOCH, "--solar", solar.solarDir, "--out", target],
                `the devnet generator for seed ${i + 1}`,
            );
            const read = (file) => fs.readFileSync(path.join(target, file), "utf8");
            const files = Object.fromEntries(CRYPTO_FILES.map((f) => [`crypto/${f}`, read(path.join("crypto", f))]));
            const summary = JSON.parse(read("network-summary.json"));
            const wallets = JSON.parse(read("wallets.json"));
            const delegateKeys = JSON.parse(read("delegate-keys.json"));
            const wallet = (w) => ({ role: w.role, address: w.address, publicKey: w.publicKey, passphrase: w.passphrase, genesisBalance: w.genesisBalance });
            const check = JSON.parse(run([options.self, "--check-genesis", path.join(target, "crypto"), "--solar-dir", solar.solarDir], "the genesis check"));

            out.push({
                op: "genesis.generate",
                network: "devnet",
                height: 1,
                name: `seed ${i + 1}`,
                input: {
                    seed,
                    epoch: DEFAULT_EPOCH,
                    delegates: 53,
                    usernamePrefix: "genesis_",
                    rulesHeight: 5000000,
                    derivationTag: DERIVATION_TAG,
                    profile: profileInput(profile),
                },
                output: {
                    files,
                    fileSha256: Object.fromEntries(Object.entries(files).map(([f, text]) => [f, sha256Hex(text)])),
                    nethash: summary.nethash,
                    genesisId: summary.genesisId,
                    transactions: summary.genesisTransactions,
                    issued: summary.issued,
                    seedSha256: summary.keyDerivation.seedSha256,
                    delegates: delegateKeys.delegates.map((d) => ({
                        username: d.username,
                        role: d.role,
                        address: d.address,
                        publicKey: d.publicKey,
                        privateKey: d.privateKey,
                        passphrase: d.passphrase,
                    })),
                    genesisWallets: wallets.genesis.map(wallet),
                    donationWallets: wallets.donations.map((w) => ({ ...wallet(w), basisPoints: w.basisPoints, fromHeight: w.fromHeight })),
                    generator: { address: wallets.generator.address, publicKey: wallets.generator.publicKey, passphrase: wallets.generator.passphrase, issued: wallets.generator.issued },
                    check,
                },
            });
        }
    } finally {
        fs.rmSync(work, { recursive: true, force: true });
    }
    return out;
};

/**
 * The genesis check, run in a fresh process: the reference caches the genesis transaction ids
 * per network byte on first use, so a process that has loaded another configuration of the same
 * byte cannot judge a genesis block. Loads the four files, rebuilds the block with
 * BlockFactory.fromJson at height 1, and decodes every transaction strictly.
 */
module.exports.checkGenesis = function checkGenesis(solar, cryptoDir) {
    const read = (f) => JSON.parse(fs.readFileSync(path.join(cryptoDir, f), "utf8"));
    const { Blocks, Managers, Transactions } = solar.lib;
    const genesisBlock = read("genesisBlock.json");
    Managers.configManager.setConfig({ network: read("network.json"), milestones: read("milestones.json"), exceptions: read("exceptions.json"), genesisBlock });
    Managers.configManager.setHeight(1);
    const block = Blocks.BlockFactory.fromJson(JSON.parse(JSON.stringify(genesisBlock)));
    let strict = 0;
    for (const t of block.transactions) {
        const decoded = Transactions.TransactionFactory.fromBytes(t.serialised, true);
        if (decoded.isVerified && decoded.id === t.id) strict++;
    }
    return {
        id: block.data.id,
        verified: block.verification.verified,
        errors: block.verification.errors.map(String),
        transactions: block.transactions.length,
        strictlyVerified: strict,
        payloadLength: block.data.payloadLength,
    };
};
