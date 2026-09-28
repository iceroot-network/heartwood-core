"use strict";
// Oracle operations of the relay's side of its forger, over the reference's internal controller,
// network monitor, NetworkState and peer controller with their collaborators stubbed:
//   forger.round    {chain: {crypto, blocks}, at: [{now, timestamps?}]} -> {answers: [{round,
//                   slotNumbers}]}: the reference's first start on the chain definition `crypto`,
//                   the blocks of `blocks` (hex) applied, then for each entry of `at` the
//                   internal controller's getCurrentRound at `now` (milliseconds since the Unix
//                   epoch), written by its codec (`round`, the JSON text), and its getSlotNumber
//                   for each of `timestamps` (null: no timestamp) (controllers/internal.ts)
//   quorum.analyze  {tip: {hex}, delegates: [{publicKey, username, rank}], active, peers,
//                   declared, forgerKeys, now, coldStart, minimumDelegateReach?} -> {json,
//                   canForge, peerKeys}: the network monitor's getNetworkState(false) with its
//                   cleanse stubbed (network-monitor.ts, network-state.ts). `tip` is the relay's
//                   last block (its username set as setBlockUsername sets it); `delegates` the
//                   registered delegates (rank null: none); `active` the keys getActiveDelegates
//                   gives for the round of the tip plus one, every one of them a registered
//                   delegate; each peer is {ip, publicKeys, state, fastVerification}, with state
//                   null (the reference's empty state) or {height, forgingAllowed, currentSlot,
//                   header}, and fastVerification null or {myHeight, theirHeight,
//                   highestCommonHeight} (null: none); `declared` are the private keys of the
//                   relay's delegates.json and `forgerKeys` the public keys of its forger.json,
//                   which getDelegatesOnThisNode reads. `json` is the getNetworkState answer's text,
//                   `canForge` the state's canForge at the tip's milestone, and `peerKeys` each
//                   peer's keys after the analysis took the delegates on this node from them
//   status.serve    {tip: {hex}, now, peerConfig, registered, forgerKeys} -> {hex, preimage}: the
//                   peer controller's getStatus at `now` over the tip, written by the getStatus
//                   codec, and the text it signs (controllers/peer.ts). `peerConfig` is what
//                   getPeerConfig gives; `registered` the public keys of the registered delegates
//                   in allByUsername order; `forgerKeys` the private keys of delegates.json, in file
//                   order, and a live forger lists all of them in forger.json
// The wall clock is fixed at `now`: Date.now() and a Date made without arguments (as dayjs()
// makes it, which the slot calculator reads) give that time. The files the reference reads
// (delegates.json, forger.json) are stubbed for the call. Loaded by lib/ops.js.

const path = require("path");

const SILENT = { emergency() {}, alert() {}, critical() {}, error() {}, warning() {}, notice() {}, info() {}, debug() {} };

/** Run fn with the reference's clock at `nowMs`, then let the work it started settle. */
async function atTime(nowMs, fn) {
    const RealDate = global.Date;
    class FixedDate extends RealDate {
        constructor(...args) {
            if (args.length === 0) super(nowMs);
            else super(...args);
        }

        static now() {
            return nowMs;
        }
    }
    global.Date = FixedDate;
    try {
        const result = await fn();
        // NetworkState sets the tip's slot number after an await of its own.
        await new Promise((resolve) => setImmediate(resolve));
        return result;
    } finally {
        global.Date = RealDate;
    }
}

/** Run fn with the properties of `patches` ([object, key, value]) replaced. */
async function patched(patches, fn) {
    const saved = patches.map(([object, key]) => [object, key, object[key]]);
    for (const [object, key, value] of patches) object[key] = value;
    try {
        return await fn();
    } finally {
        for (const [object, key, value] of saved) object[key] = value;
    }
}

function list(value, name) {
    if (!Array.isArray(value)) throw new TypeError(`${name} must be an array`);
    return value;
}

