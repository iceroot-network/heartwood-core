"use strict";
// Oracle operations of the peer verifier, over the reference's own classes:
//
//   nsect.find {n, low, high, common, stop?} -> {probes: [[height, ...]], result}
//       the kernel's NSect over [low, high] with a probe that answers the highest height asked
//       at or below `common` (nothing when none is), or nothing at probe number `stop`
//
//   verifier.checkState {chain, delegates, rounds, peer, claimed, fast, deadline?, milestones?}
//       -> {result, probes, fetched}
//       the reference's PeerVerifier (packages/p2p/src/peer-verifier.ts), made with Object.create
//       and given stubs for what it reads of the node, the peer and the clock:
//         chain       our chain: the serialised headers (signature included) from height 1, hex;
//                     the state store's last blocks and last height, and the database
//                     interceptor's getBlocksByHeight, answer from it
//         delegates   [[publicKey, username], ...]: the registered delegates, which the
//                     blockchain's own setBlockUsername reads through the wallet repository
//         rounds      [[round, [publicKey, ...]], ...]: the delegates getActiveDelegates gives for
//                     each round; for any other round it gives none, and the DPoS state's current
//                     round is none of them, so the verifier's assertion throws there
//         peer        {fork, branch, perReply?, common?}: the peer's chain is ours up to `fork`,
//                     then the `branch` headers; getBlocks serves at most `perReply` of them per
//                     request; getCommonBlocks answers honestly (the highest block among the ids
//                     that the peer has), except for the probes `common` scripts by number:
//                     "honest", null (no common block) or {height, id}
//         claimed     {height, header}: the state the peer reported and its last block's header
//         fast        checkStateFast instead of checkState
//         deadline    the requests allowed before the deadline passes: Date.now() reads 0 before
//                     the first request and 1,000 ms more after each, and the deadline is
//                     1,000 x deadline - 500 ms (no deadline when absent)
//       A header is serialised hex, or {hex, set}: the header of the hex with fields replaced as
//       the peer sends them (null removes one; amounts are decimal text). A header reaches the
//       verifier as the reply checks leave it: amounts as BigNumber, the generator key lowercase.
//       `result` is the verification ({myHeight, theirHeight, highestCommonHeight}, the common
//       height absent from a fast one without it), null when the chain does not verify, or
//       "stopped" when the verifier throws, with the error in `notes`. `probes` holds the heights
//       of our ids in each getCommonBlocks, `fetched` the [fromBlockHeight, blockLimit] of each
//       getBlocks.
// Loaded by lib/ops.js.

const path = require("path");

