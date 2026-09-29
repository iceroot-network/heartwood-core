"use strict";
// The relay's side of its forger:
//   relayForger (V30)  forger.round on a chain forged here on the devnet generator's first chain
//                      definition, at a round's start, at a round's end, before the epoch (the
//                      round then has no delegate at the forger's indices) and slots later;
//                      quorum.analyze (cold start, below the minimum, the local peer and the key
//                      removal, an attested key ranked in the top 53 but outside the round's set,
//                      a duplicated key, forked and over-height peers, every total from 0 to 60
//                      for canForge)
//   statusServe (V31)  status.serve with no forger, one key, an unregistered key and a duplicate,
//                      in both halves of a slot and before the epoch
// Without the devnet generator's chain definition (V10) the forger.round records are not
// written. Blocks are signed with fixed aux bytes, so the records are the same on every run.

const { ReplaySession } = require("../corpus");
const { FIXED_AUX } = require("../ops");

const CRYPTO_FILES = ["network", "milestones", "genesisBlock", "exceptions"];
// The relay's tip in the quorum and status records: height 10, in slot 10.
const TIP_HEIGHT = 10;
// The chain of the round records: two blocks into its second round.
const CHAIN_HEIGHT = 55;

/** The keys of the records' delegate `i`. */
function keysOf(solar, i) {
    return solar.lib.Identities.Keys.fromPassphrase(`relay record delegate ${i}`);
}

/** An empty block at `height` and `timestamp` by delegate `signer`, as its bytes in hex. */
function emptyBlock(solar, height, timestamp, signer) {
    const { Blocks, Crypto, Utils } = solar.lib;
    const keys = keysOf(solar, signer);
    const block = Blocks.BlockFactory.make(
        {
            version: 0,
            generatorPublicKey: keys.publicKey.secp256k1,
            timestamp,
            previousBlock: "0".repeat(64),
            height,
            numberOfTransactions: 0,
            totalAmount: Utils.BigNumber.ZERO,
            totalFee: Utils.BigNumber.ZERO,
            reward: Utils.BigNumber.ZERO,
            payloadLength: 0,
            payloadHash: Crypto.HashAlgorithms.sha256([]).toString("hex"),
            transactions: [],
        },
        keys,
        FIXED_AUX,
    );
    return block.serialised.toString("hex");
}

