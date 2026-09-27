"use strict";
// Oracle operations of the forger's decisions and of the internal routes' bodies:
//   forger.decide           {round, networkState, status, slots, timeLeftInMs, privateKey,
//                           firstAttempt, pool?, fail?, milestones?} -> {action, reason, lines}: one
//                           attempt of the reference's ForgerService.forgeNewBlock, with its
//                           isForgingAllowed, over a stub client that answers from the input
//                           (forger-service.ts). `round` and `networkState` are the relay's JSON
//                           bodies; `status` is the status header {timestamp?, generatorPublicKey?};
//                           `slots` answers the four getSlotNumber calls in their order (the round's
//                           slot, the current slot, the last block's slot, the block's slot); `pool`
//                           is {poolSize, transactions: [hex]}; `fail` {call, message} makes one
//                           call (networkState, roundSlot, transactions, status, currentSlot,
//                           lastBlockSlot, blockSlot) fail as a RelayCommunicationError. The
//                           forger's clock leaves `timeLeftInMs` in the round's slot. `action` is
//                           stop, error, forged, refused, late or sameSlot; `reason` names a
//                           refusal (unknown, coldStart, belowMinimum, alreadyForged, quorum);
//                           `lines` are [level, message] up to the pause after the attempt.
//   internal.networkState   {status, lastBlock?, lastSlotNumber?, quorumDetails?} or {body} ->
//                           {json, read, activeDelegates}: a NetworkState built by its own class as
//                           a relay builds it, written by the getNetworkState codec, and read back
//                           by NetworkState.parse; {body} reads a body given as text
//   internal.roundJson      {wallets, active, current, next, lastBlock, reward, timestamp,
//                           canForge, round} -> {json}: the object getCurrentRound returns, over
//                           the state package's wallets and a block the reference makes, written by
//                           the getCurrentRound codec (controllers/internal.ts)
//   internal.currentRound   {json} -> {read}: the fields the forger reads from such a body
// Loaded by lib/ops.js.

const path = require("path");

const LEVELS = ["emergency", "alert", "critical", "error", "warning", "notice", "info", "debug"];
// The routes of the stub client's calls, for the endpoint of a failure.
const ROUTES = {
    networkState: "p2p.internal.getNetworkState",
    roundSlot: "p2p.internal.getSlotNumber",
    transactions: "p2p.transactions.getUnconfirmedTransactions",
    status: "p2p.peer.getStatus",
    currentSlot: "p2p.internal.getSlotNumber",
    lastBlockSlot: "p2p.internal.getSlotNumber",
    blockSlot: "p2p.internal.getSlotNumber",
};
const SLOT_CALLS = ["roundSlot", "currentSlot", "lastBlockSlot", "blockSlot"];

