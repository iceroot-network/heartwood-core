"use strict";
// Oracle operation of the block processor (lib/process.js):
//
//   chain.process {session, hex, now, state?}   on a chain session (chain.open)
//   chain.process {chain, hex, now, state?}     on a chain given whole: {crypto: {network,
//                                               milestones, genesisBlock, exceptions}, blocks:
//                                               [hex, ...]}, the blocks after the genesis block
//       The reference's BlockProcessor.process of the block `hex`, rebuilt as a job rebuilds it
//       with the clock at `now` (milliseconds since the Unix epoch), on the chain's tip.
//       `state` sets the chain state the processor reads before it runs:
//         lastDownloaded  null, or {height, id, previousBlock, timestamp}: the last downloaded
//                         block [the tip]
//         forked          null, or the hex of the forked block [none]
//         started         whether the blockchain has started [true]
//         unstored        how many of the chain's last blocks are applied but not stored yet,
//                         as blocks accepted earlier in the same job are [0]
//         forged          transaction ids the store holds besides its blocks' [none]
//         unnamed         whether the block has no delegate name, as a block enqueued before its
//                         delegate registered has none [false]
//       In the session form a key that is absent leaves the session's value, and an accepted
//       block is stored as a job of one block stores it (the session moves on to it); in the
//       chain form the defaults apply, and every case starts from the chain's tip: the chain is
//       replayed once and kept, and after each case its store and state are put back (an accepted
//       block is stored and reverted, and the store's missed and round rows restored), which the
//       state digest checks; after a block refused in its
//       apply, which may leave its sender's key bound (the reference binds it before its checks),
//       the chain is replayed again.
//       -> {result, handler, unchained?, line?, missed, lastDownloaded, forked, clearQueue,
//           wakeUp, removed}: the result's name; the class of the handler that decided; the
//          unchained handler's status; the digest line after an accepted block (in the chain
//          form without cd); the missed rows written while the processor ran (a block refused
//          in its apply after missed slots leaves them); the last downloaded block ({height, id}
//          or null) and the forked block's height (or null) after it; whether the queue was
//          cleared and the wake-up timer restarted; and the ids the pool was asked to remove.
//       A block the reference cannot rebuild is the error it raises.
//
// Loaded by lib/ops.js.

const crypto = require("crypto");

const { ReplaySession } = require("../corpus");
const { hashes, chainSections } = require("../digest");
const { sessions: registry } = require("../pool");
const { makeProcessor } = require("../process");

// Chains of the chain form kept at their tip, the most recently used last.
const KEPT_CHAINS = 16;
// The most blocks a case may leave unstored: the reference's in-memory last blocks.
const MAX_UNSTORED = 100;