module.exports = function relayOps({ solar, devnet, hexBuffer, safeInteger }) {
    const { Blocks, Identities } = solar.lib;
    let reference;

    function load() {
        if (reference) return reference;
        const packages = path.join(solar.solarDir, "packages");
        const p2pDir = path.join(packages, "p2p");
        const dist = (pkg, file) => require(path.join(packages, pkg, "dist", file));
        const shared = require(require.resolve("@solar-network/crypto", { paths: [p2pDir] }));
        if (shared !== solar.lib) throw new Error("@solar-network/p2p loaded a second copy of @solar-network/crypto");
        const kernel = require(require.resolve("@solar-network/kernel", { paths: [p2pDir] }));
        if (kernel !== dist("kernel", "index")) throw new Error("@solar-network/p2p loaded a second copy of @solar-network/kernel");
        const codecs = (file) => dist("p2p", path.join("socket-server", "codecs", file));
        reference = {
            kernel,
            NetworkMonitor: dist("p2p", "network-monitor").NetworkMonitor,
            Peer: dist("p2p", "peer").Peer,
            FastPeerVerificationResult: dist("p2p", "peer-verifier").FastPeerVerificationResult,
            InternalController: dist("p2p", path.join("socket-server", "controllers", "internal")).InternalController,
            PeerController: dist("p2p", path.join("socket-server", "controllers", "peer")).PeerController,
            internalCodecs: codecs("internal"),
            peerCodecs: codecs("peer"),
            peerConfig: dist("p2p", path.join("socket-server", "utils", "get-peer-config")),
            forgerDelegates: dist("kernel", path.join("utils", "get-forger-delegates")),
            fsExtra: require(require.resolve("fs-extra", { paths: [p2pDir] })),
        };
        return reference;
    }

    /** The stubs of delegates.json (`declared`) and forger.json (`forgerKeys`) for one call. */
    function keyFiles(declared, forgerKeys) {
        const { forgerDelegates, fsExtra } = load();
        const read = fsExtra.readJsonSync;
        const readJsonSync = (file, ...rest) => (String(file).endsWith("/delegates.json") ? { keys: [...declared] } : read(file, ...rest));
        return [
            [forgerDelegates, "getForgerDelegates", () => [...forgerKeys]],
            [fsExtra, "readJsonSync", readJsonSync],
        ];
    }

    /** The tip of `tip` ({hex}), as the relay keeps it. */
    function tipBlock(tip) {
        if (!tip || typeof tip.hex !== "string") throw new TypeError("tip must be {hex}");
        return Blocks.BlockFactory.fromBytes(hexBuffer(tip.hex, "tip.hex"));
    }

    const publicKeyOf = (secret) => Identities.Keys.fromPrivateKey(hexBuffer(secret, "private key").toString("hex")).publicKey.secp256k1;

    return {
        "quorum.analyze": async (input) => {
            const { kernel, NetworkMonitor, Peer, FastPeerVerificationResult, internalCodecs } = load();
            const lastBlock = tipBlock(input.tip);
            const byKey = new Map(list(input.delegates, "delegates").map((d) => [d.publicKey, d]));
            const wallet = (key) => {
                const delegate = byKey.get(key);
                if (!delegate) throw new TypeError(`${key} is no registered delegate`);
                const attributes = { "delegate.username": delegate.username, "delegate.rank": delegate.rank ?? undefined };
                return {
                    getPublicKey: () => key,
                    hasAttribute: (name) => attributes[name] !== undefined,
                    getAttribute: (name) => attributes[name],
                };
            };
            const walletRepository = { hasByPublicKey: (key) => byKey.has(key), findByPublicKey: wallet };
            // Blockchain.setBlockUsername, as the relay named its tip.
            if (lastBlock.data.version === 0 && byKey.has(lastBlock.data.generatorPublicKey)) {
                lastBlock.data.username = byKey.get(lastBlock.data.generatorPublicKey).username;
            }
            const active = list(input.active, "active").map(wallet);
            const peers = list(input.peers, "peers").map((stub, index) => {
                const peer = new Peer(stub.ip, 4002);
                peer.publicKeys = [...list(stub.publicKeys, `peers[${index}].publicKeys`)];
                if (stub.state !== null) {
                    const { height, forgingAllowed, currentSlot, header } = stub.state;
                    peer.state = { height, forgingAllowed, currentSlot, header: { ...header } };
                }
                const fast = stub.fastVerification;
                if (fast !== null) peer.fastVerificationResult = new FastPeerVerificationResult(fast.myHeight, fast.theirHeight, fast.highestCommonHeight ?? undefined);
                return peer;
            });
            const reach = input.minimumDelegateReach;
            const configuration = {
                getRequired: () => 4002,
                getOptional: (key, fallback) => (key === "minimumDelegateReach" && reach !== undefined ? safeInteger(reach, "minimumDelegateReach") : fallback),
            };
            const Id = kernel.Container.Identifiers;
            const monitor = Object.create(NetworkMonitor.prototype);
            let cold = input.coldStart === true;
            Object.assign(monitor, {
                logger: SILENT,
                app: {
                    get: (id) => (id === Id.BlockchainService ? { getLastBlock: () => lastBlock } : { findBlockByHeights: async () => [] }),
                    getTagged: () => configuration,
                    configPath: () => "/config",
                },
                configuration,
                repository: { getPeers: () => [...peers] },
                walletRepository,
                stateStore: { getLastBlock: () => lastBlock },
                triggers: { call: async () => active },
                cleansePeers: async () => undefined,
                isColdStart: () => cold,
                completeColdStart: () => {
                    cold = false;
                },
            });
            const forgerKeys = list(input.forgerKeys, "forgerKeys");
            const declared = list(input.declared, "declared");
            // The reference's test environment is not honoured: a relay never runs in it here.
            const environment = process.env.CORE_ENV;
            delete process.env.CORE_ENV;
            let state;
            try {
                state = await patched(keyFiles(declared, forgerKeys), () => atTime(safeInteger(input.now, "now"), () => monitor.getNetworkState(false)));
            } finally {
                if (environment !== undefined) process.env.CORE_ENV = environment;
            }
            const json = internalCodecs.getNetworkState.response.serialise(state).toString();
            return { json, canForge: state.canForge(), peerKeys: peers.map((peer) => peer.publicKeys) };
        },

        "status.serve": async ({ tip, now, peerConfig, registered, forgerKeys }) => {
            const { kernel, PeerController, peerCodecs, peerConfig: configModule } = load();
            const lastBlock = tipBlock(tip);
            const controller = Object.create(PeerController.prototype);
            Object.assign(controller, {
                app: { get: () => ({ findBlockByHeights: async () => [] }), configPath: () => "/config" },
                blockchain: { getLastBlock: () => lastBlock },
                walletRepository: { allByUsername: () => list(registered, "registered").map((key) => ({ getPublicKey: () => key })) },
                cachedHeader: undefined,
            });
            const secrets = list(forgerKeys, "forgerKeys");
            const listed = secrets.map(publicKeyOf);
            const patches = [...keyFiles(secrets, listed), [configModule, "getPeerConfig", () => JSON.parse(JSON.stringify(peerConfig))]];
            const header = await patched(patches, () => atTime(safeInteger(now, "now"), () => controller.getStatus({}, {})));
            const preimage = kernel.Utils.stringify({ state: header.state, config: header.config });
            const bytes = peerCodecs.getStatus.response.serialise(header);
            return { hex: Buffer.from(bytes).toString("hex"), preimage };
        },

        "forger.round": async ({ chain, at }) => {
            const { ReplaySession } = require("../corpus");
            const { InternalController, internalCodecs } = load();
            const { crypto, blocks } = chain || {};
            if (!crypto || typeof crypto !== "object") throw new TypeError("chain.crypto must be the chain's crypto files");
            const s = new ReplaySession(solar, crypto);
            return s.chain.withConfig(
                () => devnet.restore(),
                async () => {
                    await s.start();
                    for (const [i, hex] of list(blocks, "chain.blocks").entries()) {
                        const r = await s.apply(hexBuffer(hex, `chain.blocks[${i}]`), "apply");
                        if (!r.accepted) throw new Error(`chain block ${i + 1} was refused: ${r.error.class}: ${r.error.message}`);
                    }
                    const app = s.chain.app;
                    const Id = load().kernel.Container.Identifiers;
                    const controller = Object.create(InternalController.prototype);
                    Object.assign(controller, {
                        app,
                        logger: SILENT,
                        blockchain: { getLastBlock: () => s.chain.lastBlock() },
                        roundState: s.chain.roundState,
                        walletRepository: s.chain.walletRepository,
                        triggers: app.get(Id.TriggerService),
                    });
                    const answers = [];
                    for (const entry of list(at, "at")) {
                        const answer = await atTime(safeInteger(entry.now, "now"), async () => {
                            const round = await controller.getCurrentRound({}, {});
                            const slotNumbers = [];
                            for (const timestamp of entry.timestamps || []) {
                                const payload = timestamp === null ? {} : { timestamp: safeInteger(timestamp, "timestamp") };
                                slotNumbers.push(await controller.getSlotNumber({ payload }, {}));
                            }
                            return { round: internalCodecs.getCurrentRound.response.serialise(round).toString(), slotNumbers };
                        });
                        answers.push(answer);
                    }
                    return { answers };
                },
            );
        },
    };
};