/** The network state records. */
async function quorumRecords({ solar, devnet, rec, must }) {
    const out = [];
    const epochMs = new Date(devnet.config.milestones[0].epoch).getTime();
    const pub = (i) => keysOf(solar, i).publicKey.secp256k1;
    const secret = (i) => keysOf(solar, i).privateKey;
    const range = (from, to) => Array.from({ length: to - from }, (_, i) => from + i);
    // Delegates 0 to 59 rank 1 to 60; delegate 60 is registered without a rank; 61 is no delegate.
    const delegates = (keys) =>
        [...new Set(keys)].filter((i) => i <= 60).map((i) => ({ publicKey: pub(i), username: `relay_${i}`, rank: i === 60 ? null : i + 1 }));
    const tip = { hex: emptyBlock(solar, TIP_HEIGHT, 80, 0) };
    const header = (height) => ({
        id: `${height}`.padStart(64, "0"),
        version: 0,
        timestamp: height * 8,
        previousBlock: `${height - 1}`.padStart(64, "0"),
        height,
        numberOfTransactions: 0,
        totalAmount: "0",
        totalFee: "5",
        reward: "180000000",
        payloadLength: 0,
        payloadHash: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        generatorPublicKey: pub(3),
        blockSignature: "ab".repeat(64),
    });
    const peer = (last, keys, height, options = {}) => ({
        ip: `192.0.2.${last}`,
        publicKeys: keys.map(pub),
        state: options.state === null ? null : { height, forgingAllowed: options.forgingAllowed ?? true, currentSlot: options.slot ?? 125, header: header(height) },
        fastVerification: options.fast === undefined ? { myHeight: TIP_HEIGHT, theirHeight: height, highestCommonHeight: Math.min(height, TIP_HEIGHT) } : options.fast,
    });
    // The registered delegates are those of `keys`, of the round's set (`active`, by default the
    // first 53) and the tip's generator.
    const record = async (name, input) => {
        const active = input.active ?? range(0, 53);
        const full = {
            tip,
            delegates: delegates([0, ...input.keys, ...active]),
            active: active.map(pub),
            peers: input.peers,
            declared: (input.declared ?? []).map(secret),
            forgerKeys: (input.forgerKeys ?? input.declared ?? []).map(pub),
            now: epochMs + 1_001_000,
            coldStart: input.coldStart === true,
        };
        if (input.reach !== undefined) full.minimumDelegateReach = input.reach;
        out.push({ ...must(await rec("quorum.analyze", TIP_HEIGHT, full)), name: `quorum: ${name}` });
    };
    const base = [peer(1, [1, 3], TIP_HEIGHT), peer(2, [4], TIP_HEIGHT)];
    await record("no delegate on this node", { keys: [1, 3, 4], peers: base, reach: 1 });
    await record("the local peer takes this node's delegates from the other peers", {
        keys: [1, 2, 3, 4, 7],
        peers: base,
        declared: [1, 2, 1, 7],
        forgerKeys: [1, 2, 7],
        active: range(0, 53).filter((i) => i !== 7),
        reach: 1,
    });
    await record("an attested key ranked in the top 53 outside the round's set", {
        keys: [7, 3],
        peers: [peer(1, [7, 3], TIP_HEIGHT)],
        declared: [7],
        active: range(0, 53).filter((i) => i !== 7),
        reach: 1,
    });
    await record("a duplicated key counts twice", { keys: [3], peers: [peer(1, [3, 5], TIP_HEIGHT)], declared: [3, 3], reach: 1 });
    await record("a declared key the forger does not list", { keys: [1, 2], peers: base, declared: [1, 2], forgerKeys: [1], reach: 1 });
    await record("a declared key that is no delegate", { keys: [61], peers: base, declared: [61, 2], reach: 1 });
    await record("the cold start", { keys: [1, 3, 4], peers: base, coldStart: true, reach: 1 });
    await record("below the default reach", { keys: range(0, 26), peers: [peer(1, range(0, 26), TIP_HEIGHT)] });
    await record("at the default reach", { keys: range(0, 27), peers: [peer(1, range(0, 27), TIP_HEIGHT)] });
    await record("at the default reach with this node's delegates", { keys: range(0, 27), peers: [peer(1, range(0, 20), TIP_HEIGHT)], declared: range(20, 27) });
    await record("below a configured reach", { keys: [1, 2], peers: [peer(1, [1, 2], TIP_HEIGHT)], reach: 3 });
    await record("keys that are no active delegate", { keys: [55, 60, 61, 1], peers: [peer(1, [55, 60, 61], TIP_HEIGHT), peer(2, [1], TIP_HEIGHT)], reach: 1 });
    const unforked = { myHeight: TIP_HEIGHT, theirHeight: TIP_HEIGHT, highestCommonHeight: TIP_HEIGHT };
    await record("peers at other heights, forked, or without a state", {
        keys: range(1, 12),
        peers: [
            peer(1, [1, 2], 12, { slot: 124 }),
            peer(2, [3], 9),
            peer(3, [4], TIP_HEIGHT, { fast: { myHeight: TIP_HEIGHT, theirHeight: TIP_HEIGHT } }),
            peer(4, [5], TIP_HEIGHT, { fast: { myHeight: TIP_HEIGHT, theirHeight: 12, highestCommonHeight: 8 } }),
            peer(5, [6], TIP_HEIGHT, { fast: null }),
            peer(6, [7], TIP_HEIGHT, { forgingAllowed: false }),
            peer(7, [8], TIP_HEIGHT, { state: null, fast: null }),
            peer(8, [9, 10], 14, { forgingAllowed: false, fast: unforked }),
            peer(9, [11], TIP_HEIGHT, { slot: 126 }),
        ],
        declared: [11],
        reach: 1,
    });
    // canForge: every total of delegates counted from 0 to 60, with as many agreeing as it takes
    // and one fewer. A key may count more than once, on a peer that repeats it.
    const counted = (n, from) => range(from, from + n).map((i) => i % 53);
    for (let total = 0; total <= 60; total++) {
        const needed = Math.min(27, total);
        for (const agreeing of new Set([Math.max(0, needed - 1), needed])) {
            const peers = [peer(1, counted(agreeing, 0), TIP_HEIGHT)];
            if (total > agreeing) peers.push(peer(2, counted(total - agreeing, agreeing), TIP_HEIGHT - 1));
            const keys = range(0, Math.min(total, 53));
            await record(`canForge with ${agreeing} of ${total} agreeing`, { keys, peers, active: [], reach: 0 });
        }
    }
    return out;
}

