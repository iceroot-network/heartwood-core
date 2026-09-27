"use strict";
// V10 genesis: the devnet chain definition that the devnet generator (gen-network.js, with its
// default profile) writes for three seeds with a fixed epoch. Each record holds the inputs (seed,
// epoch, and the genesis profile described below) and what the generator wrote: the exact text of
// the four crypto files, the key material of the generator, the delegates, the genesis, donation
// and test wallets, the nethash, the genesis id and the issued amount. A fresh process then
// rebuilds the genesis block with the reference's BlockFactory.fromJson under those files and
// records its id and verification.
//
// The devnet generator is a separate tool of the maintainers. It is not part of the reference
// implementation, of the oracle image or of this repository, and it is not published: V10 is a
// frozen, committed class that only the maintainers regenerate. The meta record names the SHA-256
// of the generator's files, so a regeneration shows which generator wrote it.
//
// The genesis profile is the file that Heartwood's chain-definition driver reads
// (`difftest genesis --profile FILE`): every value the chain definition is built from except the
// seed and the epoch. It is made from the generator's own modules (lib/profiles.js for the
// economics, lib/common.js for the derivation tag and the milestone merge, lib/reference.js for the
// reference's merge rules) and the reference's presets, and it restates the generator's fixed
// conventions (role names, delegate names, the network constants). After every generator run the
// profile is checked against what the generator wrote, so a change on either side fails here.
//
// gen-vectors.js is given the generator's path (--devnet-generator), and in the container the
// generator's directory is mounted read-only.
//
// Run on its own, this file writes the genesis profile:
//
//   node lib/vectors/genesis.js --devnet-generator FILE [--solar-dir DIR] --out FILE

const crypto = require("crypto");
const fs = require("fs");
const os = require("os");
const path = require("path");
const { spawnSync } = require("child_process");

const { DEFAULT_EPOCH, DEFAULT_SEED } = require("../network");

// The generator's default seed, a second text seed and a fixed 32-byte hex seed (arbitrary and
// public; every seed here is a test value).
const SEEDS = [DEFAULT_SEED, "difftest-seed-2", "f43d5aeaedcedf2d73b9033bb2eb110263d208ebcb119764a24a79bf9ddde80d"];
// The name of the genesis profile, which the driver writes into its summary.
const PROFILE_NAME = "iceroot-devnet";
const CRYPTO_FILES = ["network.json", "milestones.json", "genesisBlock.json", "exceptions.json"];

// The generator's fixed conventions (gen-network.js defaults), restated for the profile and checked
// against every run: the mainnet height whose merged milestone is the rule set, the role names, the
// delegate names and the number of test wallets.
const RULES_HEIGHT = 5000000;
const GENERATOR_ROLE = "generator";
const DELEGATES = { count: 53, rolePrefix: "delegate/", usernamePrefix: "genesis_" };
const WALLETS = { count: 20, rolePrefix: "wallet/" };

function sha256Hex(data) {
    return crypto.createHash("sha256").update(data).digest("hex");
}

/** The generator's files, and a digest of each, so a record says which generator wrote it. */
function generatorFiles(generator) {
    const dir = path.dirname(generator);
    const files = [path.basename(generator), "verify-network.js", ...fs.readdirSync(path.join(dir, "lib")).filter((f) => f.endsWith(".js")).map((f) => `lib/${f}`)];
    return Object.fromEntries(files.sort().map((f) => [f, sha256Hex(fs.readFileSync(path.join(dir, f)))]));
}

/**
 * The genesis profile of the generator's default profile, for the reference built at solarDir.
 *
 *   derivationTag        the key-derivation tag
 *   network              network.json without nethash and addressCharacter (the builder sets both)
 *   rules                the reference's mainnet milestones merged at RULES_HEIGHT, without height
 *   overrides            the profile's leaf values on the first milestone (dotted path: value)
 *   generatorRole        the role of the key that signs the genesis block and the issuance record
 *   allocations, supply  the issuance record's items (amounts as decimal strings) and their sum
 *   delegates            count, role prefix and name prefix (roles and names count from 1)
 *   donations            role, basisPoints, purpose; active from donationsFromHeight
 *   wallets              test wallets: key material only, not funded at genesis
 */