module.exports = function p2pVerifierOps(context) {
    const { solar, withConfig, safeInteger, hexBuffer, describe } = context;
    const { Blocks, Utils } = solar.lib;
    const load = (pkg, sub = "") => require(path.join(solar.solarDir, "packages", pkg, "dist", sub));
    let reference;
    const classes = () => {
        if (!reference) {
            const kernel = load("kernel");
            reference = {
                NSect: kernel.Utils.NSect,
                Identifiers: kernel.Container.Identifiers,
                PeerVerifier: load("p2p", "peer-verifier").PeerVerifier,
                setBlockUsername: load("blockchain", "blockchain").Blockchain.prototype.setBlockUsername,
            };
        }
        return reference;
    };

    const AMOUNTS = ["totalAmount", "totalFee", "reward"];
    // The fields of a header in the order the protobuf encoder writes them.
    const WIRE_ORDER = ["id", "version", "timestamp", "previousBlock", "height", "numberOfTransactions", ...AMOUNTS];
    const WIRE_TAIL = ["payloadLength", "payloadHash", "generatorPublicKey", "blockSignature"];

    /** A header's data, from its serialised hex, with its computed id. */
    function headerData(hex, name) {
        const { data } = Blocks.Deserialiser.deserialise(hexBuffer(hex, name), true);
        return data;
    }

    /** A header as the peer sends it, after the reply checks: keys in wire order. */
    function sent(header, name) {
        const { hex, set } = typeof header === "string" ? { hex: header, set: {} } : header;
        const data = headerData(hex, name);
        const out = {};
        for (const key of [...WIRE_ORDER, ...WIRE_TAIL]) out[key] = data[key];
        for (const [key, value] of Object.entries(set || {})) {
            if (!(key in out)) throw new TypeError(`${name}: ${key} is not a header field`);
            if (value === null) delete out[key];
            else out[key] = value;
        }
        for (const key of AMOUNTS) if (key in out) out[key] = Utils.BigNumber.make(out[key]);
        if (typeof out.generatorPublicKey === "string") out.generatorPublicKey = out.generatorPublicKey.toLowerCase();
        return out;
    }

    const silent = { debug() {}, info() {}, warning() {}, error() {} };

    return {
        "nsect.find": async ({ n, low, high, common, stop }) => {
            for (const [value, name] of [
                [n, "n"],
                [low, "low"],
                [high, "high"],
                [common, "common"],
            ])
                safeInteger(value, name);
            const probes = [];
            const probe = async (heights) => {
                probes.push(heights);
                if (stop === probes.length - 1) return undefined;
                const matched = heights.filter((h) => h <= common);
                return matched.length ? matched[matched.length - 1] : undefined;
            };
            const result = await new (classes().NSect)(n, probe).find(low, high);
            return { probes, result: result === undefined ? null : result };
        },

        "verifier.checkState": async ({ chain, delegates, rounds, peer, claimed, fast, deadline, milestones }) =>
            withConfig(milestones, async () => {
                const { Identifiers, PeerVerifier, setBlockUsername } = classes();
                if (!Array.isArray(chain) || chain.length === 0) throw new TypeError("chain must list our headers");
                const ours = chain.map((hex, i) => headerData(hex, `chain[${i}]`));
                ours.forEach((d, i) => {
                    if (d.height !== i + 1) throw new TypeError(`chain[${i}] is at height ${d.height}`);
                });
                const ourHeight = ours.length;
                const heightById = new Map(ours.map((d) => [d.id, d.height]));
                safeInteger(peer.fork, "peer.fork");
                if (peer.fork < 0 || peer.fork > ourHeight) throw new TypeError("peer.fork must be a height of our chain");
                const branch = (peer.branch || []).map((header, i) => sent(header, `peer.branch[${i}]`));
                const theirs = [...chain.slice(0, peer.fork).map((hex) => sent(hex, "chain")), ...branch];
                // The blocks the peer stores, by id, with their heights: ours up to the fork, then
                // its branch as signed, whatever fields it sends otherwise.
                const stored = new Map(ours.slice(0, peer.fork).map((d) => [d.id, d.height]));
                (peer.branch || []).forEach((header, i) => {
                    stored.set(headerData(typeof header === "string" ? header : header.hex, "peer.branch").id, peer.fork + i + 1);
                });

                const usernames = new Map(delegates);
                const walletRepository = {
                    hasByPublicKey: (key) => usernames.has(key),
                    findByPublicKey: (key) => ({
                        hasAttribute: (name) => name === "delegate.username" && usernames.has(key),
                        getAttribute: (name) => (name === "delegate.username" ? usernames.get(key) : undefined),
                    }),
                };
                const roundDelegates = new Map(rounds);
                const wallet = (key) => ({
                    getPublicKey: () => key,
                    getAttribute: (name) => (name === "delegate.username" ? usernames.get(key) : undefined),
                });

                const lastBlocks = chain.slice(Math.max(0, ourHeight - 100)).map((hex) => Blocks.BlockFactory.fromData(headerData(hex, "chain")));

                let now = 0;
                const deadlineMs = deadline === undefined || deadline === null ? Infinity : safeInteger(deadline, "deadline") * 1000 - 500;
                const probes = [];
                const fetched = [];
                const script = peer.common || [];
                const communicator = {
                    hasCommonBlocks: async (_peer, ids) => {
                        const index = probes.length;
                        probes.push(ids.map((id) => heightById.get(id)));
                        now += 1000;
                        const scripted = script[index];
                        if (scripted !== undefined && scripted !== "honest") return scripted === null ? false : scripted;
                        let best;
                        for (const id of ids) {
                            const height = stored.get(id);
                            if (height !== undefined && (!best || height > best.height)) best = { height, id };
                        }
                        return best || false;
                    },
                    getPeerBlocks: async (_peer, { fromBlockHeight, blockLimit, headersOnly }) => {
                        if (headersOnly !== true) throw new Error("the verifier asked for whole blocks");
                        fetched.push([fromBlockHeight, blockLimit]);
                        now += 1000;
                        const count = Math.min(blockLimit, peer.perReply ?? Infinity);
                        return theirs.slice(fromBlockHeight, fromBlockHeight + count).map((d) => ({ ...d, ip: "192.0.2.7" }));
                    },
                };
                const app = {
                    get: (id) => {
                        if (id === Identifiers.StateStore) return { getLastBlocks: () => lastBlocks, getLastHeight: () => ourHeight };
                        if (id === Identifiers.TriggerService) {
                            return {
                                call: async (name, { roundInfo }) => {
                                    if (name !== "getActiveDelegates") throw new Error(`unexpected trigger ${name}`);
                                    return (roundDelegates.get(roundInfo.round) || []).map(wallet);
                                },
                            };
                        }
                        throw new Error(`unexpected container lookup ${String(id)}`);
                    },
                };
                const verifier = Object.create(PeerVerifier.prototype);
                Object.assign(verifier, {
                    app,
                    blockchain: { setBlockUsername: (data) => setBlockUsername.call({ walletRepository }, data) },
                    dposState: { getRoundInfo: () => ({ round: 0, maxDelegates: 0 }), getRoundDelegates: () => [] },
                    logger: silent,
                    databaseInterceptor: {
                        getBlocksByHeight: async (heights) => heights.filter((h) => h >= 1 && h <= ourHeight).map((h) => ({ ...ours[h - 1] })),
                    },
                    communicator,
                });
                verifier.initialise({ ip: "192.0.2.7", port: 4002 });

                const state = { height: safeInteger(claimed.height, "claimed.height"), header: sent(claimed.header, "claimed.header") };
                const realNow = Date.now;
                Date.now = () => now;
                let result;
                let notes;
                try {
                    const r = fast ? await verifier.checkStateFast(state) : await verifier.checkState(state, deadlineMs);
                    if (r === undefined) result = null;
                    else {
                        result = { myHeight: r.myHeight, theirHeight: r.theirHeight };
                        if (r.highestCommonHeight !== undefined) result.highestCommonHeight = r.highestCommonHeight;
                    }
                } catch (error) {
                    result = "stopped";
                    notes = { error: describe(error) };
                } finally {
                    Date.now = realNow;
                }
                const out = { result, probes, fetched };
                if (notes) out.notes = notes;
                return out;
            }),
    };
};