/** The served status records. */
async function statusRecords({ solar, devnet, rec, must }) {
    const out = [];
    const epochMs = new Date(devnet.config.milestones[0].epoch).getTime();
    const network = devnet.config.network;
    const peerConfig = {
        version: "4.3.1",
        network: {
            version: network.pubKeyHash,
            name: network.name,
            nethash: network.nethash,
            explorer: network.client.explorer,
            token: { name: network.client.token, symbol: network.client.symbol },
        },
        plugins: {
            "@solar-network/api": { enabled: true, port: 6003, estimateTotalCount: true },
            "@solar-network/webhooks": { enabled: false, port: 6004 },
        },
    };
    const pub = (i) => keysOf(solar, i).publicKey.secp256k1;
    const secret = (i) => keysOf(solar, i).privateKey;
    const tip = { hex: emptyBlock(solar, TIP_HEIGHT, 80, 0) };
    const registered = [0, 1, 2, 3, 4, 5].map(pub);
    const serve = async (name, forger, now = epochMs + 1_001_000) => {
        const input = { tip, now, peerConfig, registered, forgerKeys: forger.map(secret) };
        out.push({ ...must(await rec("status.serve", TIP_HEIGHT, input)), name: `status: ${name}` });
    };
    await serve("no forger", []);
    await serve("one key", [1]);
    await serve("an unregistered key", [9, 2]);
    await serve("a duplicate", [1, 3, 1]);
    await serve("forging not allowed", [1], epochMs + 1_005_000);
    await serve("before the epoch", [], epochMs - 20_000);
    await serve("before the epoch with a key", [2], epochMs - 20_000);
    return out;
}

/**
 * A forger on the session `s`: the next empty block by the delegate scheduled for the first slot
 * at least `gap` slots after the next one that has a delegate of the chain definition.
 */
function forger(solar, s, passphrases) {
    const { Blocks, Crypto, Identities, Utils } = solar.lib;
    const chain = s.chain;
    const { Utils: AppUtils, Container } = chain.pkgs.kernel;
    const triggers = chain.app.get(Container.Identifiers.TriggerService);
    return async function forge(gap = 0) {
        const last = chain.lastBlock().data;
        const height = last.height + 1;
        const blockTime = solar.lib.Managers.configManager.getMilestone(height).blockTime;
        const lookup = await AppUtils.forgingInfoCalculator.getBlockTimeLookup(chain.app, height);
        const delegates = await triggers.call("getActiveDelegates", { roundInfo: AppUtils.roundCalculator.calculateRound(height) });
        for (let extra = gap; extra < gap + 200; extra++) {
            const timestamp = last.timestamp + blockTime * (1 + extra);
            const info = AppUtils.forgingInfoCalculator.calculateForgingInfo(timestamp, height, lookup);
            const scheduled = delegates[info.currentForger];
            const username = scheduled && scheduled.getAttribute("delegate.username");
            if (!username || !passphrases[username]) continue;
            const { reward } = await chain.roundState.getRewardForBlockInRound(height, scheduled);
            const block = Blocks.BlockFactory.make(
                {
                    version: 0,
                    timestamp,
                    height,
                    previousBlock: last.id,
                    numberOfTransactions: 0,
                    totalAmount: Utils.BigNumber.ZERO,
                    totalFee: Utils.BigNumber.ZERO,
                    reward,
                    payloadLength: 0,
                    payloadHash: Crypto.HashAlgorithms.sha256([]).toString("hex"),
                    transactions: [],
                },
                Identities.Keys.fromPassphrase(passphrases[username]),
                FIXED_AUX,
            );
            return block.serialised.toString("hex");
        }
        throw new Error(`no slot at height ${height} has a delegate of the chain definition`);
    };
}