function genesisProfile(solarDir, generator) {
    const dir = path.dirname(path.resolve(generator));
    const { DEFAULT_PROFILE, PROFILES } = require(path.join(dir, "lib", "profiles.js"));
    const { DERIVATION_TAG, mergedMilestoneAt } = require(path.join(dir, "lib", "common.js"));
    const { detectReference, readPreset } = require(path.join(dir, "lib", "reference.js"));
    const profile = PROFILES[DEFAULT_PROFILE];
    if (!profile.genesis || !profile.donations || BigInt(profile.delegateBalance) !== 0n) {
        throw new Error(`profile ${profile.name}: the genesis profile needs explicit allocations, donations and no delegate balance`);
    }

    const reference = detectReference(solarDir);
    const merged = mergedMilestoneAt(readPreset(solarDir, "mainnet").milestones, RULES_HEIGHT, reference.replaceKeys);
    const rules = {};
    for (const [key, value] of Object.entries(merged)) if (key !== "height") rules[key] = value;

    const testnet = JSON.parse(fs.readFileSync(path.join(solarDir, "packages", "crypto", "dist", "networks", "testnet", "network.json"), "utf8"));
    const name = profile.network.name;
    return {
        name: PROFILE_NAME,
        derivationTag: DERIVATION_TAG,
        network: {
            name,
            messagePrefix: `Solar ${name} message:\n`,
            bip32: testnet.bip32,
            pubKeyHash: profile.network.pubKeyHash,
            wif: testnet.wif,
            slip44: testnet.slip44,
            client: { token: profile.network.token, symbol: profile.network.symbol, explorer: "" },
        },
        rules,
        overrides: profile.rules,
        generatorRole: GENERATOR_ROLE,
        allocations: profile.genesis.map((g) => ({ role: g.role, amount: g.amount.toString() })),
        supply: profile.supply.toString(),
        delegates: DELEGATES,
        donations: profile.donations.map((d) => ({ role: d.role, basisPoints: d.basisPoints, purpose: d.purpose })),
        donationsFromHeight: profile.donationsFromHeight,
        wallets: WALLETS,
    };
}

/** Set a dotted leaf ("a.b.c"), replacing anything on the way that is not an object. */
function setLeaf(obj, dotted, value) {
    const keys = dotted.split(".");
    let o = obj;
    for (const k of keys.slice(0, -1)) {
        if (o[k] === null || typeof o[k] !== "object" || Array.isArray(o[k])) o[k] = {};
        o = o[k];
    }
    o[keys[keys.length - 1]] = JSON.parse(JSON.stringify(value));
}