module.exports = function forgerDecideOps({ solar, cm, withConfig, hexBuffer, safeInteger, plain }) {
    let reference;
    // The `delay` module of the forger, whose waits the operation stubs: `pause` runs instead of
    // the wait while it is set.
    const pause = { hook: undefined };

    function load() {
        if (reference) return reference;
        const packages = path.join(solar.solarDir, "packages");
        const forgerDir = path.join(packages, "forger");
        const delayPath = require.resolve("delay", { paths: [forgerDir] });
        const realDelay = require(delayPath);
        const stubDelay = Object.assign((ms, ...rest) => (pause.hook ? pause.hook(ms) : realDelay(ms, ...rest)), realDelay);
        require.cache[delayPath].exports = stubDelay;
        const dist = (pkg, file) => require(path.join(packages, pkg, "dist", file));
        for (const pkg of ["forger", "p2p", "state"]) {
            const shared = require(require.resolve("@solar-network/crypto", { paths: [path.join(packages, pkg)] }));
            if (shared !== solar.lib) throw new Error(`@solar-network/${pkg} loaded a second copy of @solar-network/crypto`);
        }
        const { Services } = dist("kernel", "index");
        const attributes = new Services.Attributes.AttributeSet();
        for (const key of ["votes", "delegate", "delegate.username", "delegate.voteBalance", "delegate.rank", "delegate.round"]) {
            attributes.set(key);
        }
        reference = {
            ForgerService: dist("forger", "forger-service").ForgerService,
            Delegate: dist("forger", "delegate").Delegate,
            errors: dist("forger", "errors"),
            NetworkState: dist("p2p", "network-state").NetworkState,
            codecs: dist("p2p", path.join("socket-server", "codecs", "internal")),
            Wallet: dist("state", "index").Wallets.Wallet,
            AttributeMap: Services.Attributes.AttributeMap,
            attributes,
        };
        return reference;
    }

    function object(value, name) {
        if (value === null || typeof value !== "object" || Array.isArray(value)) throw new TypeError(`${name} must be an object`);
        return value;
    }

    /** A block the reference makes and a relay keeps as its tip: its data, with the username set. */
    function lastBlock({ passphrase, height, timestamp, username }) {
        const { Blocks, Identities } = solar.lib;
        const keys = Identities.Keys.fromPassphrase(passphrase);
        const block = Blocks.BlockFactory.make(
            {
                version: 0,
                generatorPublicKey: keys.publicKey.secp256k1,
                timestamp: safeInteger(timestamp, "lastBlock.timestamp"),
                previousBlock: "0".repeat(64),
                height: safeInteger(height, "lastBlock.height"),
                numberOfTransactions: 0,
                totalAmount: solar.lib.Utils.BigNumber.ZERO,
                totalFee: solar.lib.Utils.BigNumber.ZERO,
                reward: solar.lib.Utils.BigNumber.ZERO,
                payloadLength: 0,
                payloadHash: solar.lib.Crypto.HashAlgorithms.sha256([]).toString("hex"),
                transactions: [],
            },
            keys,
            Buffer.alloc(32, 7),
        );
        if (username !== undefined) block.data.username = username;
        return block;
    }

    return {
        "forger.decide": (input) =>
            withConfig(input.milestones, async () => {
                const { ForgerService, Delegate, errors, NetworkState } = load();
                const { round, networkState, status, slots, timeLeftInMs, privateKey, firstAttempt, pool, fail } = input;
                object(round, "round");
                if (!Array.isArray(slots) || slots.length !== 4) throw new TypeError("slots must be four slot numbers");
                if (fail !== undefined && !Object.prototype.hasOwnProperty.call(ROUTES, fail.call)) throw new TypeError(`unknown call ${fail.call}`);
                const lines = [];
                const service = Object.create(ForgerService.prototype);
                service.logger = Object.fromEntries(LEVELS.map((level) => [level, (message) => lines.push([level, message])]));
                const delegate = new Delegate(hexBuffer(privateKey, "privateKey").toString("hex"));
                const names = (wallets) => Object.fromEntries(wallets.map((w) => [w.publicKey, w.delegate.username]));
                Object.assign(service, {
                    delay: 0,
                    errorCount: 0,
                    logAppReady: true,
                    transactions: [],
                    round,
                    initialised: true,
                    delegates: [delegate],
                    usernames: names(round.delegates),
                    allUsernames: names(round.allDelegates),
                });
                service.activeDelegates = service.delegates.filter((d) => service.usernames.hasOwnProperty(d.publicKey));
                service.inactiveDelegates = service.delegates.filter((d) => !service.activeDelegates.includes(d));
                const failing = (call) => {
                    if (fail !== undefined && fail.call === call) {
                        throw new errors.RelayCommunicationError(`127.0.0.1:4002<${ROUTES[call]}>`, fail.message);
                    }
                };
                let slotCalls = 0;
                let posted = false;
                service.client = {
                    getNetworkState: async () => {
                        failing("networkState");
                        return NetworkState.parse(networkState);
                    },
                    getSlotNumber: async () => {
                        const call = SLOT_CALLS[slotCalls];
                        failing(call);
                        return slots[slotCalls++];
                    },
                    getTransactions: async () => {
                        failing("transactions");
                        const answer = pool ?? { poolSize: 0, transactions: [] };
                        return { poolSize: answer.poolSize, transactions: answer.transactions.map((hex) => hexBuffer(hex, "transaction")) };
                    },
                    getStatus: async () => {
                        failing("status");
                        return { state: { header: status } };
                    },
                    broadcastBlock: async () => {
                        posted = true;
                    },
                    emitEvent: () => {},
                    getRound: async () => round,
                };
                let allowed;
                service.isForgingAllowed = (...args) => (allowed = ForgerService.prototype.isForgingAllowed.apply(service, args));

                // The forger's clock: `timeLeftInMs` before the end of the round's slot.
                const milestone = cm.getMilestone(round.lastBlock.height);
                const epoch = new Date(cm.getMilestone(1).epoch).getTime();
                const now = epoch + round.timestamp * 1000 + milestone.blockTime * 1000 - safeInteger(timeLeftInMs, "timeLeftInMs");
                const realNow = Date.now;
                const realImmediate = global.setImmediate;
                let attempt;
                let seen;
                Date.now = () => now;
                global.setImmediate = (fn) => {
                    attempt = attempt ?? fn;
                };
                pause.hook = async () => {
                    seen = seen ?? lines.slice();
                };
                try {
                    await service.forgeNewBlock(delegate, firstAttempt === true, round);
                    await attempt();
                } finally {
                    Date.now = realNow;
                    global.setImmediate = realImmediate;
                    pause.hook = undefined;
                }
                let action;
                if (seen === undefined) action = "stop";
                else if (service.errorCount !== 0) action = "error";
                else if (posted) action = "forged";
                else if (allowed === false) action = "refused";
                else if (seen.some(([, message]) => message.startsWith("Failed to forge new block"))) action = "late";
                else action = "sameSlot";
                const logged = seen ?? lines;
                let reason = null;
                if (action === "refused") {
                    const refusals = { 4: "unknown", 3: "coldStart", 1: "belowMinimum" };
                    const parsed = await NetworkState.parse(networkState);
                    if (refusals[parsed.status] !== undefined) reason = refusals[parsed.status];
                    else if (logged.some(([, message]) => message.includes("already forged a block"))) reason = "alreadyForged";
                    else reason = "quorum";
                }
                return { action, reason, lines: logged };
            }),

        "internal.networkState": async ({ status, lastBlock: tip, lastSlotNumber, quorumDetails, body }) => {
            const { NetworkState, codecs } = load();
            let json;
            if (body !== undefined) {
                if (typeof body !== "string") throw new TypeError("body must be text");
                json = body;
            } else {
                const state = new NetworkState(safeInteger(status, "status"));
                if (tip !== undefined) {
                    // setLastBlock without a monitor sets the tip's height, id and generator; the
                    // relay's monitor then sets its slot.
                    await state.setLastBlock(lastBlock(object(tip, "lastBlock")));
                    if (lastSlotNumber !== undefined) state.lastSlotNumber = safeInteger(lastSlotNumber, "lastSlotNumber");
                }
                if (quorumDetails !== undefined) Object.assign(state.quorumDetails, object(quorumDetails, "quorumDetails"));
                json = codecs.getNetworkState.response.serialise(state).toString();
            }
            const parsed = await NetworkState.parse(codecs.getNetworkState.response.deserialise(Buffer.from(json)));
            const read = {
                status: parsed.status,
                nodeHeight: parsed.getNodeHeight() ?? null,
                lastBlockId: parsed.getLastBlockId() ?? null,
                lastGenerator: parsed.getLastGenerator() ?? null,
                lastSlotNumber: parsed.getLastSlotNumber() ?? null,
                quorum: parsed.getQuorum(),
                canForge: parsed.canForge(),
                overHeightBlockHeaders: plain(parsed.getOverHeightBlockHeaders()),
            };
            return { json, read, activeDelegates: cm.getMilestone().activeDelegates };
        },

        "internal.roundJson": ({ wallets, active, current, next, lastBlock: tip, reward, timestamp, canForge, round }) => {
            const { Wallet, AttributeMap, attributes, codecs } = load();
            const { Identities, Utils } = solar.lib;
            if (!Array.isArray(wallets) || !Array.isArray(active)) throw new TypeError("wallets and active must be arrays");
            const made = wallets.map(({ passphrase, username, balance, nonce, rank }) => {
                const publicKey = Identities.Keys.fromPassphrase(passphrase).publicKey.secp256k1;
                const wallet = new Wallet(Identities.Address.fromPublicKey(publicKey), new AttributeMap(attributes));
                wallet.setPublicKey(publicKey);
                wallet.setBalance(Utils.BigNumber.make(balance));
                wallet.setNonce(Utils.BigNumber.make(nonce));
                wallet.setAttribute("delegate", { username, voteBalance: Utils.BigNumber.make(balance), rank });
                return wallet;
            });
            const listed = (wallet) => ({ ...wallet.getData(), delegate: wallet.getAttribute("delegate") });
            const delegates = active.map((index) => listed(made[index]));
            const answer = {
                allDelegates: made.map(listed),
                canForge,
                current: round,
                currentForger: current === null ? undefined : delegates[current],
                delegates,
                lastBlock: lastBlock(object(tip, "lastBlock")).data,
                nextForger: next === null ? undefined : delegates[next],
                reward,
                timestamp,
            };
            return { json: codecs.getCurrentRound.response.serialise(answer).toString() };
        },

        "internal.currentRound": ({ json }) => {
            if (typeof json !== "string") throw new TypeError("json must be text");
            const parsed = load().codecs.getCurrentRound.response.deserialise(Buffer.from(json));
            const pairs = (list) => list.map((wallet) => [wallet.publicKey, wallet.delegate.username]);
            const read = {
                timestamp: parsed.timestamp,
                reward: parsed.reward,
                lastBlockHeight: parsed.lastBlock.height,
                currentForger: parsed.currentForger?.publicKey ?? null,
                nextForger: parsed.nextForger?.publicKey ?? null,
                delegates: pairs(parsed.delegates),
                allDelegates: pairs(parsed.allDelegates),
            };
            return { read };
        },
    };
};