/** The round records, on a chain forged on the first chain definition. */
async function roundRecords({ solar, devnet, rec, must, shared }) {
    if (!shared.genesisDefinitions) return [];
    const definition = shared.genesisDefinitions[0];
    const crypto = Object.fromEntries(CRYPTO_FILES.map((f) => [f, JSON.parse(definition.files[`crypto/${f}.json`])]));
    const epochMs = new Date(crypto.milestones[0].epoch).getTime();
    const blocks = [];
    const s = new ReplaySession(solar, crypto);
    await s.chain.withConfig(
        () => devnet.restore(),
        async () => {
            await s.start();
            const forge = forger(solar, s, definition.passphrases);
            while (blocks.length + 1 < CHAIN_HEIGHT) {
                // A gap of two slots before every tenth block leaves missed slots in the rounds.
                const hex = await forge(blocks.length % 10 === 9 ? 2 : 0);
                const r = await s.apply(Buffer.from(hex, "hex"));
                if (!r.accepted) throw new Error(`the builder's block ${blocks.length + 2} was refused: ${r.error.class}`);
                blocks.push({ hex, timestamp: r.block.data.timestamp });
            }
        },
    );
    const out = [];
    // `at` in seconds after the epoch; each moment asks the slots of the given timestamps too.
    const round = async (name, tip, moments) => {
        const at = moments.map(([seconds, timestamps]) => ({ now: epochMs + seconds * 1000, timestamps }));
        const input = { chain: { crypto, blocks: blocks.slice(0, tip - 1).map((b) => b.hex) }, at };
        out.push({ ...must(await rec("forger.round", tip, input)), name: `round: ${name}` });
    };
    const after = (tip) => (tip === 1 ? 0 : blocks[tip - 2].timestamp);
    await round("before the epoch: no delegate at the forger's indices", 1, [
        [-20, [null, -17, -1, 0, 7, 8]],
        [-1, [null]],
    ]);
    await round("the first round from the genesis block", 1, [
        [8, [null]],
        [30, [null, 30]],
    ]);
    await round("a round's end", CHAIN_HEIGHT - 3, [
        [after(CHAIN_HEIGHT - 3) + 8, [null, after(CHAIN_HEIGHT - 3)]],
        [after(CHAIN_HEIGHT - 3) + 45, [null]],
    ]);
    await round("a round's start", CHAIN_HEIGHT - 2, [
        [after(CHAIN_HEIGHT - 2) + 8, [null]],
        [after(CHAIN_HEIGHT - 2) + 800, [null, after(CHAIN_HEIGHT - 2)]],
    ]);
    await round("into the second round", CHAIN_HEIGHT, [[after(CHAIN_HEIGHT) + 12, [null]]]);
    return out;
}

module.exports = {
    relayForger: async function relayForgerVectors(ctx) {
        return [...(await roundRecords(ctx)), ...(await quorumRecords(ctx))];
    },
    statusServe: async function statusServeVectors(ctx) {
        return statusRecords(ctx);
    },
};