/** Fail unless the profile describes what the generator wrote for it. */
function checkProfile(profile, epoch, written) {
    const { network, milestones, wallets, delegateKeys } = written;
    const fail = (what) => {
        throw new Error(`the genesis profile disagrees with the generator's output: ${what}`);
    };
    const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);

    const { nethash, addressCharacter, ...rest } = network;
    if (!same(rest, profile.network)) fail("network.json");
    const first = { height: 1, ...profile.rules, epoch, donations: {} };
    for (const [leaf, value] of Object.entries(profile.overrides)) setLeaf(first, leaf, value);
    if (!same(first, milestones[0])) fail("the first milestone");
    const donationHeights = milestones.slice(1).map((m) => m.height);
    if (!same(donationHeights, profile.donations.length ? [profile.donationsFromHeight] : [])) fail("the donation milestone");

    if (wallets.generator.role !== profile.generatorRole) fail("the generator role");
    const pattern = (p, i) => `${p}${i + 1}`;
    const delegates = delegateKeys.delegates.map((d) => [d.role, d.username]);
    const expected = Array.from({ length: profile.delegates.count }, (_, i) => [pattern(profile.delegates.rolePrefix, i), pattern(profile.delegates.usernamePrefix, i)]);
    if (!same(delegates, expected)) fail("the delegate roles and names");
    const testWallets = wallets.wallets.map((w) => w.role);
    if (!same(testWallets, Array.from({ length: profile.wallets.count }, (_, i) => pattern(profile.wallets.rolePrefix, i)))) fail("the test wallet roles");
    if (!same(wallets.genesis.map((w) => ({ role: w.role, amount: w.genesisBalance })), profile.allocations)) fail("the allocations");
    const donations = wallets.donations.map((w) => ({ role: w.role, basisPoints: w.basisPoints, fromHeight: w.fromHeight }));
    if (!same(donations, profile.donations.map((d) => ({ role: d.role, basisPoints: d.basisPoints, fromHeight: profile.donationsFromHeight })))) {
        fail("the donations");
    }
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
    const { DEFAULT_PROFILE } = require(path.join(dir, "lib", "profiles.js"));
    const profile = genesisProfile(solar.solarDir, generator);
    shared.devnetGenerator = { files: generatorFiles(path.resolve(generator)) };

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
            checkProfile(profile, DEFAULT_EPOCH, {
                network: JSON.parse(files["crypto/network.json"]),
                milestones: JSON.parse(files["crypto/milestones.json"]),
                wallets,
                delegateKeys,
            });
            const wallet = (w) => ({ role: w.role, address: w.address, publicKey: w.publicKey, passphrase: w.passphrase, genesisBalance: w.genesisBalance });
            const check = JSON.parse(run([options.self, "--check-genesis", path.join(target, "crypto"), "--solar-dir", solar.solarDir], "the genesis check"));

            out.push({
                op: "genesis.generate",
                network: "devnet",
                height: 1,
                name: `seed ${i + 1}`,
                input: { seed, epoch: DEFAULT_EPOCH, profile },
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
                    testWallets: wallets.wallets.map((w) => ({ role: w.role, address: w.address, publicKey: w.publicKey, passphrase: w.passphrase })),
                    generator: {
                        role: wallets.generator.role,
                        address: wallets.generator.address,
                        publicKey: wallets.generator.publicKey,
                        passphrase: wallets.generator.passphrase,
                        issued: wallets.generator.issued,
                    },
                    check,
                },
            });
        }
    } finally {
        fs.rmSync(work, { recursive: true, force: true });
    }
    // The chain definitions, for the classes that start a state on them (V18), with the
    // passphrases of the generator, the delegates and the wallets, which sign variants of them.
    shared.genesisDefinitions = out.map((r) => ({
        name: r.name,
        files: r.output.files,
        passphrases: Object.fromEntries([
            ["generator", r.output.generator.passphrase],
            ...r.output.delegates.map((d) => [d.username, d.passphrase]),
            ...r.output.genesisWallets.map((w) => [w.role, w.passphrase]),
            ...r.output.testWallets.map((w) => [w.role, w.passphrase]),
        ]),
    }));
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

module.exports.genesisProfile = genesisProfile;

if (require.main === module) {
    const args = process.argv.slice(2);
    const flag = (name) => {
        const i = args.indexOf(name);
        return i >= 0 && i + 1 < args.length ? args[i + 1] : undefined;
    };
    const generator = flag("--devnet-generator") || process.env.DEVNET_GENERATOR;
    const solarDir = flag("--solar-dir") || process.env.SOLAR_DIR;
    const outFile = flag("--out");
    if (!generator || !solarDir || !outFile) {
        console.error("usage: node lib/vectors/genesis.js --devnet-generator FILE [--solar-dir DIR] --out FILE");
        process.exit(2);
    }
    const profile = genesisProfile(path.resolve(solarDir), generator);
    fs.mkdirSync(path.dirname(path.resolve(outFile)), { recursive: true });
    fs.writeFileSync(outFile, JSON.stringify(profile, null, 4) + "\n");
    console.log(`genesis profile ${profile.name} written to ${outFile}`);
}