module.exports = function processOps(context) {
    const { solar, devnet, hexBuffer, safeInteger } = context;
    const kept = new Map();

    const inSession = (s, fn) => s.chain.withConfig(() => devnet.restore(), fn);

    /** The chain session of `input.chain`, replayed to its tip once and kept. */
    async function keptChain(chain) {
        if (chain === null || typeof chain !== "object" || !Array.isArray(chain.blocks)) throw new TypeError("chain must be {crypto, blocks}");
        const { crypto: files, blocks } = chain;
        if (files === null || typeof files !== "object") throw new TypeError("chain.crypto must be the four crypto files");
        for (const name of ["network", "milestones", "genesisBlock", "exceptions"]) {
            if (files[name] === undefined) throw new TypeError(`chain.crypto.${name} is missing`);
        }
        const key = crypto.createHash("sha256").update(JSON.stringify(chain)).digest("hex");
        let entry = kept.get(key);
        if (entry) {
            kept.delete(key);
            kept.set(key, entry);
            return entry;
        }
        const replay = new ReplaySession(solar, files);
        const sd = await inSession(replay, async () => {
            await replay.start();
            for (const [i, hex] of blocks.entries()) {
                const r = await replay.apply(hexBuffer(hex, `chain.blocks[${i}]`), "apply");
                if (!r.accepted) throw new Error(`chain.blocks[${i}] was refused: ${r.error.class}: ${r.error.message}`);
            }
            return hashes(chainSections(replay.chain, [])).sd.toString("hex");
        });
        entry = { replay, sd };
        kept.set(key, entry);
        while (kept.size > KEPT_CHAINS) kept.delete(kept.keys().next().value);
        return entry;
    }

    /** Set the chain state the processor reads; with `defaults`, an absent key takes its default. */
    function setState(s, made, state, defaults) {
        if (state === null || typeof state !== "object" || Array.isArray(state)) throw new TypeError("state must be an object");
        const chain = s.chain;
        const stateStore = chain.stateStore;
        const tip = stateStore.getLastBlock().data;
        const given = (name) => Object.prototype.hasOwnProperty.call(state, name);
        if (given("lastDownloaded") || defaults) {
            const link = given("lastDownloaded") ? state.lastDownloaded : tip;
            if (link !== null && (typeof link !== "object" || !Number.isSafeInteger(link.height) || typeof link.id !== "string")) {
                throw new TypeError("state.lastDownloaded must be null or {height, id, previousBlock, timestamp}");
            }
            stateStore.setLastDownloadedBlock(link === null ? undefined : link);
        }
        if (given("forked") || defaults) {
            const forked = given("forked") ? state.forked : null;
            if (forked === null) stateStore.clearForkedBlock();
            else stateStore.setForkedBlock(chain.receive(hexBuffer(forked, "state.forked")));
        }
        if (given("started") || defaults) {
            const started = given("started") ? state.started : true;
            if (typeof started !== "boolean") throw new TypeError("state.started must be a boolean");
            stateStore.setStarted(started);
        }
        if (given("unstored") || defaults) {
            const unstored = given("unstored") ? safeInteger(state.unstored, "state.unstored") : 0;
            if (unstored < 0 || unstored > MAX_UNSTORED || unstored >= tip.height) throw new RangeError(`state.unstored must be 0 to ${MAX_UNSTORED}, below the tip's height`);
            stateStore.setLastStoredBlockHeight(tip.height - unstored);
        }
        if (given("forged") || defaults) {
            const forged = given("forged") ? state.forged : [];
            if (!Array.isArray(forged) || forged.some((id) => typeof id !== "string" || !/^[0-9a-f]{64}$/.test(id))) {
                throw new TypeError("state.forged must be a list of transaction ids");
            }
            made.setForged(forged);
        }
    }

    const unnamed = (state) => {
        if (state.unnamed !== undefined && typeof state.unnamed !== "boolean") throw new TypeError("state.unnamed must be a boolean");
        return state.unnamed === true;
    };

    const shown = (out) => {
        const { reward, ...rest } = out;
        return rest;
    };

    return {
        "chain.process": async ({ session: id, chain, hex, now, state = {} }) => {
            if ((id === undefined) === (chain === undefined)) throw new TypeError("chain.process takes a session or a chain");
            const bytes = hexBuffer(hex, "hex");
            safeInteger(now, "now");
            if (id !== undefined) {
                const session = registry.get(id);
                if (session.kind !== "chain") throw new Error(`session ${id} is not a chain session`);
                const s = session.replay;
                return inSession(s, async () => {
                    const made = makeProcessor(solar, s);
                    setState(s, made, state, false);
                    const { out, block } = await made.process(bytes, now, !unnamed(state));
                    if (out.result === "Accepted") {
                        await s.chain.stubs.databaseService.saveBlocks([block]);
                        s.chain.stateStore.setLastStoredBlockHeight(block.data.height);
                        out.line = s.emit("apply", out.reward, out.missed);
                    }
                    return shown(out);
                });
            }
            const entry = await keptChain(chain);
            const s = entry.replay;
            return inSession(s, async () => {
                const made = makeProcessor(solar, s);
                const store = s.chain.store;
                const lengths = { missed: store.missed.length, recorded: store.recorded.length };
                // The revert of an accepted block that starts a round deletes the round rows the
                // chain saved with its tip (the reference's deleteBlocks): the store is put back.
                const rounds = new Map([...store.rounds].map(([round, rows]) => [round, rows.map((row) => ({ ...row }))]));
                setState(s, made, state, true);
                let out;
                try {
                    const processed = await made.process(bytes, now, !unnamed(state));
                    out = processed.out;
                    if (out.result === "Accepted") {
                        await s.chain.stubs.databaseService.saveBlocks([processed.block]);
                        await s.chain.revert(1);
                    }
                } finally {
                    store.missed.length = lengths.missed;
                    store.recorded.length = lengths.recorded;
                    store.rounds = rounds;
                }
                const forget = () => {
                    for (const [key, value] of kept) if (value === entry) kept.delete(key);
                };
                // A block refused in its apply may leave its sender's key bound (the reference binds
                // it before its checks): the chain is replayed again for the next case.
                if (out.handler === "AcceptBlockHandler" && out.result !== "Accepted") {
                    forget();
                } else if (hashes(chainSections(s.chain, [])).sd.toString("hex") !== entry.sd) {
                    forget();
                    throw new Error("the chain did not return to its tip after the case");
                }
                if (out.line) {
                    const { cd, ...line } = out.line;
                    out.line = line;
                }
                return shown(out);
            });
        },
    };
};
